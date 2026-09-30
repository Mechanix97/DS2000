use crate::controller::Controller;
use crate::coordinator::UiRefreshHandle;
use crate::tray::{TRAY_ID, tray_menu};
use common::rgb_update::{LedRgb, RGBConfig, RGBMode};
use config::credentials::URL_DISCORD_SETUP_GUIDE;
use config::language::Language;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// What the Discord tab needs to render itself.
///
/// The client secret is deliberately absent: once stored it is never handed back to the
/// frontend, only replaced.
#[derive(Serialize)]
pub struct DiscordCredentialsStatus {
    pub client_id: Option<String>,
    pub has_client_secret: bool,
    pub connected: bool,
    pub setup_guide_url: &'static str,
    pub redirect_uri: &'static str,
}

#[tauri::command]
pub async fn ds_set_voice_settings_command(
    mute: bool,
    deaf: bool,
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<(), String> {
    debug!("ds_set_voice_settings_command");
    // Deafening implies muting: Discord will not accept a deafened-but-unmuted state.
    controller
        .lock()
        .await
        .ds_set_voice_settings(mute || deaf, deaf)
        .await
        .map_err(|err| err.to_string())
}

/// Reports whether a Discord application is configured, so the UI can decide whether to open on
/// the Discord tab and what to show there.
#[tauri::command]
pub async fn discord_credentials_status(
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<DiscordCredentialsStatus, String> {
    let controller = controller.lock().await;

    let client_id = controller.config.discord_client_id().await;
    let has_client_secret = controller.has_discord_credentials().await;
    let connected = controller.discord_worker.is_connected();

    Ok(DiscordCredentialsStatus {
        client_id,
        has_client_secret,
        connected,
        setup_guide_url: URL_DISCORD_SETUP_GUIDE,
        redirect_uri: config::credentials::DISCORD_REDIRECT_URI,
    })
}

/// Stores the Discord application credentials entered by the user and reconnects.
#[tauri::command]
pub async fn discord_set_credentials(
    client_id: String,
    client_secret: String,
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<(), String> {
    let client_id = client_id.trim();
    let client_secret = client_secret.trim();

    if client_id.is_empty() || client_secret.is_empty() {
        return Err("Both the Client ID and the Client Secret are required".to_owned());
    }
    if !client_id.chars().all(|c| c.is_ascii_digit()) {
        return Err("The Client ID should be numeric — copy it from OAuth2 → Client ID".to_owned());
    }

    controller
        .lock()
        .await
        .set_discord_credentials(client_id, client_secret)
        .await
        .map_err(|err| err.to_string())
}

/// Forgets the configured Discord application.
#[tauri::command]
pub async fn discord_clear_credentials(
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<(), String> {
    controller
        .lock()
        .await
        .clear_discord_credentials()
        .await
        .map_err(|err| err.to_string())
}

/// How the application behaves when the machine starts.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StartupPreferences {
    pub start_with_windows: bool,
    /// Start into the tray without building a window. The webview, and the memory and CPU that
    /// come with it, are only paid for once the user opens it.
    pub start_minimized: bool,
}

#[tauri::command]
pub async fn startup_preferences(
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<StartupPreferences, String> {
    let settings = controller.lock().await.config.settings().await;

    Ok(StartupPreferences {
        start_with_windows: settings.start_with_windows,
        start_minimized: settings.start_minimized,
    })
}

/// Stores the startup preferences and applies the launch-at-login one immediately.
///
/// Registering with the OS can fail on its own — a locked registry, a policy — so it is applied
/// first and the preference is only stored once it took, keeping the checkbox honest about what
/// will actually happen.
#[tauri::command]
pub async fn set_startup_preferences(
    preferences: StartupPreferences,
    app: AppHandle,
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    let applied = if preferences.start_with_windows {
        manager.enable()
    } else {
        manager.disable()
    };
    applied.map_err(|err| {
        warn!("Could not change the start with Windows setting: {err}");
        "Could not register the application to start with Windows".to_owned()
    })?;

    controller
        .lock()
        .await
        .config
        .update_startup_preferences(preferences.start_with_windows, preferences.start_minimized)
        .await;

    debug!("Startup preferences updated: {preferences:?}");
    Ok(())
}

/// Language the UI should render in, resolved by the backend.
///
/// The backend resolves it rather than the webview because it renders the tray menu, which exists
/// before any window does when the app starts minimised.
#[tauri::command]
pub async fn ui_language(controller: State<'_, Arc<Mutex<Controller>>>) -> Result<String, String> {
    Ok(controller
        .lock()
        .await
        .config
        .language()
        .await
        .tag()
        .to_owned())
}

/// Stores the chosen language and relabels the tray to match.
#[tauri::command]
pub async fn set_ui_language(
    language: String,
    app: AppHandle,
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<(), String> {
    let language =
        Language::from_tag(&language).ok_or_else(|| format!("Unsupported language: {language}"))?;

    controller
        .lock()
        .await
        .config
        .update_language(Some(language.tag().to_owned()))
        .await;

    // The tray was built at startup in the previous language; without this it would keep it until
    // the next launch.
    relabel_tray(&app, language)?;

    debug!("UI language set to {}", language.tag());
    Ok(())
}

/// Rebuilds the tray menu in the given language.
///
/// Tauri menus are immutable once built, so changing a label means building a new menu and
/// handing it to the existing tray icon.
fn relabel_tray(app: &AppHandle, language: Language) -> Result<(), String> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        // Nothing to relabel, which is not worth failing the language change over.
        warn!("No tray icon with id {TRAY_ID}, leaving its labels alone");
        return Ok(());
    };

    let menu = tray_menu(app, language).map_err(|err| {
        warn!("Could not build the tray menu: {err}");
        "Could not update the tray menu".to_owned()
    })?;

    tray.set_menu(Some(menu)).map_err(|err| {
        warn!("Could not apply the tray menu: {err}");
        "Could not update the tray menu".to_owned()
    })
}

/// Version the application was built with.
///
/// Read from the Tauri package info so there is one source of truth. The About tab used to
/// carry a hardcoded "1.0" while Cargo.toml said 0.1.0 and tauri.conf.json said 0.1.1.
#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub async fn controller_start(ui_refresh: State<'_, UiRefreshHandle>) -> Result<(), String> {
    // The coordinator is already running by the time the frontend loads; the workers were started
    // before the window existed. All this does now is ask for the current state, which the
    // frontend needs because nothing was emitted while there was no window to receive it.
    debug!("Frontend ready, resending the current state");
    let _ = ui_refresh.send(());
    Ok(())
}

/// Lighting configuration as the frontend sends it.
///
/// The mode travels as a name rather than the index of a `<select>`. Coupling it to the option
/// order meant reordering the dropdown silently changed what the device did — and they had in
/// fact drifted apart: the UI's second option read "Respiración" while the backend mapped it to
/// `Fixed`, and the third read "Fijo" while mapping to `Breathing`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RgbRequest {
    pub mode: RgbModeName,
    pub brightness: u8,
    /// Only the animated modes read it, but the frontend sends it unconditionally so the field
    /// does not have to be optional on either side.
    pub speed: u8,
    pub led1: LedRgb,
    pub led2: LedRgb,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RgbModeName {
    Rainbow,
    Fixed,
    Breathing,
}

impl From<RgbRequest> for RGBConfig {
    fn from(request: RgbRequest) -> Self {
        let RgbRequest {
            mode,
            brightness,
            speed,
            led1,
            led2,
        } = request;

        RGBConfig {
            brightness,
            speed,
            rgb_mode: match mode {
                RgbModeName::Rainbow => RGBMode::Rainbow,
                RgbModeName::Fixed => RGBMode::Fixed { led1, led2 },
                RgbModeName::Breathing => RGBMode::Breathing { led1, led2 },
            },
        }
    }
}

#[tauri::command]
pub async fn serial_set_rgb(
    request: RgbRequest,
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<(), String> {
    let update = RGBConfig::from(request);

    let mut controller = controller.lock().await;
    controller.config.update_rgb(&update).await;
    controller
        .serial_worker
        .set_rgb_config(&update)
        .await
        .map_err(|err| err.to_string())?;

    debug!("RGB update: {update:?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn led(red: u8, green: u8, blue: u8) -> LedRgb {
        LedRgb { red, green, blue }
    }

    fn request(mode: &str) -> RgbRequest {
        serde_json::from_str(&format!(
            r#"{{"mode":"{mode}","brightness":128,"speed":64,
                 "led1":{{"red":1,"green":2,"blue":3}},
                 "led2":{{"red":4,"green":5,"blue":6}}}}"#
        ))
        .expect("deserialises")
    }

    #[test]
    fn each_mode_name_maps_to_its_own_variant() {
        // Guards the coupling that used to exist through the dropdown's option order.
        assert_eq!(
            RGBConfig::from(request("fixed")).rgb_mode,
            RGBMode::Fixed {
                led1: led(1, 2, 3),
                led2: led(4, 5, 6)
            }
        );
        assert_eq!(
            RGBConfig::from(request("breathing")).rgb_mode,
            RGBMode::Breathing {
                led1: led(1, 2, 3),
                led2: led(4, 5, 6)
            }
        );
        assert_eq!(
            RGBConfig::from(request("rainbow")).rgb_mode,
            RGBMode::Rainbow
        );
    }

    /// Speed reaches the device even in the mode that ignores it, so the byte keeps its offset.
    #[test]
    fn speed_survives_the_conversion_in_every_mode() {
        for mode in ["rainbow", "fixed", "breathing"] {
            assert_eq!(RGBConfig::from(request(mode)).speed, 64, "mode {mode}");
        }
    }

    #[test]
    fn startup_preferences_travel_as_the_frontend_spells_them() {
        // The frontend sends these keys verbatim. Renaming a field without the serde attribute
        // would leave the checkboxes silently doing nothing rather than failing loudly.
        let parsed: StartupPreferences =
            serde_json::from_str(r#"{"startWithWindows":true,"startMinimized":false}"#)
                .expect("deserialises");

        assert!(parsed.start_with_windows);
        assert!(!parsed.start_minimized);
        assert_eq!(
            serde_json::to_string(&parsed).expect("serialises"),
            r#"{"startWithWindows":true,"startMinimized":false}"#
        );
    }

    #[test]
    fn an_unknown_mode_is_rejected_rather_than_defaulted() {
        // The name here only has to be one the enum does not carry. It used to be "breathing",
        // which stopped being unknown the moment `Wave` was renamed to match what the UI and the
        // firmware had always called that effect.
        assert!(serde_json::from_str::<RgbRequest>(r#"{"mode":"strobe","brightness":1,"led1":{"red":0,"green":0,"blue":0},"led2":{"red":0,"green":0,"blue":0}}"#).is_err());
    }
}
