//! Client for Discord's local RPC pipe.
//!
//! # Why a reader task
//!
//! Discord multiplexes replies and pushed events over the same connection. The previous
//! implementation wrote a command and then read the *next* frame, assuming it was the reply —
//! which holds only while nothing is subscribed. As soon as `SUBSCRIBE` is used, an event can
//! land between a command and its reply and every subsequent read answers the wrong question.
//!
//! So exactly one task reads the pipe. It routes each frame by nonce: replies wake the command
//! waiting on that nonce, events go to the event channel. Nothing else ever reads.

use common::task_guard::AbortOnDrop;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Duration, timeout};
use tracing::{debug, info, warn};

#[cfg(windows)]
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

#[cfg(unix)]
use {std::env::var, tokio::net::UnixStream};

use crate::error::DiscordError;
use crate::pipe_message::{HEADER_LEN, Opcode, PipeMessage, ResponseKind, error_message};
use crate::voice_channel::{VoiceChannel, parse_channel, parse_channel_select};
use crate::voice_devices::{DeviceKind, VoiceDevices, parse_voice_devices};

/// The account an RPC session is authenticated as, from the `AUTHENTICATE` reply.
///
/// Only what the status panel shows. It is not secret, but it identifies the user, so it is kept
/// in memory and never written to the config file or the logs.
#[derive(Deserialize, Clone, PartialEq, Eq)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    /// The display name, when the user set one.
    pub global_name: Option<String>,
    /// Hash of the avatar image, or `None` for Discord's default avatar.
    pub avatar: Option<String>,
}

impl DiscordUser {
    /// The name Discord itself shows: the display name, falling back to the username.
    pub fn display_name(&self) -> &str {
        self.global_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.username)
    }

    /// The avatar on Discord's CDN, at the size the panel draws it at twice over.
    pub fn avatar_url(&self) -> Option<String> {
        let hash = self.avatar.as_deref()?;
        Some(format!(
            "https://cdn.discordapp.com/avatars/{}/{hash}.png?size=64",
            self.id
        ))
    }
}

/// Redacting, like the credentials: these end up in logs through the worker events.
impl std::fmt::Debug for DiscordUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DiscordUser { .. }")
    }
}

/// The pipe's concrete type, so the rest of the module stays platform independent.
#[cfg(windows)]
type Transport = NamedPipeClient;
#[cfg(unix)]
type Transport = UnixStream;

/// How long a command waits for its reply before giving up.
///
/// Bounded so a dropped or malformed reply cannot park the connection forever; the state machine
/// reconnects instead. Discord answers these in milliseconds.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `AUTHORIZE` waits, which is a different kind of waiting entirely.
///
/// It does not wait for Discord, it waits for a person to read a modal and click Authorize. Under
/// the ordinary command timeout the request expired after ten seconds and the reconnect loop then
/// queued a fresh modal on every retry, so the user was chasing a dialog that kept being replaced
/// by another one.
const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(180);

/// Event Discord pushes once subscribed. Carries the same payload as `GET_VOICE_SETTINGS`.
pub const VOICE_SETTINGS_UPDATE: &str = "VOICE_SETTINGS_UPDATE";

/// Event Discord pushes once subscribed, whenever the user joins, moves or leaves a voice channel.
pub const VOICE_CHANNEL_SELECT: &str = "VOICE_CHANNEL_SELECT";

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DiscordConnectionState {
    NotConnected,
    Connected,
    HandshakeDone,
    Authorized,
    Authenticated,
}

/// Something the reader task observed, delivered to the state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcEvent {
    VoiceSettings {
        mute: bool,
        deafen: bool,
    },
    /// The voice channel the user is in now; `None` once they left.
    VoiceChannel(Option<VoiceChannel>),
    /// The pipe closed or failed. The state machine reconnects with backoff.
    Disconnected,
}

type PendingReplies = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// Signals that Discord answered the handshake with `READY`.
///
/// Held in an `Arc<Mutex<Option<..>>>` because the client has to be `Clone` for the actor, and a
/// oneshot receiver is not.
type ReadySignal = Arc<Mutex<Option<oneshot::Receiver<()>>>>;

/// How long to wait for `READY` before treating the handshake as rejected.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct Connection {
    writer: Arc<Mutex<WriteHalf<Transport>>>,
    pending: PendingReplies,
    ready: ReadySignal,
    _reader_task: Arc<AbortOnDrop>,
}

#[derive(Clone)]
pub struct IpcClient {
    connection: Option<Connection>,
    client_id: Option<String>,
    events: mpsc::UnboundedSender<IpcEvent>,
    pub state: DiscordConnectionState,
}

impl IpcClient {
    pub fn new(events: mpsc::UnboundedSender<IpcEvent>) -> Self {
        Self {
            connection: None,
            client_id: None,
            events,
            state: DiscordConnectionState::NotConnected,
        }
    }

    /// Opens the pipe and starts the reader task.
    ///
    /// Discord numbers its pipes 0-9 so several clients can coexist; the first that accepts a
    /// connection wins.
    pub async fn connect(&mut self) -> Result<(), DiscordError> {
        if self.connection.is_some() || self.state != DiscordConnectionState::NotConnected {
            return Err(DiscordError::ClientAlreadyConnected);
        }

        let pipe = open_pipe().await?;
        let (reader, writer) = tokio::io::split(pipe);

        let writer = Arc::new(Mutex::new(writer));
        let pending: PendingReplies = Arc::new(Mutex::new(HashMap::new()));
        let (ready_tx, ready_rx) = oneshot::channel();

        let reader_task = tokio::spawn(read_loop(
            reader,
            writer.clone(),
            pending.clone(),
            self.events.clone(),
            ready_tx,
        ));

        self.connection = Some(Connection {
            writer,
            pending,
            ready: Arc::new(Mutex::new(Some(ready_rx))),
            _reader_task: AbortOnDrop::new(reader_task),
        });
        self.state = DiscordConnectionState::Connected;
        Ok(())
    }

    pub async fn disconnect(&mut self) {
        if self.state != DiscordConnectionState::NotConnected {
            info!("Discord disconnected");
        }
        // Dropping the connection aborts the reader task and closes the pipe.
        self.connection = None;
        self.client_id = None;
        self.state = DiscordConnectionState::NotConnected;
    }

    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    async fn write_frame(&self, message: PipeMessage) -> Result<(), DiscordError> {
        let connection = self
            .connection
            .as_ref()
            .ok_or(DiscordError::PipeNotConnected)?;
        connection
            .writer
            .lock()
            .await
            .write_all(&message.to_buff())
            .await?;
        Ok(())
    }

    /// Sends a frame carrying a nonce and waits for the matching reply.
    ///
    /// The waiter is registered *before* the frame goes out, otherwise a fast reply could arrive
    /// while there is nothing left to receive it.
    async fn request(
        &self,
        build: impl FnOnce(&str) -> Result<PipeMessage, DiscordError>,
    ) -> Result<Value, DiscordError> {
        self.request_within(COMMAND_TIMEOUT, build).await
    }

    /// Same as [`Self::request`] with an explicit deadline, for commands that wait on a human.
    async fn request_within(
        &self,
        deadline: Duration,
        build: impl FnOnce(&str) -> Result<PipeMessage, DiscordError>,
    ) -> Result<Value, DiscordError> {
        let connection = self
            .connection
            .as_ref()
            .ok_or(DiscordError::PipeNotConnected)?;

        let nonce = crate::pipe_message::generate_nonce();
        let message = build(&nonce)?;

        let (tx, rx) = oneshot::channel();
        connection.pending.lock().await.insert(nonce.clone(), tx);

        if let Err(err) = self.write_frame(message).await {
            connection.pending.lock().await.remove(&nonce);
            return Err(err);
        }

        let payload = match timeout(deadline, rx).await {
            Ok(Ok(payload)) => payload,
            // The sender was dropped: the reader task ended, so the connection is gone.
            Ok(Err(_)) => return Err(DiscordError::PipeNotConnected),
            Err(_) => {
                connection.pending.lock().await.remove(&nonce);
                return Err(DiscordError::CommandTimedOut);
            }
        };

        if let Some(message) = error_message(&payload) {
            return Err(DiscordError::Rpc(message));
        }
        Ok(payload)
    }

    /// Performs the handshake and waits for Discord to answer `READY`.
    ///
    /// Waiting is not optional. Discord ignores commands sent before it has finished processing
    /// the handshake, silently — no error, no reply, nothing. Firing `AUTHORIZE` straight after
    /// writing the handshake is a race: win it and everything works, lose it and the connection
    /// hangs until the command times out, with no clue as to why.
    pub async fn handshake(&mut self, client_id: &str) -> Result<(), DiscordError> {
        if self.state != DiscordConnectionState::Connected {
            return Err(DiscordError::ClientNotConnected);
        }
        let ready = {
            let connection = self
                .connection
                .as_ref()
                .ok_or(DiscordError::PipeNotConnected)?;
            connection.ready.lock().await.take()
        };
        let ready = ready.ok_or(DiscordError::HandshakeFailed)?;

        self.client_id = Some(client_id.to_owned());
        self.write_frame(PipeMessage::handshake(client_id)?).await?;

        match timeout(HANDSHAKE_TIMEOUT, ready).await {
            Ok(Ok(())) => {}
            // The sender was dropped, so the reader ended: Discord closed the connection, which
            // is what a rejected client id looks like.
            Ok(Err(_)) => return Err(DiscordError::HandshakeFailed),
            Err(_) => return Err(DiscordError::HandshakeFailed),
        }

        debug!("Discord handshake acknowledged");
        self.state = DiscordConnectionState::HandshakeDone;
        Ok(())
    }

    /// Prompts the user with Discord's authorisation modal and returns the OAuth code.
    ///
    /// `scopes` is space separated here and sent as a JSON array, which is what Discord expects.
    /// Sending the string as-is makes Discord read it as one scope whose name contains spaces; it
    /// then answers nothing at all and the command times out ten seconds later with no hint why.
    pub async fn authorize(&mut self, scopes: &str) -> Result<String, DiscordError> {
        if self.state != DiscordConnectionState::HandshakeDone {
            return Err(DiscordError::HandshakeNotDone);
        }
        let client_id = self
            .client_id
            .clone()
            .ok_or(DiscordError::ClientIdNotFound)?;

        let scopes: Vec<&str> = scopes.split_whitespace().collect();

        let payload = self
            .request_within(AUTHORIZE_TIMEOUT, |nonce| {
                PipeMessage::command(
                    "AUTHORIZE",
                    nonce,
                    Some(json!({ "client_id": client_id, "scopes": scopes })),
                )
            })
            .await?;

        let code = payload["data"]["code"]
            .as_str()
            .ok_or(DiscordError::NoDataFound)?
            .to_owned();

        self.state = DiscordConnectionState::Authorized;
        Ok(code)
    }

    /// Opens the RPC session with `token`, returning the account it belongs to.
    ///
    /// The account is `None` when the reply does not carry a readable one: the panel then shows
    /// a generic name, which is no reason to fail a working session.
    pub async fn authenticate(&mut self, token: &str) -> Result<Option<DiscordUser>, DiscordError> {
        if self.state == DiscordConnectionState::NotConnected {
            return Err(DiscordError::ClientNotConnected);
        }
        let payload = self
            .request(|nonce| {
                PipeMessage::command(
                    "AUTHENTICATE",
                    nonce,
                    Some(json!({ "access_token": token })),
                )
            })
            .await?;

        info!("Discord connected");
        self.state = DiscordConnectionState::Authenticated;
        Ok(user_from_authenticate(&payload))
    }

    /// Subscribes to voice settings changes.
    ///
    /// This is what removes the old 250 ms poll: Discord pushes a frame whenever the user mutes
    /// or deafens, so the app costs nothing while nothing happens.
    pub async fn subscribe_voice_settings(&self) -> Result<(), DiscordError> {
        self.require_authenticated()?;
        self.request(|nonce| {
            PipeMessage::subscription("SUBSCRIBE", VOICE_SETTINGS_UPDATE, nonce, None)
        })
        .await?;
        debug!("Subscribed to {VOICE_SETTINGS_UPDATE}");
        Ok(())
    }

    /// Subscribes to the user joining and leaving voice channels, for Shift + disconnect.
    pub async fn subscribe_voice_channel_select(&self) -> Result<(), DiscordError> {
        self.require_authenticated()?;
        self.request(|nonce| {
            PipeMessage::subscription("SUBSCRIBE", VOICE_CHANNEL_SELECT, nonce, None)
        })
        .await?;
        debug!("Subscribed to {VOICE_CHANNEL_SELECT}");
        Ok(())
    }

    /// The voice channel the user is in, if any.
    pub async fn get_selected_voice_channel(&self) -> Result<Option<VoiceChannel>, DiscordError> {
        self.require_authenticated()?;
        let payload = self
            .request(|nonce| PipeMessage::command("GET_SELECTED_VOICE_CHANNEL", nonce, None))
            .await?;
        Ok(parse_channel(&payload["data"]))
    }

    /// Joins a voice channel. Answers with the channel Discord joined, which carries its name.
    ///
    /// Without `force`, so Discord refuses rather than move the user out of a channel they are in.
    pub async fn join_voice_channel(
        &self,
        channel_id: &str,
    ) -> Result<Option<VoiceChannel>, DiscordError> {
        self.require_authenticated()?;
        let payload = self
            .request(|nonce| {
                PipeMessage::command(
                    "SELECT_VOICE_CHANNEL",
                    nonce,
                    Some(json!({ "channel_id": channel_id })),
                )
            })
            .await?;
        Ok(parse_channel(&payload["data"]))
    }

    /// Reads the current voice settings.
    ///
    /// Only used once, to seed the initial state after connecting: the response carries the
    /// machine's whole audio device list, so polling it was needlessly expensive.
    pub async fn get_voice_settings(&self) -> Result<(bool, bool), DiscordError> {
        self.require_authenticated()?;
        let payload = self
            .request(|nonce| PipeMessage::command("GET_VOICE_SETTINGS", nonce, None))
            .await?;
        parse_voice_settings(&payload["data"])
    }

    pub async fn set_voice_settings(&self, mute: bool, deafen: bool) -> Result<(), DiscordError> {
        self.require_authenticated()?;
        self.request(|nonce| {
            PipeMessage::command(
                "SET_VOICE_SETTINGS",
                nonce,
                Some(json!({ "mute": mute, "deaf": deafen })),
            )
        })
        .await?;
        Ok(())
    }

    /// Reads the audio devices Discord offers and the ones it uses now.
    ///
    /// Asked for on demand rather than kept up to date: devices come and go with USB, and the list
    /// is only needed when the user switches device or picks which ones to switch between.
    pub async fn get_voice_devices(&self) -> Result<VoiceDevices, DiscordError> {
        self.require_authenticated()?;
        let payload = self
            .request(|nonce| PipeMessage::command("GET_VOICE_SETTINGS", nonce, None))
            .await?;
        parse_voice_devices(&payload["data"])
    }

    /// Switches Discord's microphone or output to the device with `device_id`.
    pub async fn set_voice_device(
        &self,
        kind: DeviceKind,
        device_id: &str,
    ) -> Result<(), DiscordError> {
        self.require_authenticated()?;
        self.request(|nonce| {
            PipeMessage::command(
                "SET_VOICE_SETTINGS",
                nonce,
                Some(json!({ kind.key(): { "device_id": device_id } })),
            )
        })
        .await?;
        Ok(())
    }

    pub async fn select_voice_channel(
        &self,
        channel_id: Option<String>,
    ) -> Result<(), DiscordError> {
        self.require_authenticated()?;
        self.request(|nonce| {
            PipeMessage::command(
                "SELECT_VOICE_CHANNEL",
                nonce,
                Some(json!({ "channel_id": channel_id })),
            )
        })
        .await?;
        Ok(())
    }

    fn require_authenticated(&self) -> Result<(), DiscordError> {
        if self.state != DiscordConnectionState::Authenticated {
            return Err(DiscordError::ClientNotConnected);
        }
        Ok(())
    }
}

/// Reads `mute` and `deaf` out of a voice settings payload.
pub fn parse_voice_settings(data: &Value) -> Result<(bool, bool), DiscordError> {
    let mute = data["mute"].as_bool().ok_or(DiscordError::NoDataFound)?;
    let deafen = data["deaf"].as_bool().ok_or(DiscordError::NoDataFound)?;
    Ok((mute, deafen))
}

#[cfg(windows)]
async fn open_pipe() -> Result<Transport, DiscordError> {
    for i in 0..10 {
        if let Ok(pipe) = ClientOptions::new().open(format!(r"\\?\pipe\discord-ipc-{i}")) {
            return Ok(pipe);
        }
    }
    Err(DiscordError::PipeConnectionFailed)
}

#[cfg(unix)]
async fn open_pipe() -> Result<Transport, DiscordError> {
    let base = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(|key| var(key).ok())
        .unwrap_or_else(|| "/tmp".to_owned());

    for i in 0..10 {
        let path = std::path::Path::new(&base).join(format!("discord-ipc-{i}"));
        if let Ok(stream) = UnixStream::connect(&path).await {
            return Ok(stream);
        }
    }
    Err(DiscordError::PipeConnectionFailed)
}

/// The single reader. Every frame Discord sends passes through here.
///
/// It awaits the pipe rather than polling it, so an idle connection costs no CPU at all.
async fn read_loop(
    mut reader: ReadHalf<Transport>,
    writer: Arc<Mutex<WriteHalf<Transport>>>,
    pending: PendingReplies,
    events: mpsc::UnboundedSender<IpcEvent>,
    ready: oneshot::Sender<()>,
) {
    let mut ready = Some(ready);
    loop {
        let message = match read_frame(&mut reader).await {
            Ok(message) => message,
            Err(err) => {
                debug!("Discord pipe closed: {err}");
                break;
            }
        };

        match message.opcode {
            Opcode::Ping => {
                // Discord expects a pong; staying silent gets the connection dropped.
                if let Ok(pong) = PipeMessage::pong()
                    && let Err(err) = writer.lock().await.write_all(&pong.to_buff()).await
                {
                    debug!("Could not answer Discord's ping: {err}");
                    break;
                }
                continue;
            }
            Opcode::Close => {
                debug!("Discord asked to close the connection");
                break;
            }
            Opcode::Pong => continue,
            _ => {}
        }

        match message.classify() {
            Ok(ResponseKind::Response { nonce, payload }) => {
                if let Some(waiter) = pending.lock().await.remove(&nonce) {
                    // The receiver is gone when the command already timed out; nothing to do.
                    let _ = waiter.send(payload);
                } else {
                    debug!("Reply for an unknown nonce, ignoring");
                }
            }
            Ok(ResponseKind::Event { event, payload }) => {
                if event == "READY"
                    && let Some(signal) = ready.take()
                {
                    let _ = signal.send(());
                }
                if let Some(message) = error_message(&payload) {
                    // An error with no nonce answers no particular command, so without this it
                    // would vanish and the caller would only ever see a timeout.
                    warn!("Discord reported an error: {message}");
                }
                if event == VOICE_SETTINGS_UPDATE {
                    match parse_voice_settings(&payload["data"]) {
                        Ok((mute, deafen)) => {
                            if events
                                .send(IpcEvent::VoiceSettings { mute, deafen })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(err) => warn!("Malformed {VOICE_SETTINGS_UPDATE} payload: {err}"),
                    }
                }
                if event == VOICE_CHANNEL_SELECT {
                    let channel = parse_channel_select(&payload["data"]);
                    debug!("Voice channel selected: {channel:?}");
                    if events.send(IpcEvent::VoiceChannel(channel)).is_err() {
                        break;
                    }
                }
            }
            Ok(ResponseKind::Unsolicited(payload)) => {
                debug!(
                    "Unsolicited frame from Discord: cmd={:?} evt={:?}",
                    payload.get("cmd").and_then(|v| v.as_str()),
                    payload.get("evt").and_then(|v| v.as_str())
                );
            }
            Err(err) => warn!("Could not parse a frame from Discord: {err}"),
        }
    }

    // Whatever ended the loop, the connection is unusable. Clearing the waiters stops pending
    // commands from blocking until their timeout expires.
    pending.lock().await.clear();
    let _ = events.send(IpcEvent::Disconnected);
}

/// The account in an `AUTHENTICATE` reply, if it carries a readable one.
fn user_from_authenticate(payload: &Value) -> Option<DiscordUser> {
    match serde_json::from_value(payload["data"]["user"].clone()) {
        Ok(user) => Some(user),
        Err(err) => {
            debug!("The AUTHENTICATE reply has no readable user: {err}");
            None
        }
    }
}

async fn read_frame(reader: &mut ReadHalf<Transport>) -> Result<PipeMessage, DiscordError> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header).await?;
    let (opcode, length) = PipeMessage::parse_header(header)?;

    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload).await?;

    Ok(PipeMessage::new(
        opcode,
        String::from_utf8_lossy(&payload).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authenticate_reply(user: Value) -> Value {
        json!({ "cmd": "AUTHENTICATE", "data": { "scopes": ["rpc"], "user": user } })
    }

    #[test]
    fn the_user_is_read_out_of_the_authenticate_reply() {
        let reply = authenticate_reply(json!({
            "id": "80351110224678912", "username": "mechardo", "discriminator": "0",
            "global_name": "Mechardo", "avatar": "8342729096ea3675442027381ff50dfe"
        }));
        let user = user_from_authenticate(&reply).expect("a user");

        assert_eq!(user.display_name(), "Mechardo");
        assert_eq!(
            user.avatar_url().as_deref(),
            Some(
                "https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png?size=64"
            )
        );
    }

    #[test]
    fn a_user_without_display_name_or_avatar_falls_back() {
        let reply = authenticate_reply(json!({
            "id": "1", "username": "mechardo", "global_name": null, "avatar": null
        }));
        let user = user_from_authenticate(&reply).expect("a user");

        assert_eq!(user.display_name(), "mechardo");
        assert_eq!(user.avatar_url(), None);
    }

    #[test]
    fn a_reply_without_a_user_is_not_an_error() {
        assert_eq!(user_from_authenticate(&json!({ "data": {} })), None);
    }

    #[test]
    fn the_user_is_redacted_in_logs() {
        let reply = authenticate_reply(json!({ "id": "1", "username": "mechardo" }));
        let user = user_from_authenticate(&reply).expect("a user");

        assert!(!format!("{user:?}").contains("mechardo"));
    }

    #[test]
    fn voice_settings_are_read_out_of_a_payload() {
        let data = json!({ "mute": true, "deaf": false, "input": { "volume": 100.0 } });
        assert_eq!(parse_voice_settings(&data).expect("parses"), (true, false));
    }

    #[test]
    fn a_payload_missing_the_flags_is_an_error_not_a_default() {
        // Silently defaulting to false here would light the wrong state on the hardware.
        assert!(parse_voice_settings(&json!({ "deaf": true })).is_err());
        assert!(parse_voice_settings(&json!({})).is_err());
    }

    #[tokio::test]
    async fn commands_are_refused_before_authentication() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = IpcClient::new(tx);

        assert!(matches!(
            client.get_voice_settings().await,
            Err(DiscordError::ClientNotConnected)
        ));
        assert!(matches!(
            client.set_voice_settings(true, false).await,
            Err(DiscordError::ClientNotConnected)
        ));
    }

    #[tokio::test]
    async fn a_fresh_client_reports_no_connection() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = IpcClient::new(tx);

        assert!(!client.is_connected());
        assert_eq!(client.state, DiscordConnectionState::NotConnected);
    }
}
