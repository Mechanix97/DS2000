//! In-app updates (#30).
//!
//! The release workflow publishes a `latest.json` next to the MSI, naming the version, the MSI's
//! URL and its signature. `tauri-plugin-updater` fetches it from the endpoint in tauri.conf.json,
//! and only an MSI signed with the update-signing key whose public half is in that file is
//! installed. Releases are drafts until published, so a draft is never offered.
//!
//! Two commands rather than the plugin's own JavaScript API: installing has to shut the workers
//! down first, so the config is saved and the serial port and the Discord pipe are let go before
//! the plugin exits the process, and that sequence belongs in the backend.

use crate::controller::Controller;

use serde::Serialize;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// The update found by the last check, kept so installing does not fetch `latest.json` again and
/// cannot end up installing a different version than the one the user was shown.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

/// What the About tab shows for an available update.
#[derive(Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

/// Download progress, streamed to the About tab while the MSI downloads.
#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum UpdateProgress {
    /// `total` is `None` when the server sends no content length.
    Started {
        total: Option<u64>,
    },
    Progress {
        downloaded: u64,
    },
    /// The MSI is downloaded and verified; the app is about to close for the installer.
    Installing,
}

/// Looks for a newer release. `None` means this is the latest version.
///
/// An error here is "could not check" (offline, GitHub down, no release published yet), not a
/// fault in the app, and the About tab says so.
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|err| err.to_string())?;
    let update = updater.check().await.map_err(|err| {
        warn!("Could not check for updates: {err}");
        err.to_string()
    })?;

    let info = update.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        notes: update.body.clone(),
    });
    match &info {
        Some(info) => info!("Update available: {}", info.version),
        None => info!("No update available"),
    }

    *pending.0.lock().await = update;
    Ok(info)
}

/// Downloads and installs the update found by the last check, then exits for the installer.
///
/// On success this does not return: the plugin hands the MSI to `msiexec` and exits the process,
/// and the installer starts the new version when it is done.
#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
    controller: State<'_, Arc<Mutex<Controller>>>,
    on_progress: Channel<UpdateProgress>,
) -> Result<(), String> {
    let Some(update) = pending.0.lock().await.take() else {
        return Err("No update to install: check for updates first".to_owned());
    };

    info!("Downloading update {}", update.version);
    let mut downloaded: u64 = 0;
    let mut started = false;
    let bytes = update
        .download(
            |chunk, total| {
                if !started {
                    started = true;
                    let _ = on_progress.send(UpdateProgress::Started { total });
                }
                downloaded += chunk as u64;
                let _ = on_progress.send(UpdateProgress::Progress { downloaded });
            },
            || {},
        )
        .await
        .map_err(|err| {
            warn!("Could not download update {}: {err}", update.version);
            err.to_string()
        })?;

    // Past this point the download is verified, and the app is going away either way.
    let _ = on_progress.send(UpdateProgress::Installing);
    if let Err(err) = controller.lock().await.shutdown().await {
        // Same call as quitting from the tray: an unclean shutdown is not a reason to refuse.
        error!("Error shutting down the controller before the update: {err}");
    }

    info!("Installing update {}", update.version);
    // On Windows a successful install exits inside this call. Reaching the restart means either
    // the installer could not be started, in which case the workers are down and carrying on would
    // leave a window that controls nothing, or a platform that installs in place; starting over
    // is right for both.
    if let Err(err) = update.install(bytes) {
        error!(
            "Could not start the installer for {}: {err}",
            update.version
        );
    }
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // updater.js switches on `event` and reads `data`; this is that contract.
    #[test]
    fn progress_messages_have_the_shape_the_about_tab_reads() {
        let encode = |progress| serde_json::to_value(progress).unwrap();
        assert_eq!(
            encode(UpdateProgress::Started { total: Some(10) }),
            json!({"event": "started", "data": {"total": 10}})
        );
        assert_eq!(
            encode(UpdateProgress::Progress { downloaded: 4 }),
            json!({"event": "progress", "data": {"downloaded": 4}})
        );
        assert_eq!(
            encode(UpdateProgress::Installing),
            json!({"event": "installing"})
        );
    }
}
