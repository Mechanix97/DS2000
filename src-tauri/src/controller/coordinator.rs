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

use discord::discord_state::{DiscordVoiceSettings, DiscordWorkerEvent};
use serial::messages::button::Button;
use serial::serial_message::{PROTOCOL_VERSION, SerialMessage};
use serial::serial_state::{DeviceStatus, SerialWorkerEvent};

use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, warn};

const DISCORD_CONNECTION_STATUS_EVENT: &str = "DISCORD_CONNECTION_STATUS_EVENT";
const DISCORD_VOICE_SETTINGS_EVENT: &str = "DISCORD_VOICE_SETTINGS_EVENT";
const SERIAL_CONNECTION_STATUS_EVENT: &str = "SERIAL_CONNECTION_STATUS_EVENT";
const DISCORD_AWAITING_AUTHORIZATION_EVENT: &str = "DISCORD_AWAITING_AUTHORIZATION_EVENT";

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
    /// `None` while nothing is attached, and for firmware that predates versioning.
    pub firmware: Option<String>,
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
            firmware: device.firmware.map(|version| version.to_string()),
        }
    }
}

/// Everything the UI shows, plus whether it has been told about it.
///
/// Keeping the last known values here is what lets the coordinator skip emitting while the
/// window is hidden and then bring it up to date in one go when it is shown again.
#[derive(Default)]
struct UiState {
    discord_connected: bool,
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
}

impl Coordinator {
    pub fn new(app: AppHandle, controller: Arc<Mutex<Controller>>) -> Self {
        Self {
            app,
            controller,
            ui: UiState::default(),
            voice_settings: DiscordVoiceSettings::default(),
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
                if let Err(err) = self.app.emit(DISCORD_AWAITING_AUTHORIZATION_EVENT, ()) {
                    debug!("Could not emit the authorisation notice: {err}");
                }
            }
            DiscordWorkerEvent::ConnectionChanged { connected } => {
                debug!("Discord connection changed: connected={connected}");
                self.ui.discord_connected = connected;
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
    use serial::messages::device_info::{DeviceVersion, FirmwareVersion, UNVERSIONED_PROTOCOL};

    fn device(protocol: u8) -> DeviceVersion {
        DeviceVersion {
            protocol,
            firmware: Some(FirmwareVersion {
                major: 0,
                minor: 2,
                patch: 1,
            }),
        }
    }

    #[test]
    fn a_connected_device_reports_its_firmware_version() {
        let payload = SerialStatusPayload::from(DeviceStatus::Connected(device(PROTOCOL_VERSION)));

        assert_eq!(payload.state, SerialState::Connected);
        assert_eq!(payload.firmware.as_deref(), Some("0.2.1"));
    }

    #[test]
    fn a_mismatch_says_which_side_to_update() {
        let older = DeviceVersion {
            protocol: UNVERSIONED_PROTOCOL,
            firmware: None,
        };
        let firmware_outdated = SerialStatusPayload::from(DeviceStatus::Incompatible(older));
        assert_eq!(firmware_outdated.state, SerialState::FirmwareOutdated);
        assert_eq!(firmware_outdated.firmware, None);

        let app_outdated =
            SerialStatusPayload::from(DeviceStatus::Incompatible(device(PROTOCOL_VERSION + 1)));
        assert_eq!(app_outdated.state, SerialState::AppOutdated);
    }

    #[test]
    fn the_status_reaches_the_frontend_in_camel_case() {
        let payload = SerialStatusPayload::from(DeviceStatus::Incompatible(device(0)));

        assert_eq!(
            serde_json::to_value(payload).expect("serialises"),
            serde_json::json!({ "state": "firmwareOutdated", "firmware": "0.2.1" })
        );
    }
}
