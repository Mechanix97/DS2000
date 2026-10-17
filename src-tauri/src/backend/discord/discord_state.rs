//! Connection state machine for Discord RPC.
//!
//! Nothing here runs on a timer while the connection is healthy. Voice state arrives as
//! `VOICE_SETTINGS_UPDATE` events pushed by Discord, and the only scheduled work is the
//! reconnect attempt, which backs off when Discord is not running.

use crate::credentials::{DISCORD_SCOPES, DiscordCredentials};
use crate::error::DiscordError;
use crate::ipc::{DiscordConnectionState, DiscordUser, IpcClient, IpcEvent};
use crate::oauth;
use spawned_concurrency::tasks::{
    CallResponse, CastResponse, GenServer, GenServerHandle, send_after,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc};
use tokio::time::Duration;
use tracing::{debug, info, warn};

/// First delay after a failed connection attempt.
const RECONNECT_BACKOFF_MIN: Duration = Duration::from_millis(500);
/// Ceiling for the backoff. Discord being closed for an hour must not mean an hour of retries.
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);

pub type DiscordStateHandler = GenServerHandle<DiscordState>;

#[derive(Clone)]
pub enum InCallMessage {
    DiscordStatus,
    AccessToken,
    RefreshToken,
    VoiceSettings,
    Shutdown,
}

#[derive(Clone)]
pub enum InMessage {
    /// Attempts to bring the connection up. Rescheduled with backoff only while it fails.
    Connect,
    /// Result of an attempt that ran off the actor.
    ConnectFinished(Box<Result<Connection, ConnectFailure>>),
    SetVoiceSetting(bool, bool),
    DisconnectChannel,
    /// Applies credentials entered at runtime, so connecting does not require a restart.
    SetCredentials(Box<Option<DiscordCredentials>>),
    /// Something the reader task observed.
    Ipc(IpcEvent),
}

#[derive(Clone, PartialEq)]
pub enum OutMessage {
    Done,
    DiscordStatus(DiscordConnectionState),
    AccessToken(Option<String>),
    RefreshToken(Option<String>),
    VoiceSettings(DiscordVoiceSettings),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DiscordVoiceSettings {
    pub mute: bool,
    pub deafen: bool,
}

/// Something worth telling the controller about.
///
/// Emitted only on an actual change, so a consumer can treat every one of these as news and does
/// not need to compare against what it already knew.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DiscordWorkerEvent {
    VoiceSettingsChanged(DiscordVoiceSettings),
    ConnectionChanged {
        connected: bool,
    },
    /// Discord is showing its authorisation modal and nothing will progress until the user
    /// accepts it.
    ///
    /// Worth announcing because that modal is drawn inside Discord's own window: with Discord
    /// minimised to the tray it is queued and invisible, and the app otherwise just sits there
    /// looking broken for three minutes.
    AwaitingAuthorization,
    /// The account the session is authenticated as, announced on every successful connection.
    UserChanged(Option<DiscordUser>),
    /// Whether Discord itself is running, judged by whether its RPC pipe is there. Lets the UI
    /// say "Discord is closed" instead of a vaguer "not connected".
    RunningChanged {
        running: bool,
    },
}

/// `Clone` is required by the actor framework, not by this code: the state is moved through each
/// handler and handed back. Everything cloneable here is either cheap or an `Arc`.
#[derive(Clone)]
pub struct DiscordState {
    ipc_client: IpcClient,
    /// Kept so a connection attempt running off the actor can build its own client wired to the
    /// same event channel.
    events: mpsc::UnboundedSender<IpcEvent>,
    /// Readable without going through the mailbox, so a caller asking "are we connected?" is not
    /// blocked while a connection attempt is in flight.
    connected: Arc<AtomicBool>,
    /// True while an attempt is running off the actor, so overlapping attempts cannot pile up.
    connecting: bool,
    /// `None` until the user registers a Discord application. While it is `None` the state
    /// machine stays parked instead of retrying a connection it cannot possibly complete.
    credentials: Option<DiscordCredentials>,
    voice_settings: Arc<Mutex<DiscordVoiceSettings>>,
    /// Where changes are announced. The controller listens here instead of polling.
    observer: mpsc::UnboundedSender<DiscordWorkerEvent>,
    /// Last connection state announced, so `ConnectionChanged` really means changed.
    announced_connected: bool,
    /// Last "is Discord running" announced; `None` until the first attempt says either way.
    announced_running: Option<bool>,
    reconnect_backoff: Duration,
    /// Guards against several reconnect timers piling up, which would defeat the backoff.
    reconnect_scheduled: bool,
    shutdown: bool,
}

impl DiscordState {
    pub async fn spawn(
        credentials: Option<DiscordCredentials>,
        voice_settings: Arc<Mutex<DiscordVoiceSettings>>,
        observer: mpsc::UnboundedSender<DiscordWorkerEvent>,
        connected: Arc<AtomicBool>,
    ) -> DiscordStateHandler {
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();

        let state = Self {
            ipc_client: IpcClient::new(events_tx.clone()),
            events: events_tx,
            connected: connected.clone(),
            connecting: false,
            credentials,
            voice_settings,
            observer,
            announced_connected: false,
            announced_running: None,
            reconnect_backoff: RECONNECT_BACKOFF_MIN,
            reconnect_scheduled: false,
            shutdown: false,
        };
        let handle = state.start();

        // Bridges the reader task's events into the actor's mailbox, so the actor only ever
        // reacts to messages and never polls anything.
        let mut forward_to = handle.clone();
        tokio::spawn(async move {
            while let Some(event) = events_rx.recv().await {
                if forward_to.cast(InMessage::Ipc(event)).await.is_err() {
                    break;
                }
            }
        });

        handle
    }

    /// Starts a connection attempt on its own task.
    ///
    /// Deliberately *not* run inside the message handler. The sequence includes `AUTHORIZE`,
    /// which blocks until the user clicks Discord's authorisation modal, and an actor handles one
    /// message at a time. Doing it inline left the actor mute for as long as that modal stayed
    /// open, so every `call` in that window timed out, breaking the Discord tab during the one
    /// moment a new user is actually looking at it.
    fn begin_connecting(&mut self, handle: &GenServerHandle<Self>) {
        if self.connecting || self.shutdown {
            return;
        }
        let Some(credentials) = self.credentials.clone() else {
            return;
        };

        self.connecting = true;
        let events = self.events.clone();
        let observer = self.observer.clone();
        let mut handle = handle.clone();

        tokio::spawn(async move {
            let outcome = connect(credentials, events, observer).await;
            let _ = handle
                .cast(InMessage::ConnectFinished(Box::new(outcome)))
                .await;
        });
    }

    fn set_tokens(&mut self, access_token: Option<String>, refresh_token: Option<String>) {
        if let Some(credentials) = self.credentials.as_mut() {
            credentials.access_token = access_token;
            credentials.refresh_token = refresh_token;
        }
    }

    /// Records the voice state and announces it, but only if it actually moved.
    async fn update_voice_settings(&mut self, settings: DiscordVoiceSettings) {
        let mut current = self.voice_settings.lock().await;
        if *current == settings {
            return;
        }
        *current = settings;
        drop(current);

        let _ = self
            .observer
            .send(DiscordWorkerEvent::VoiceSettingsChanged(settings));
    }

    /// Announces a connection transition, once per transition.
    fn announce_connection(&mut self) {
        let connected = self.ipc_client.state == DiscordConnectionState::Authenticated;
        // Published outside the mailbox too, so status stays readable while the actor is busy.
        self.connected.store(connected, Ordering::Relaxed);

        if connected == self.announced_connected {
            return;
        }
        self.announced_connected = connected;
        let _ = self
            .observer
            .send(DiscordWorkerEvent::ConnectionChanged { connected });
    }

    /// Announces whether Discord is running, once per change.
    fn announce_running(&mut self, running: bool) {
        if self.announced_running == Some(running) {
            return;
        }
        self.announced_running = Some(running);
        let _ = self
            .observer
            .send(DiscordWorkerEvent::RunningChanged { running });
    }

    /// Schedules another attempt, doubling the delay up to the ceiling.
    fn schedule_reconnect(&mut self, handle: &GenServerHandle<Self>) {
        if self.shutdown || self.credentials.is_none() || self.reconnect_scheduled {
            return;
        }
        self.reconnect_scheduled = true;
        send_after(self.reconnect_backoff, handle.clone(), InMessage::Connect);
        self.reconnect_backoff = (self.reconnect_backoff * 2).min(RECONNECT_BACKOFF_MAX);
    }
}

/// Why a connection attempt did not succeed.
///
/// A summary rather than the original error because the actor framework requires its messages to
/// be `Clone`, and `DiscordError` wraps I/O and HTTP errors that are not. The actor only needs to
/// tell "Discord is closed" apart from anything else; the detail is already logged where it
/// happened.
#[derive(Clone, Debug)]
pub enum ConnectFailure {
    /// The RPC pipe is not there, which is simply what a closed Discord looks like.
    DiscordNotRunning,
    /// The user has to decide something before another attempt can succeed: accept the modal,
    /// fix the redirect URI, supply a valid secret.
    ///
    /// Retrying these on a timer is worse than useless. It throws a modal at the user every few
    /// seconds, and Discord rate-limits the repeated requests until it stops answering AUTHORIZE
    /// altogether — which is exactly what happened while debugging this flow.
    NeedsUserAction(String),
    Other(String),
}

impl From<DiscordError> for ConnectFailure {
    fn from(err: DiscordError) -> Self {
        match err {
            DiscordError::PipeConnectionFailed => ConnectFailure::DiscordNotRunning,
            // A timeout on a command that waits for a click means the click never came.
            DiscordError::CommandTimedOut => ConnectFailure::NeedsUserAction(
                "the authorisation modal was not accepted".to_owned(),
            ),
            err @ (DiscordError::OAuth { .. }
            | DiscordError::AuthorizationFailed
            | DiscordError::AuthenticationFailed) => {
                ConnectFailure::NeedsUserAction(err.to_string())
            }
            DiscordError::Rpc(message) => ConnectFailure::NeedsUserAction(message),
            other => ConnectFailure::Other(other.to_string()),
        }
    }
}

/// Everything a successful connection produces.
#[derive(Clone)]
pub struct Connection {
    client: IpcClient,
    user: Option<DiscordUser>,
    tokens: (Option<String>, Option<String>),
    voice_settings: DiscordVoiceSettings,
}

/// Runs the connect, handshake, authorise, authenticate and subscribe sequence.
///
/// A free function rather than a method so it cannot touch actor state while running off the
/// actor: everything it learns comes back in [`Connection`].
async fn connect(
    credentials: DiscordCredentials,
    events: mpsc::UnboundedSender<IpcEvent>,
    observer: mpsc::UnboundedSender<DiscordWorkerEvent>,
) -> Result<Connection, ConnectFailure> {
    let mut client = IpcClient::new(events);

    client.connect().await?;
    client.handshake(&credentials.client_id).await?;

    let (token, tokens) = obtain_token(&mut client, &credentials, &observer).await?;
    let user = client.authenticate(&token).await?;

    // Subscribe before reading the current value: the other order leaves a window where a change
    // is neither in the snapshot nor delivered as an event.
    client.subscribe_voice_settings().await?;

    let (mute, deafen) = client.get_voice_settings().await?;

    Ok::<Connection, DiscordError>(Connection {
        client,
        user,
        tokens,
        voice_settings: DiscordVoiceSettings { mute, deafen },
    })
    .map_err(ConnectFailure::from)
}

/// Produces a usable access token, reusing or refreshing what is stored before falling back to
/// prompting the user with Discord's authorisation modal.
///
/// Returns the token to authenticate with plus the pair to persist, which differs from what was
/// passed in when a refresh happened.
async fn obtain_token(
    client: &mut IpcClient,
    credentials: &DiscordCredentials,
    observer: &mpsc::UnboundedSender<DiscordWorkerEvent>,
) -> Result<(String, (Option<String>, Option<String>)), DiscordError> {
    if let Some(access_token) = credentials.access_token.clone() {
        let refresh = credentials.refresh_token.clone();
        return Ok((access_token.clone(), (Some(access_token), refresh)));
    }

    if let Some(refresh_token) = credentials.refresh_token.clone() {
        match oauth::refresh_access_token(
            &credentials.client_id,
            &credentials.client_secret,
            &credentials.redirect_url,
            &refresh_token,
        )
        .await
        {
            Ok(tokens) => {
                return Ok((
                    tokens.access_token.clone(),
                    (Some(tokens.access_token), Some(tokens.refresh_token)),
                ));
            }
            Err(err) => warn!("Could not refresh the access token, asking the user again: {err}"),
        }
    }

    // Everything past this point waits on the user, so say so before blocking.
    info!("Waiting for the authorisation to be accepted in Discord");
    let _ = observer.send(DiscordWorkerEvent::AwaitingAuthorization);

    let code = client.authorize(DISCORD_SCOPES).await?;
    let tokens = oauth::exchange_code(
        &credentials.client_id,
        &credentials.client_secret,
        &credentials.redirect_url,
        &code,
    )
    .await?;

    Ok((
        tokens.access_token.clone(),
        (Some(tokens.access_token), Some(tokens.refresh_token)),
    ))
}

impl GenServer for DiscordState {
    type CallMsg = InCallMessage;
    type CastMsg = InMessage;
    type OutMsg = OutMessage;
    type Error = DiscordError;

    async fn handle_cast(
        mut self,
        message: Self::CastMsg,
        handle: &GenServerHandle<Self>,
    ) -> CastResponse<Self> {
        if self.shutdown {
            return CastResponse::NoReply(self);
        }

        match message {
            InMessage::Connect => {
                self.reconnect_scheduled = false;

                if self.credentials.is_none() || self.ipc_client.is_connected() {
                    return CastResponse::NoReply(self);
                }
                self.begin_connecting(handle);
                CastResponse::NoReply(self)
            }

            InMessage::ConnectFinished(outcome) => {
                self.connecting = false;

                match *outcome {
                    Ok(connection) => {
                        self.ipc_client = connection.client;
                        let (access, refresh) = connection.tokens;
                        self.set_tokens(access, refresh);
                        // Reset the backoff so a later drop retries promptly.
                        self.reconnect_backoff = RECONNECT_BACKOFF_MIN;
                        self.announce_running(true);
                        self.announce_connection();
                        let _ = self
                            .observer
                            .send(DiscordWorkerEvent::UserChanged(connection.user));
                        self.update_voice_settings(connection.voice_settings).await;
                    }
                    Err(failure) => {
                        self.ipc_client.disconnect().await;
                        // Any failure past opening the pipe means Discord is there.
                        self.announce_running(!matches!(
                            failure,
                            ConnectFailure::DiscordNotRunning
                        ));
                        self.announce_connection();

                        match failure {
                            // The common case: Discord is closed. It will open eventually, so
                            // keep trying.
                            ConnectFailure::DiscordNotRunning => {
                                debug!("Discord is not running, will retry");
                                self.schedule_reconnect(handle);
                            }
                            ConnectFailure::Other(reason) => {
                                warn!("Could not connect to Discord: {reason}");
                                self.schedule_reconnect(handle);
                            }
                            // Nothing will change until the user does something, so stop. The
                            // next attempt comes from them pressing Connect, which arrives as
                            // SetCredentials.
                            ConnectFailure::NeedsUserAction(reason) => {
                                warn!("Discord authorisation needs your attention: {reason}");
                            }
                        }
                    }
                }
                CastResponse::NoReply(self)
            }

            InMessage::Ipc(IpcEvent::VoiceSettings { mute, deafen }) => {
                debug!("Voice settings pushed by Discord — mute: {mute} deafen: {deafen}");
                self.update_voice_settings(DiscordVoiceSettings { mute, deafen })
                    .await;
                CastResponse::NoReply(self)
            }

            InMessage::Ipc(IpcEvent::Disconnected) => {
                self.ipc_client.disconnect().await;
                self.announce_connection();
                self.schedule_reconnect(handle);
                CastResponse::NoReply(self)
            }

            InMessage::SetCredentials(credentials) => {
                let credentials = *credentials;
                let changed = self.credentials != credentials;

                // Already connected with exactly these credentials: nothing to do.
                if !changed && self.ipc_client.is_connected() {
                    return CastResponse::NoReply(self);
                }

                if changed {
                    // Any live session belongs to the previous application.
                    self.ipc_client.disconnect().await;
                    self.announce_connection();
                    self.credentials = credentials;
                }
                self.reconnect_backoff = RECONNECT_BACKOFF_MIN;

                if self.credentials.is_some() {
                    // This message is also how the user retries after an authorisation failure,
                    // which is the only thing that restarts a connection parked by
                    // `NeedsUserAction`. Resending unchanged credentials therefore has to mean
                    // "try again", not "nothing changed".
                    info!("Discord credentials set, connecting");
                    self.begin_connecting(handle);
                } else {
                    info!("Discord credentials cleared, connection stopped");
                }
                CastResponse::NoReply(self)
            }

            InMessage::SetVoiceSetting(mute, deafen) => {
                if let Err(err) = self.ipc_client.set_voice_settings(mute, deafen).await {
                    warn!("Could not set voice settings: {err}");
                    self.ipc_client.disconnect().await;
                    self.announce_connection();
                    self.schedule_reconnect(handle);
                }
                CastResponse::NoReply(self)
            }

            InMessage::DisconnectChannel => {
                if let Err(err) = self.ipc_client.select_voice_channel(None).await {
                    warn!("Could not leave the voice channel: {err}");
                }
                CastResponse::NoReply(self)
            }
        }
    }

    async fn handle_call(
        mut self,
        message: Self::CallMsg,
        _handle: &GenServerHandle<Self>,
    ) -> CallResponse<Self> {
        match message {
            InCallMessage::DiscordStatus => {
                let state = self.ipc_client.state.clone();
                CallResponse::Reply(self, OutMessage::DiscordStatus(state))
            }
            InCallMessage::VoiceSettings => {
                let settings = *self.voice_settings.lock().await;
                CallResponse::Reply(self, OutMessage::VoiceSettings(settings))
            }
            InCallMessage::AccessToken => {
                let token = self
                    .credentials
                    .as_ref()
                    .and_then(|c| c.access_token.clone());
                CallResponse::Reply(self, OutMessage::AccessToken(token))
            }
            InCallMessage::RefreshToken => {
                let token = self
                    .credentials
                    .as_ref()
                    .and_then(|c| c.refresh_token.clone());
                CallResponse::Reply(self, OutMessage::RefreshToken(token))
            }
            InCallMessage::Shutdown => {
                self.shutdown = true;
                self.ipc_client.disconnect().await;
                CallResponse::Reply(self, OutMessage::Done)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (DiscordState, mpsc::UnboundedReceiver<DiscordWorkerEvent>) {
        let (ipc_tx, _ipc_rx) = mpsc::unbounded_channel();
        let (observer, observed) = mpsc::unbounded_channel();
        (
            DiscordState {
                ipc_client: IpcClient::new(ipc_tx.clone()),
                events: ipc_tx,
                connected: Arc::new(AtomicBool::new(false)),
                connecting: false,
                credentials: None,
                voice_settings: Arc::new(Mutex::new(DiscordVoiceSettings::default())),
                observer,
                announced_connected: false,
                announced_running: None,
                reconnect_backoff: RECONNECT_BACKOFF_MIN,
                reconnect_scheduled: false,
                shutdown: false,
            },
            observed,
        )
    }

    #[tokio::test]
    async fn a_voice_change_is_announced_once_and_repeats_are_swallowed() {
        // The coordinator treats every announcement as news and acts on it — pushing to the
        // device and emitting to the webview — so a duplicate here becomes wasted work there.
        let (mut state, mut observed) = state();
        let muted = DiscordVoiceSettings {
            mute: true,
            deafen: false,
        };

        state.update_voice_settings(muted).await;
        assert_eq!(
            observed.try_recv().expect("an announcement"),
            DiscordWorkerEvent::VoiceSettingsChanged(muted)
        );

        state.update_voice_settings(muted).await;
        assert!(
            observed.try_recv().is_err(),
            "the same value must not be announced twice"
        );

        assert_eq!(*state.voice_settings.lock().await, muted);
    }

    #[tokio::test]
    async fn whether_discord_runs_is_announced_once_per_change() {
        let (mut state, mut observed) = state();

        state.announce_running(false);
        assert_eq!(
            observed.try_recv().expect("an announcement"),
            DiscordWorkerEvent::RunningChanged { running: false }
        );

        // Every retry while Discord stays closed says the same thing: nothing new to announce.
        state.announce_running(false);
        assert!(observed.try_recv().is_err());

        state.announce_running(true);
        assert_eq!(
            observed.try_recv().expect("an announcement"),
            DiscordWorkerEvent::RunningChanged { running: true }
        );
    }

    #[tokio::test]
    async fn a_connection_change_is_announced_once_per_transition() {
        let (mut state, mut observed) = state();

        // Still disconnected, so there is nothing to announce.
        state.announce_connection();
        assert!(observed.try_recv().is_err());

        state.ipc_client.state = DiscordConnectionState::Authenticated;
        state.announce_connection();
        assert_eq!(
            observed.try_recv().expect("an announcement"),
            DiscordWorkerEvent::ConnectionChanged { connected: true }
        );

        state.announce_connection();
        assert!(observed.try_recv().is_err());

        state.ipc_client.state = DiscordConnectionState::NotConnected;
        state.announce_connection();
        assert_eq!(
            observed.try_recv().expect("an announcement"),
            DiscordWorkerEvent::ConnectionChanged { connected: false }
        );
    }

    #[tokio::test]
    async fn only_full_authentication_counts_as_connected() {
        // A half-open connection must not light the indicator: the pipe being open says nothing
        // about whether the RPC session is usable.
        let (mut state, mut observed) = state();

        for intermediate in [
            DiscordConnectionState::Connected,
            DiscordConnectionState::HandshakeDone,
            DiscordConnectionState::Authorized,
        ] {
            state.ipc_client.state = intermediate;
            state.announce_connection();
            assert!(observed.try_recv().is_err());
        }
    }
}
