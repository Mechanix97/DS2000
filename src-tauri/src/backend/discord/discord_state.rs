//! Connection state machine for Discord RPC.
//!
//! Nothing here runs on a timer while the connection is healthy. Voice state arrives as
//! `VOICE_SETTINGS_UPDATE` events pushed by Discord, and the only scheduled work is the
//! reconnect attempt, which backs off when Discord is not running.

use crate::credentials::{DISCORD_SCOPES, DiscordCredentials};
use crate::error::DiscordError;
use crate::ipc::{DiscordConnectionState, DiscordUser, IpcClient, IpcEvent};
use crate::oauth;
use crate::voice_devices::{DeviceKind, VoiceDevices};
use spawned_concurrency::message::Message;
use spawned_concurrency::tasks::{Actor, ActorRef, ActorStart, Context, Handler, send_after};
use spawned_concurrency::{ActorError, Response, actor, protocol};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc};
use tokio::time::Duration;
use tracing::{debug, info, warn};

/// First delay after a failed connection attempt.
const RECONNECT_BACKOFF_MIN: Duration = Duration::from_millis(500);
/// Ceiling for the backoff. Discord being closed for an hour must not mean an hour of retries.
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);

use discord_state_protocol::{
    AccessToken, Connect, DisconnectChannel, DiscordStatus, RefreshToken, SetCredentials,
    SetVoiceDevice, SetVoiceSetting, Shutdown, VoiceDevices as GetVoiceDevices, VoiceSettings,
};

pub type DiscordStateHandler = ActorRef<DiscordState>;

/// What the Discord actor accepts from the rest of the application.
#[protocol]
pub trait DiscordStateProtocol: Send + Sync {
    /// Attempts to bring the connection up. Rescheduled with backoff only while it fails.
    fn connect(&self) -> Result<(), ActorError>;
    fn set_voice_setting(&self, mute: bool, deafen: bool) -> Result<(), ActorError>;
    fn disconnect_channel(&self) -> Result<(), ActorError>;
    /// Applies credentials entered at runtime, so connecting does not require a restart.
    fn set_credentials(&self, credentials: Option<DiscordCredentials>) -> Result<(), ActorError>;
    fn discord_status(&self) -> Response<DiscordConnectionState>;
    fn access_token(&self) -> Response<Option<String>>;
    fn refresh_token(&self) -> Response<Option<String>>;
    fn voice_settings(&self) -> Response<DiscordVoiceSettings>;
    /// Discord's audio devices, read from Discord on each call.
    fn voice_devices(&self) -> Response<Result<VoiceDevices, DiscordError>>;
    fn set_voice_device(
        &self,
        kind: DeviceKind,
        device_id: String,
    ) -> Response<Result<(), DiscordError>>;
    fn shutdown(&self) -> Response<()>;
}

/// Result of an attempt that ran off the actor. Internal, so not part of the protocol.
struct ConnectFinished(Result<Connection, ConnectFailure>);

impl Message for ConnectFinished {
    type Result = ();
}

/// Something the reader task observed. Internal, so not part of the protocol.
struct Ipc(IpcEvent);

impl Message for Ipc {
    type Result = ();
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
        let forward_to = handle.clone();
        tokio::spawn(async move {
            while let Some(event) = events_rx.recv().await {
                if forward_to.send(Ipc(event)).is_err() {
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
    fn begin_connecting(&mut self, ctx: &Context<Self>) {
        if self.connecting || self.shutdown {
            return;
        }
        let Some(credentials) = self.credentials.clone() else {
            return;
        };

        self.connecting = true;
        let events = self.events.clone();
        let observer = self.observer.clone();
        let ctx = ctx.clone();

        tokio::spawn(async move {
            let outcome = connect(credentials, events, observer).await;
            let _ = ctx.send(ConnectFinished(outcome));
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
    fn schedule_reconnect(&mut self, ctx: &Context<Self>) {
        if self.shutdown || self.credentials.is_none() || self.reconnect_scheduled {
            return;
        }
        self.reconnect_scheduled = true;
        send_after(self.reconnect_backoff, ctx.clone(), Connect);
        self.reconnect_backoff = (self.reconnect_backoff * 2).min(RECONNECT_BACKOFF_MAX);
    }
}

/// Why a connection attempt did not succeed.
///
/// A summary rather than the original error: the actor only needs to tell "Discord is closed"
/// apart from what needs the user and from anything else, and the detail is already logged where
/// it happened.
#[derive(Debug)]
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
    let mut credentials = credentials;

    // At most two rounds: the second runs without the stored access token, so it cannot loop.
    let (client, user, tokens) = loop {
        let mut client = IpcClient::new(events.clone());

        client.connect().await?;
        client.handshake(&credentials.client_id).await?;

        let reused_stored_token = credentials.access_token.is_some();
        let (token, tokens) = obtain_token(&mut client, &credentials, &observer).await?;

        match client.authenticate(&token).await {
            Ok(user) => break (client, user, tokens),
            Err(err) if should_retry_without_stored_token(reused_stored_token, &err) => {
                // Access tokens last a week and the stored one is reused without knowing when it
                // was issued, so after a week Discord rejects it. Parking on that as "needs the
                // user" left the app silently disconnected until the credentials were re-entered.
                // Starting over on a fresh pipe also sidesteps whatever Discord does with a
                // session whose AUTHENTICATE failed.
                info!("Discord rejected the stored access token ({err}), trying the refresh token");
                client.disconnect().await;
                credentials.access_token = None;
            }
            Err(err) => return Err(err.into()),
        }
    };

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

/// Whether a failed `AUTHENTICATE` is worth one more attempt without the stored access token.
///
/// Only when the token came from storage, which is when it may simply have expired, and only when
/// Discord itself refused it. A pipe or timeout failure says nothing about the token, and a token
/// that was just refreshed or issued failing again would only repeat the failure.
fn should_retry_without_stored_token(reused_stored_token: bool, err: &DiscordError) -> bool {
    reused_stored_token
        && matches!(
            err,
            DiscordError::Rpc(_) | DiscordError::AuthenticationFailed
        )
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

#[actor(protocol = DiscordStateProtocol)]
impl DiscordState {
    #[send_handler]
    async fn handle_connect(&mut self, _msg: Connect, ctx: &Context<Self>) {
        self.reconnect_scheduled = false;

        if self.shutdown || self.credentials.is_none() || self.ipc_client.is_connected() {
            return;
        }
        self.begin_connecting(ctx);
    }

    #[send_handler]
    async fn handle_connect_finished(&mut self, msg: ConnectFinished, ctx: &Context<Self>) {
        if self.shutdown {
            return;
        }
        self.connecting = false;

        match msg.0 {
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
                self.announce_running(!matches!(failure, ConnectFailure::DiscordNotRunning));
                self.announce_connection();

                match failure {
                    // The common case: Discord is closed. It will open eventually, so keep
                    // trying.
                    ConnectFailure::DiscordNotRunning => {
                        debug!("Discord is not running, will retry");
                        self.schedule_reconnect(ctx);
                    }
                    ConnectFailure::Other(reason) => {
                        warn!("Could not connect to Discord: {reason}");
                        self.schedule_reconnect(ctx);
                    }
                    // Nothing will change until the user does something, so stop. The next
                    // attempt comes from them pressing Connect, which arrives as SetCredentials.
                    ConnectFailure::NeedsUserAction(reason) => {
                        warn!("Discord authorisation needs your attention: {reason}");
                    }
                }
            }
        }
    }

    #[send_handler]
    async fn handle_ipc(&mut self, msg: Ipc, ctx: &Context<Self>) {
        if self.shutdown {
            return;
        }

        match msg.0 {
            IpcEvent::VoiceSettings { mute, deafen } => {
                debug!("Voice settings pushed by Discord — mute: {mute} deafen: {deafen}");
                self.update_voice_settings(DiscordVoiceSettings { mute, deafen })
                    .await;
            }
            IpcEvent::Disconnected => {
                self.ipc_client.disconnect().await;
                self.announce_connection();
                self.schedule_reconnect(ctx);
            }
        }
    }

    #[send_handler]
    async fn handle_set_credentials(&mut self, msg: SetCredentials, ctx: &Context<Self>) {
        if self.shutdown {
            return;
        }

        let credentials = msg.credentials;
        let changed = self.credentials != credentials;

        // Already connected with exactly these credentials: nothing to do.
        if !changed && self.ipc_client.is_connected() {
            return;
        }

        if changed {
            // Any live session belongs to the previous application.
            self.ipc_client.disconnect().await;
            self.announce_connection();
            self.credentials = credentials;
        }
        self.reconnect_backoff = RECONNECT_BACKOFF_MIN;

        if self.credentials.is_some() {
            // This message is also how the user retries after an authorisation failure, which is
            // the only thing that restarts a connection parked by `NeedsUserAction`. Resending
            // unchanged credentials therefore has to mean "try again", not "nothing changed".
            info!("Discord credentials set, connecting");
            self.begin_connecting(ctx);
        } else {
            info!("Discord credentials cleared, connection stopped");
        }
    }

    #[send_handler]
    async fn handle_set_voice_setting(&mut self, msg: SetVoiceSetting, ctx: &Context<Self>) {
        if self.shutdown {
            return;
        }
        if let Err(err) = self
            .ipc_client
            .set_voice_settings(msg.mute, msg.deafen)
            .await
        {
            warn!("Could not set voice settings: {err}");
            self.ipc_client.disconnect().await;
            self.announce_connection();
            self.schedule_reconnect(ctx);
        }
    }

    #[send_handler]
    async fn handle_disconnect_channel(&mut self, _msg: DisconnectChannel, _ctx: &Context<Self>) {
        if self.shutdown {
            return;
        }
        if let Err(err) = self.ipc_client.select_voice_channel(None).await {
            warn!("Could not leave the voice channel: {err}");
        }
    }

    #[request_handler]
    async fn handle_discord_status(
        &mut self,
        _msg: DiscordStatus,
        _ctx: &Context<Self>,
    ) -> DiscordConnectionState {
        self.ipc_client.state.clone()
    }

    #[request_handler]
    async fn handle_voice_settings(
        &mut self,
        _msg: VoiceSettings,
        _ctx: &Context<Self>,
    ) -> DiscordVoiceSettings {
        *self.voice_settings.lock().await
    }

    #[request_handler]
    async fn handle_voice_devices(
        &mut self,
        _msg: GetVoiceDevices,
        _ctx: &Context<Self>,
    ) -> Result<VoiceDevices, DiscordError> {
        self.ipc_client.get_voice_devices().await
    }

    /// A refusal here is Discord rejecting the device, not the connection failing, so unlike
    /// [`Self::handle_set_voice_setting`] it does not reconnect. A pipe that did fail is reported by
    /// the reader task anyway.
    #[request_handler]
    async fn handle_set_voice_device(
        &mut self,
        msg: SetVoiceDevice,
        _ctx: &Context<Self>,
    ) -> Result<(), DiscordError> {
        self.ipc_client
            .set_voice_device(msg.kind, &msg.device_id)
            .await
    }

    #[request_handler]
    async fn handle_access_token(
        &mut self,
        _msg: AccessToken,
        _ctx: &Context<Self>,
    ) -> Option<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.clone())
    }

    #[request_handler]
    async fn handle_refresh_token(
        &mut self,
        _msg: RefreshToken,
        _ctx: &Context<Self>,
    ) -> Option<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.refresh_token.clone())
    }

    #[request_handler]
    async fn handle_shutdown(&mut self, _msg: Shutdown, _ctx: &Context<Self>) {
        self.shutdown = true;
        self.ipc_client.disconnect().await;
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

    #[test]
    fn a_rejected_stored_token_is_retried_without_it() {
        // What an expired token gets back from AUTHENTICATE.
        let rejected = DiscordError::Rpc("Invalid access token".to_owned());
        assert!(should_retry_without_stored_token(true, &rejected));
        assert!(should_retry_without_stored_token(
            true,
            &DiscordError::AuthenticationFailed
        ));
    }

    #[test]
    fn a_fresh_token_or_a_transport_failure_is_not_retried() {
        let rejected = DiscordError::Rpc("Invalid access token".to_owned());
        // Just refreshed or issued: retrying would only repeat the same rejection.
        assert!(!should_retry_without_stored_token(false, &rejected));
        // Nothing to do with the token, and the reconnect backoff already handles these.
        assert!(!should_retry_without_stored_token(
            true,
            &DiscordError::CommandTimedOut
        ));
        assert!(!should_retry_without_stored_token(
            true,
            &DiscordError::PipeErrorReading
        ));
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
