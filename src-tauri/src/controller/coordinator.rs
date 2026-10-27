//! Reacts to what the workers report.
//!
//! This replaces the old `background_loop`, which woke ten times a second to ask both workers
//! whether anything had happened and to re-emit the answer to the webview regardless. It now
//! awaits the workers' event channels: with nothing happening, this task is parked and costs
//! nothing.
//!
//! It also owns the mapping between the two sides — a button press on the device becomes a
//! Discord command, a Discord change becomes an LED update — which used to be scattered through
//! the loop body.

use crate::controller::Controller;
use crate::notifications::{DesktopNotifier, NoticeTracker, SystemNotifier};

use discord::discord_state::{DiscordVoiceSettings, DiscordWorkerEvent};
use discord::ipc::DiscordUser;
use serial::messages::button::Button;
use serial::serial_message::{PROTOCOL_VERSION, SerialMessage};
use serial::serial_state::{DeviceStatus, SerialWorkerEvent};

use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;
use tracing::{debug, info, warn};

const DISCORD_CONNECTION_STATUS_EVENT: &str = "DISCORD_CONNECTION_STATUS_EVENT";
const DISCORD_VOICE_SETTINGS_EVENT: &str = "DISCORD_VOICE_SETTINGS_EVENT";
const SERIAL_CONNECTION_STATUS_EVENT: &str = "SERIAL_CONNECTION_STATUS_EVENT";
const DISCORD_AWAITING_AUTHORIZATION_EVENT: &str = "DISCORD_AWAITING_AUTHORIZATION_EVENT";
const DEVICE_BUTTON_EVENT: &str = "DEVICE_BUTTON_EVENT";
const DISCORD_USER_EVENT: &str = "DISCORD_USER_EVENT";
const DISCORD_RUNNING_EVENT: &str = "DISCORD_RUNNING_EVENT";

/// Window the frontend runs in. Emitting while it is hidden is pointless work.
const MAIN_WINDOW: &str = "main";

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct VoiceSettingsPayload {
    pub mute: bool,
    pub deafen: bool,
}

impl From<DiscordVoiceSettings> for VoiceSettingsPayload {
    fn from(settings: DiscordVoiceSettings) -> Self {
        Self {
            mute: settings.mute,
            deafen: settings.deafen,
        }
    }
}

/// Which side needs updating is all the UI can act on, so that is what the status says rather
/// than the raw protocol numbers.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum SerialState {
    #[default]
    Disconnected,
    Connected,
    /// The device speaks an older protocol than this application.
    FirmwareOutdated,
    /// The device speaks a newer protocol than this application.
    AppOutdated,
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct SerialStatusPayload {
    pub state: SerialState,
    /// What the device reported in the handshake. `None` while nothing is attached.
    pub firmware: Option<String>,
    /// Board the firmware was built for, by name. `None` while nothing is attached.
    pub board: Option<String>,
}

impl From<DeviceStatus> for SerialStatusPayload {
    fn from(status: DeviceStatus) -> Self {
        let (state, device) = match status {
            DeviceStatus::Disconnected => return Self::default(),
            DeviceStatus::Connected(device) => (SerialState::Connected, device),
            DeviceStatus::Incompatible(device) if device.protocol < PROTOCOL_VERSION => {
                (SerialState::FirmwareOutdated, device)
            }
            DeviceStatus::Incompatible(device) => (SerialState::AppOutdated, device),
        };
        Self {
            state,
            firmware: Some(device.firmware.to_string()),
            board: Some(device.board.to_string()),
        }
    }
}

/// Everything the UI shows, plus whether it has been told about it.
///
/// The Discord account for the status panel. Sent as `null` until the first connection.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscordUserPayload {
    pub name: String,
    /// `None` for Discord's default avatar; the panel shows the DS2000 logo then.
    pub avatar_url: Option<String>,
}

impl From<&DiscordUser> for DiscordUserPayload {
    fn from(user: &DiscordUser) -> Self {
        Self {
            name: user.display_name().to_owned(),
            avatar_url: user.avatar_url(),
        }
    }
}

/// Which key was pressed on the device, as the frontend names them.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ButtonPayload {
    Mute,
    Deafen,
    Disconnect,
}

impl From<Button> for ButtonPayload {
    fn from(button: Button) -> Self {
        match button {
            Button::MuteButton => Self::Mute,
            Button::DeafenButton => Self::Deafen,
            Button::DisconnectButton => Self::Disconnect,
        }
    }
}

/// Keeping the last known values here is what lets the coordinator skip emitting while the
/// window is hidden and then bring it up to date in one go when it is shown again.
#[derive(Default)]
struct UiState {
    discord_connected: bool,
    /// Kept across disconnections, so the panel still says whose Discord is closed.
    discord_user: Option<DiscordUserPayload>,
    /// Whether Discord is running; `None` until the worker has tried to reach it.
    discord_running: Option<bool>,
    serial_status: SerialStatusPayload,
    voice_settings: VoiceSettingsPayload,
}

/// Asks the coordinator to resend everything the UI shows.
///
/// Held in Tauri state so the tray and window handlers can trigger it when the window becomes
/// visible again, since nothing was emitted while it was hidden.
pub type UiRefreshHandle = mpsc::UnboundedSender<()>;

pub struct Coordinator {
    app: AppHandle,
    controller: Arc<Mutex<Controller>>,
    ui: UiState,
    /// Mirror of what the hardware and Discord should agree on.
    ///
    /// Held here rather than read back from Discord each time, because a button press has to
    /// toggle from the last known value even while the connection is down.
    voice_settings: DiscordVoiceSettings,
    /// What is worth a desktop notification, shown while the window is not on screen.
    notices: NoticeTracker,
    notifier: Box<dyn DesktopNotifier>,
}

impl Coordinator {
    pub fn new(app: AppHandle, controller: Arc<Mutex<Controller>>) -> Self {
        Self {
            notifier: Box::new(SystemNotifier(app.clone())),
            app,
            controller,
            ui: UiState::default(),
            voice_settings: DiscordVoiceSettings::default(),
            notices: NoticeTracker::default(),
        }
    }

    /// Awaits the workers until the application shuts down.
    pub async fn run(
        mut self,
        mut discord_events: mpsc::UnboundedReceiver<DiscordWorkerEvent>,
        mut serial_events: mpsc::UnboundedReceiver<SerialWorkerEvent>,
        mut ui_refresh: mpsc::UnboundedReceiver<()>,
    ) {
        debug!("Coordinator started");

        // Seed from the worker so the UI is not left showing defaults until something changes.
        self.voice_settings = self
            .controller
            .lock()
            .await
            .discord_worker
            .get_voice_settings()
            .await;
        self.ui.voice_settings = self.voice_settings.into();
        self.refresh_ui();

        loop {
            let next_notice = self.notices.next_due();
            tokio::select! {
                event = discord_events.recv() => match event {
                    Some(event) => self.on_discord_event(event).await,
                    None => break,
                },
                event = serial_events.recv() => match event {
                    Some(event) => self.on_serial_event(event).await,
                    None => break,
                },
                request = ui_refresh.recv() => match request {
                    Some(()) => self.refresh_ui(),
                    None => break,
                },
                () = tokio::time::sleep_until(next_notice.unwrap_or_else(Instant::now)),
                    if next_notice.is_some() => self.show_due_notices().await,
            }
        }

        debug!("Coordinator stopped: its event sources are gone");
    }

    async fn on_discord_event(&mut self, event: DiscordWorkerEvent) {
        match event {
            DiscordWorkerEvent::VoiceSettingsChanged(settings) => {
                debug!("Discord voice state changed: {settings:?}");
                self.voice_settings = settings;
                self.push_to_device().await;
                self.emit_voice_settings();
            }
            DiscordWorkerEvent::AwaitingAuthorization => {
                // Emitted even while the window is hidden would be pointless, but this is the one
                // case where the user has to be told to go and look at Discord.
                debug!("Waiting for the user to accept Discord's authorisation modal");
                self.notices.on_awaiting_authorization(Instant::now());
                if let Err(err) = self.app.emit(DISCORD_AWAITING_AUTHORIZATION_EVENT, ()) {
                    debug!("Could not emit the authorisation notice: {err}");
                }
            }
            DiscordWorkerEvent::UserChanged(user) => {
                // A reply without a readable user keeps the last known one.
                if let Some(user) = user {
                    self.ui.discord_user = Some(DiscordUserPayload::from(&user));
                    self.emit(DISCORD_USER_EVENT, self.ui.discord_user.clone());
                    if let Some(running) = self.ui.discord_running {
                        self.emit(DISCORD_RUNNING_EVENT, running);
                    }
                }
            }
            DiscordWorkerEvent::RunningChanged { running } => {
                debug!("Discord running: {running}");
                self.ui.discord_running = Some(running);
                self.notices.on_discord_running(running);
                self.emit(DISCORD_RUNNING_EVENT, running);
            }
            DiscordWorkerEvent::ConnectionChanged { connected } => {
                debug!("Discord connection changed: connected={connected}");
                self.ui.discord_connected = connected;
                self.notices
                    .on_discord_connection(connected, Instant::now());
                self.emit(DISCORD_CONNECTION_STATUS_EVENT, connected);

                if connected {
                    // Persist the tokens the handshake just produced, so the next launch skips
                    // the authorisation modal.
                    self.persist_tokens().await;
                }
            }
        }
    }

    async fn on_serial_event(&mut self, event: SerialWorkerEvent) {
        match event {
            SerialWorkerEvent::Message(SerialMessage::Button(message)) => {
                self.on_button(message.button).await;
            }
            SerialWorkerEvent::Message(other) => {
                debug!("Unhandled serial message: {other:?}");
            }
            SerialWorkerEvent::StatusChanged(status) => {
                debug!("Serial device status changed: {status:?}");
                self.ui.serial_status = status.into();
                self.notices
                    .on_device(self.ui.serial_status.state, Instant::now());
                self.emit(
                    SERIAL_CONNECTION_STATUS_EVENT,
                    self.ui.serial_status.clone(),
                );

                if matches!(status, DeviceStatus::Connected(_)) {
                    // A freshly connected device knows nothing, so give it the current state and
                    // the stored lighting configuration.
                    self.push_to_device().await;
                    self.push_rgb_config().await;
                    self.persist_port_name().await;
                }
            }
        }
    }

    async fn on_button(&mut self, button: Button) {
        // Lights the key up in the RGB tab, so the device can be tested without Discord.
        self.emit(DEVICE_BUTTON_EVENT, ButtonPayload::from(button));

        match button {
            Button::MuteButton => {
                self.voice_settings.mute = !self.voice_settings.mute;
                self.request_voice_settings().await;
            }
            Button::DeafenButton => {
                self.voice_settings.deafen = !self.voice_settings.deafen;
                self.request_voice_settings().await;
            }
            Button::DisconnectButton => {
                let mut controller = self.controller.lock().await;
                if let Err(err) = controller.discord_worker.disconnect().await {
                    warn!("Could not leave the voice channel: {err}");
                }
            }
        }
    }

    /// Asks Discord to apply the current state.
    ///
    /// The device is updated optimistically so the LEDs follow the button immediately; Discord's
    /// own `VOICE_SETTINGS_UPDATE` will confirm or correct it.
    async fn request_voice_settings(&mut self) {
        let (mute, deafen) = (self.voice_settings.mute, self.voice_settings.deafen);
        let mut controller = self.controller.lock().await;

        // Deafening implies muting, mirroring what Discord itself enforces.
        if let Err(err) = controller
            .discord_worker
            .set_voice_settings(mute || deafen, deafen)
            .await
        {
            warn!("Could not set voice settings: {err}");
        }
        drop(controller);

        self.push_to_device().await;
        self.emit_voice_settings();
    }

    async fn push_to_device(&mut self) {
        let mut controller = self.controller.lock().await;
        if let Err(err) = controller
            .serial_worker
            .set_voice_settings(self.voice_settings.mute, self.voice_settings.deafen)
            .await
        {
            warn!("Could not send voice settings to the device: {err}");
        }
    }

    async fn push_rgb_config(&mut self) {
        let mut controller = self.controller.lock().await;
        let config = controller.config.rgb_config().await;
        if let Err(err) = controller.serial_worker.set_rgb_config(&config).await {
            warn!("Could not send the lighting configuration to the device: {err}");
        }
    }

    async fn persist_tokens(&mut self) {
        let mut controller = self.controller.lock().await;
        let access_token = controller
            .discord_worker
            .get_access_token()
            .await
            .ok()
            .flatten();
        let refresh_token = controller
            .discord_worker
            .get_refresh_token()
            .await
            .ok()
            .flatten();

        if let Err(err) = controller
            .config
            .update_tokens(access_token, refresh_token)
            .await
        {
            warn!("Could not persist the Discord tokens: {err}");
        }
    }

    async fn persist_port_name(&mut self) {
        let mut controller = self.controller.lock().await;
        let port_name = controller
            .serial_worker
            .get_port_name()
            .await
            .ok()
            .flatten();
        controller.config.update_last_used_port(port_name).await;
    }

    fn emit_voice_settings(&mut self) {
        let payload = VoiceSettingsPayload::from(self.voice_settings);
        self.ui.voice_settings = payload;
        self.emit(DISCORD_VOICE_SETTINGS_EVENT, payload);
    }

    /// Sends everything the UI shows. Used when the window becomes visible again.
    fn refresh_ui(&self) {
        self.emit(DISCORD_CONNECTION_STATUS_EVENT, self.ui.discord_connected);
        self.emit(DISCORD_USER_EVENT, self.ui.discord_user.clone());
        self.emit(
            SERIAL_CONNECTION_STATUS_EVENT,
            self.ui.serial_status.clone(),
        );
        self.emit(DISCORD_VOICE_SETTINGS_EVENT, self.ui.voice_settings);
    }

    /// Emits only when the window is actually visible.
    ///
    /// Every emit wakes the WebView2 process to run JavaScript. While the app sits in the tray
    /// there is nothing to update, and the frontend is resynchronised on show, so skipping is
    /// free.
    fn emit<T: Serialize + Clone>(&self, event: &str, payload: T) {
        if !self.window_is_visible() {
            return;
        }
        if let Err(err) = self.app.emit(event, payload) {
            debug!("Could not emit {event}: {err}");
        }
    }

    /// Shows the notifications that are due, unless the window is on screen, where the sidebar
    /// already says the same, or the user turned them off.
    async fn show_due_notices(&mut self) {
        let due = self.notices.take_due(Instant::now());
        if due.is_empty() {
            return;
        }
        if self.window_is_on_screen() {
            debug!("Not notifying {due:?}: the window is on screen");
            return;
        }
        let (enabled, language) = {
            let controller = self.controller.lock().await;
            (
                controller.config.desktop_notifications().await,
                controller.config.language().await,
            )
        };
        if !enabled {
            debug!("Not notifying {due:?}: desktop notifications are off");
            return;
        }
        for notice in due {
            let (title, body) = notice.text(language);
            match self.notifier.show(title, body) {
                Ok(()) => {
                    info!("Desktop notification: {notice:?}");
                    self.notices.shown(notice);
                }
                Err(err) => warn!("Could not show the {notice:?} notification: {err}"),
            }
        }
    }

    /// Visible and not minimised. A minimised window counts as visible to `emit`, which keeps it
    /// up to date, but nobody is looking at it.
    fn window_is_on_screen(&self) -> bool {
        self.app
            .get_webview_window(MAIN_WINDOW)
            .is_some_and(|window| {
                window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
            })
    }

    fn window_is_visible(&self) -> bool {
        self.app
            .get_webview_window(MAIN_WINDOW)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial::messages::device_info::{Board, DeviceInfoMessage, FirmwareVersion};

    fn device(protocol: u8) -> DeviceInfoMessage {
        DeviceInfoMessage {
            protocol,
            board: Board::Rp2350Zero,
            firmware: FirmwareVersion {
                major: 0,
                minor: 2,
                patch: 1,
            },
        }
    }

    #[test]
    fn a_connected_device_reports_its_firmware_version() {
        let payload = SerialStatusPayload::from(DeviceStatus::Connected(device(PROTOCOL_VERSION)));

        assert_eq!(payload.state, SerialState::Connected);
        assert_eq!(payload.firmware.as_deref(), Some("0.2.1"));
        assert_eq!(payload.board.as_deref(), Some("RP2350-Zero"));
    }

    #[test]
    fn a_mismatch_says_which_side_to_update() {
        let firmware_outdated =
            SerialStatusPayload::from(DeviceStatus::Incompatible(device(PROTOCOL_VERSION - 1)));
        assert_eq!(firmware_outdated.state, SerialState::FirmwareOutdated);
        assert_eq!(firmware_outdated.firmware.as_deref(), Some("0.2.1"));

        let app_outdated =
            SerialStatusPayload::from(DeviceStatus::Incompatible(device(PROTOCOL_VERSION + 1)));
        assert_eq!(app_outdated.state, SerialState::AppOutdated);
    }

    #[test]
    fn the_status_reaches_the_frontend_in_camel_case() {
        let payload = SerialStatusPayload::from(DeviceStatus::Incompatible(device(0)));

        assert_eq!(
            serde_json::to_value(payload).expect("serialises"),
            serde_json::json!({
                "state": "firmwareOutdated",
                "firmware": "0.2.1",
                "board": "RP2350-Zero",
            })
        );
    }

    #[test]
    fn device_keys_reach_the_frontend_by_name() {
        let names: Vec<_> = [
            Button::MuteButton,
            Button::DeafenButton,
            Button::DisconnectButton,
        ]
        .into_iter()
        .map(|button| serde_json::to_value(ButtonPayload::from(button)).expect("serialises"))
        .collect();

        assert_eq!(names, ["mute", "deafen", "disconnect"]);
    }
}
