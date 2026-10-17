#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use controller::commands;
use controller::controller::Controller;
use controller::coordinator::{Coordinator, UiRefreshHandle};
use controller::tray::{MENU_QUIT, MENU_SHOW, TRAY_ID, tray_menu};
use controller::updater::{self, PendingUpdate};
use std::sync::Arc;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::MacosLauncher;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

/// Window the frontend runs in.
const MAIN_WINDOW: &str = "main";

/// Size the window opens at. Previously in tauri.conf.json, which no longer declares a window:
/// the webview is built on demand so that starting minimised does not pay for one.
///
/// The server rail and the channel sidebar take a fixed 312px between them, so the width is what
/// is left over that decides whether the content reads comfortably; 940 gives the RGB tab enough
/// room to lay the two LED cards side by side.
const WINDOW_WIDTH: f64 = 940.0;
const WINDOW_HEIGHT: f64 = 660.0;

/// Smallest size the layout still holds together at.
const WINDOW_MIN_WIDTH: f64 = 720.0;
const WINDOW_MIN_HEIGHT: f64 = 520.0;

/// Signals that shutdown has finished and the process may exit.
///
/// A newtype rather than a bare `UnboundedSender<()>`: Tauri keys managed state by type, and
/// `UiRefreshHandle` is also an `UnboundedSender<()>`, so registering both as-is panics at
/// startup with "state for type ... is already being managed".
#[derive(Clone)]
struct ShutdownSignal(mpsc::UnboundedSender<()>);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("App started");

    let (controller, worker_events) = Controller::new().await;
    let controller = Arc::new(Mutex::new(controller));

    if let Err(err) = controller.lock().await.start().await {
        error!("Controller could not start: {err}");
        std::process::exit(1);
    }

    let (ui_refresh_tx, ui_refresh_rx) = mpsc::unbounded_channel();
    // Signals that shutdown finished, replacing a thread that used to poll a flag ten times a
    // second for the whole life of the process.
    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel::<()>();

    // Read before the builder runs: `setup` is synchronous, and these decide whether a window is
    // created at all and what the tray is labelled in.
    let startup = controller.lock().await.config.settings().await;
    let (start_minimized, start_with_windows) =
        (startup.start_minimized, startup.start_with_windows);
    let language = controller.lock().await.config.language().await;

    let app = tauri::Builder::default()
        // Must come first, as the plugin requires. A second launch would otherwise fight this one
        // for the serial port and the Discord pipe, and with `start_minimized` the user would have
        // no window to tell them an instance is already running.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            info!("Another instance was launched, focusing this one");
            show_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            // Irrelevant on Windows, the only supported platform today, but the argument is not
            // optional. No launch arguments: starting minimised is a preference that is read from
            // the configuration on every start, however the process was launched.
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(controller.clone())
        .manage(PendingUpdate::default())
        .manage(ui_refresh_tx.clone())
        .manage(ShutdownSignal(shutdown_tx))
        .invoke_handler(tauri::generate_handler![
            commands::app_version,
            commands::controller_start,
            commands::ds_set_voice_settings_command,
            commands::serial_set_rgb,
            commands::discord_credentials_status,
            commands::discord_set_credentials,
            commands::discord_clear_credentials,
            commands::startup_preferences,
            commands::set_startup_preferences,
            commands::ui_language,
            commands::set_ui_language,
            updater::update_check,
            updater::update_install,
            updater::update_prereleases,
            updater::set_update_prereleases,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                debug!("Closing the window hides the app instead of quitting");
                api.prevent_close();
                if let Err(err) = window.hide() {
                    debug!("Could not hide the window: {err}");
                }
            }
        })
        .setup(move |app| {
            let menu = tray_menu(app.handle(), language)?;

            let mut tray = TrayIconBuilder::with_id(TRAY_ID).menu(&menu);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            } else {
                // Not fatal: the tray still works, it just has no icon.
                debug!("No default window icon available for the tray");
            }

            tray.on_menu_event(|app_handle, event| match event.id.as_ref() {
                MENU_QUIT => quit(app_handle.clone()),
                MENU_SHOW => show_window(app_handle),
                other => debug!("Unhandled tray menu item: {other}"),
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_window(tray.app_handle());
                }
            })
            .build(app)?;

            // The registry entry can drift from the configuration: an uninstall, a cleanup tool,
            // or another machine syncing the config file. The configuration is what the user set,
            // so it wins and the entry is brought back in line.
            sync_autostart(app.handle(), start_with_windows);

            if start_minimized {
                info!("Starting minimised to the tray, no webview created");
            } else if let Err(err) = create_window(app.handle()) {
                // Without a window the app is still usable from the tray, so this is not fatal.
                error!("Could not create the main window: {err}");
            }

            // The coordinator reacts to the workers; it needs the app handle to reach the webview.
            let coordinator = Coordinator::new(app.handle().clone(), controller.clone());
            tauri::async_runtime::spawn(coordinator.run(
                worker_events.discord,
                worker_events.serial,
                ui_refresh_rx,
            ));

            // Exits once shutdown reports it is done, instead of polling for it.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if shutdown_rx.recv().await.is_some() {
                    info!("Shutdown completed, exiting");
                    app_handle.exit(0);
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the tauri application");

    app.run(|_app, event| {
        // The app lives in the tray, so running out of windows is not a reason to exit. `code` is
        // `None` only for that case; an explicit `exit(0)` from the tray carries one and is let
        // through.
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
        }
    });
}

/// Builds the main window, and with it the webview.
///
/// Deferred rather than declared in tauri.conf.json so that a minimised start pays for neither.
fn create_window(app: &AppHandle) -> tauri::Result<()> {
    debug!("Creating the main window");
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .title("DS2000")
        .inner_size(WINDOW_WIDTH, WINDOW_HEIGHT)
        .min_inner_size(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT)
        .build()?;
    Ok(())
}

/// Brings the window up, creating it if this is the first time it is opened.
///
/// The refresh matters because nothing is emitted while the window is hidden, so without it the
/// UI would show whatever it last saw before being put away. A window built here does not need it
/// — its frontend asks for the state itself — but sending it twice is harmless.
fn show_window(app: &AppHandle) {
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) => {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        None => {
            if let Err(err) = create_window(app) {
                error!("Could not create the main window: {err}");
                return;
            }
        }
    }

    if let Some(refresh) = app.try_state::<UiRefreshHandle>() {
        let _ = refresh.send(());
    }
}

/// Makes the launch-at-login entry match the stored preference.
fn sync_autostart(app: &AppHandle, wanted: bool) {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    match manager.is_enabled() {
        Ok(current) if current == wanted => {}
        Ok(_) => {
            let result = if wanted {
                manager.enable()
            } else {
                manager.disable()
            };
            match result {
                Ok(()) => info!("Start with Windows set to {wanted}"),
                // Not fatal: the preference simply does not take effect, and the user can retry
                // from the UI. Refusing to start over a registry write would be worse.
                Err(err) => warn!("Could not set start with Windows to {wanted}: {err}"),
            }
        }
        Err(err) => warn!("Could not read the start with Windows setting: {err}"),
    }
}

/// Shuts the workers down and then signals the exit task.
fn quit(app: AppHandle) {
    let controller = app.state::<Arc<Mutex<Controller>>>().inner().clone();
    let shutdown = app.state::<ShutdownSignal>().inner().clone();

    tauri::async_runtime::spawn(async move {
        if let Err(err) = controller.lock().await.shutdown().await {
            // Report it and exit anyway: refusing to quit is worse than an unclean shutdown.
            error!("Error shutting down the controller: {err}");
        }
        let _ = shutdown.0.send(());
    });
}
