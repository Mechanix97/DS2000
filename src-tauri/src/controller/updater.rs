//! In-app updates (#30).
//!
//! The release workflow publishes a `latest.json` next to the MSI, naming the version, the MSI's
//! URL and its signature. `tauri-plugin-updater` fetches it from the endpoint in tauri.conf.json,
//! and only an MSI signed with the update-signing key whose public half is in that file is
//! installed. Releases are drafts until published, so a draft is never offered.
//!
//! `releases/latest` skips pre-releases, so with pre-releases turned on in the About tab the
//! newest release of either kind is found through the GitHub API and its `latest.json` read
//! instead of the configured endpoint.
//!
//! Two commands rather than the plugin's own JavaScript API: installing has to shut the workers
//! down first, so the config is saved and the serial port and the Discord pipe are let go before
//! the app exits, and that sequence belongs in the backend.
//!
//! On Windows the MSI is not handed to the plugin's `install`: that starts `msiexec` and exits on
//! the spot, so a cancelled UAC prompt or a failed install would leave no app running. A detached
//! PowerShell waits for this process to exit, runs the installer and starts the old version again
//! if it did not succeed.

use crate::controller::Controller;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{AppHandle, State, Url};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// The update found by the last check, kept so installing does not fetch `latest.json` again and
/// cannot end up installing a different version than the one the user was shown.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

/// Newest first, published ones only without a token. Twenty is far more than ever sit between two
/// releases that carry a manifest.
const RELEASES_API: &str = "https://api.github.com/repos/Mechanix97/DS2000/releases?per_page=20";
const MANIFEST: &str = "latest.json";

/// How long checking may take, the GitHub API lookup for pre-releases included. Both answers are a
/// few KB, so anything slower is a hang.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the download may go without a chunk before it is abandoned. A stall rather than a
/// total limit, so a slow connection still finishes.
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// What the About tab shows for an available update.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    /// The release page. The workflow writes it to the manifest's `notes`, which the plugin reads
    /// as `body`.
    pub release_url: Option<String>,
}

/// Why checking failed, which decides what the About tab tells the user.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckError {
    /// The request did not complete: offline, a timeout, DNS.
    Offline,
    /// The server answered without a usable manifest: no release published yet, or GitHub
    /// returning an error.
    Unavailable,
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

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Whether pre-releases are offered as updates.
#[tauri::command]
pub async fn update_prereleases(
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<bool, String> {
    Ok(controller
        .lock()
        .await
        .config
        .settings()
        .await
        .update_prereleases)
}

#[tauri::command]
pub async fn set_update_prereleases(
    enabled: bool,
    controller: State<'_, Arc<Mutex<Controller>>>,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    controller
        .lock()
        .await
        .config
        .update_prereleases(enabled)
        .await;
    // What the last check found may not be on the new channel.
    *pending.0.lock().await = None;
    debug!("Pre-release updates {}", if enabled { "on" } else { "off" });
    Ok(())
}

/// Looks for a newer release. `None` means this is the latest version.
///
/// An error here is "could not check", not a fault in the app, and the About tab says so.
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
    controller: State<'_, Arc<Mutex<Controller>>>,
) -> Result<Option<UpdateInfo>, CheckError> {
    let prereleases = controller
        .lock()
        .await
        .config
        .settings()
        .await
        .update_prereleases;
    // Not the updater's own `timeout`: that is a total limit the update inherits for the download.
    let update = match tokio::time::timeout(CHECK_TIMEOUT, check(&app, prereleases)).await {
        Ok(Ok(update)) => update,
        Ok(Err(err)) => return Err(err),
        Err(_) => {
            warn!("Timed out checking for updates");
            return Err(CheckError::Offline);
        }
    };

    let info = update.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        release_url: update.body.clone(),
    });
    match &info {
        Some(info) => info!("Update available: {}", info.version),
        None => info!("No update available"),
    }

    *pending.0.lock().await = update;
    Ok(info)
}

async fn check(app: &AppHandle, prereleases: bool) -> Result<Option<Update>, CheckError> {
    let builder = app.updater_builder();
    let builder = if prereleases {
        let endpoint = newest_manifest().await?;
        builder.endpoints(vec![endpoint]).map_err(|err| {
            error!("Could not set the pre-release update endpoint: {err}");
            CheckError::Unavailable
        })?
    } else {
        builder
    };
    let updater = builder.build().map_err(|err| {
        error!("Could not build the updater: {err}");
        CheckError::Unavailable
    })?;
    updater.check().await.map_err(|err| {
        warn!("Could not check for updates: {err}");
        check_error(&err)
    })
}

/// The `latest.json` of the newest release that has one, pre-release or not.
async fn newest_manifest() -> Result<Url, CheckError> {
    let response = reqwest::Client::new()
        .get(RELEASES_API)
        // GitHub rejects API requests without a user agent.
        .header(reqwest::header::USER_AGENT, "DS2000-updater")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| {
            warn!("Could not list releases: {err}");
            CheckError::Offline
        })?;
    let releases: Vec<Release> = response
        .error_for_status()
        .map_err(|err| {
            warn!("Could not list releases: {err}");
            CheckError::Unavailable
        })?
        .json()
        .await
        .map_err(|err| {
            warn!("Could not read the release list: {err}");
            CheckError::Unavailable
        })?;
    manifest_url(&releases).ok_or_else(|| {
        warn!("No release has a {MANIFEST}");
        CheckError::Unavailable
    })
}

/// Releases come newest first, and only published ones reach an unauthenticated request.
fn manifest_url(releases: &[Release]) -> Option<Url> {
    releases.iter().find_map(|release| {
        let asset = release.assets.iter().find(|asset| asset.name == MANIFEST)?;
        debug!("Newest release with a manifest: {}", release.tag_name);
        asset.browser_download_url.parse().ok()
    })
}

/// A non-success status, a 404 included, comes back from the plugin as `ReleaseNotFound`; only a
/// request that never got an answer is the connection's fault.
fn check_error(err: &tauri_plugin_updater::Error) -> CheckError {
    match err {
        tauri_plugin_updater::Error::Reqwest(_) | tauri_plugin_updater::Error::Network(_) => {
            CheckError::Offline
        }
        _ => CheckError::Unavailable,
    }
}

/// Sends a progress message to the About tab. A failure means the webview is gone, which does not
/// stop the update, so it is logged rather than returned.
fn report(on_progress: &Channel<UpdateProgress>, progress: UpdateProgress) {
    if let Err(err) = on_progress.send(progress) {
        debug!("Could not send update progress to the About tab: {err}");
    }
}

/// Downloads and installs the update found by the last check, then exits for the installer.
///
/// On success the app exits and the installer starts the new version when it is done; if the
/// install does not go through, the current version is started again.
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
    let mut reported: Option<u64> = None;
    let last_chunk = Arc::new(std::sync::Mutex::new(Instant::now()));
    let download = update.download(
        {
            let last_chunk = Arc::clone(&last_chunk);
            let on_progress = on_progress.clone();
            move |chunk, total| {
                *last_chunk.lock().unwrap() = Instant::now();
                if reported.is_none() {
                    report(&on_progress, UpdateProgress::Started { total });
                }
                downloaded += chunk as u64;
                // Chunks are a few KB, so reporting each one would redraw the bar thousands of
                // times. Every percent is enough, or every 256 KB when the size is unknown.
                let step = total.map_or(256 * 1024, |total| (total / 100).max(1));
                if reported.is_none_or(|last| downloaded - last >= step) {
                    reported = Some(downloaded);
                    report(&on_progress, UpdateProgress::Progress { downloaded });
                }
            }
        },
        || {},
    );
    tokio::pin!(download);
    // Dropping the download on a stall cancels the request.
    let bytes = loop {
        tokio::select! {
            result = &mut download => break result.map_err(|err| err.to_string()),
            () = tokio::time::sleep(Duration::from_secs(1)) => {
                if last_chunk.lock().unwrap().elapsed() > DOWNLOAD_STALL_TIMEOUT {
                    break Err("The download stalled".to_owned());
                }
            }
        }
    }
    .inspect_err(|err| warn!("Could not download update {}: {err}", update.version))?;

    // Past this point the download is verified, and the app is going away either way.
    report(&on_progress, UpdateProgress::Installing);
    if let Err(err) = controller.lock().await.shutdown().await {
        // Same call as quitting from the tray: an unclean shutdown is not a reason to refuse.
        error!("Error shutting down the controller before the update: {err}");
    }

    info!("Installing update {}", update.version);
    #[cfg(windows)]
    match launch_msi(&update.version, &bytes).await {
        Ok(()) => {
            // An exit with a code goes through the tray app's `prevent_exit`, and unlike the
            // plugin's `process::exit` it removes the tray icon.
            app.exit(0);
            return Ok(());
        }
        Err(err) => error!(
            "Could not start the installer for {}: {err}",
            update.version
        ),
    }
    #[cfg(not(windows))]
    if let Err(err) = update.install(bytes) {
        error!("Could not install {}: {err}", update.version);
    }
    // Reaching the restart means either the installer could not be started, in which case the
    // workers are down and carrying on would leave a window that controls nothing, or a platform
    // that installs in place; starting over is right for both.
    app.restart()
}

/// Waits for this process to exit, then runs `msiexec`, and starts the current executable again if
/// the install did not succeed. The installer itself starts the new version on success.
///
/// `/passive` with `AUTOLAUNCHAPP=True` is what the plugin passes for `installMode: passive`. The
/// paths go through the environment so they need no quoting inside the script. 1641 and 3010 are
/// msiexec's "succeeded, restart initiated" and "succeeded, restart required".
#[cfg(windows)]
async fn launch_msi(version: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    const SCRIPT: &str = r#"
        Wait-Process -Id $env:DS2000_PID -Timeout 30 -ErrorAction SilentlyContinue
        $msiexec = Join-Path $env:SystemRoot 'System32\msiexec.exe'
        $msiArgs = '/i', ('"' + $env:DS2000_MSI + '"'), '/passive', '/promptrestart', 'AUTOLAUNCHAPP=True'
        $installer = Start-Process -FilePath $msiexec -ArgumentList $msiArgs -Wait -PassThru
        if ($installer.ExitCode -notin 0, 1641, 3010) { Start-Process -FilePath $env:DS2000_EXE }
        Remove-Item -LiteralPath $env:DS2000_MSI -ErrorAction SilentlyContinue
    "#;
    // The PowerShell outlives this process, so it gets no console window of its own.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let msi = std::env::temp_dir().join(format!("DS2000-{version}.msi"));
    tokio::fs::write(&msi, bytes).await?;
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            SCRIPT,
        ])
        .env("DS2000_PID", std::process::id().to_string())
        .env("DS2000_MSI", &msi)
        .env("DS2000_EXE", std::env::current_exe()?)
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
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

    // updater.js reads `releaseUrl` and switches on the error string.
    #[test]
    fn check_results_have_the_shape_the_about_tab_reads() {
        let info = UpdateInfo {
            version: "1.2.3".to_owned(),
            release_url: Some("https://example.com".to_owned()),
        };
        assert_eq!(
            serde_json::to_value(info).unwrap(),
            json!({"version": "1.2.3", "releaseUrl": "https://example.com"})
        );
        assert_eq!(
            serde_json::to_value(CheckError::Offline).unwrap(),
            json!("offline")
        );
        assert_eq!(
            serde_json::to_value(CheckError::Unavailable).unwrap(),
            json!("unavailable")
        );
    }

    #[test]
    fn the_newest_release_with_a_manifest_is_used() {
        let releases: Vec<Release> = serde_json::from_value(json!([
            {"tag_name": "v0.2.2", "assets": [{"name": "DS2000.msi", "browser_download_url": "https://x/a.msi"}]},
            {"tag_name": "v0.2.1", "prerelease": true, "assets": [
                {"name": "DS2000.msi", "browser_download_url": "https://x/b.msi"},
                {"name": "latest.json", "browser_download_url": "https://x/v0.2.1/latest.json"}
            ]},
            {"tag_name": "v0.2.0", "assets": [{"name": "latest.json", "browser_download_url": "https://x/v0.2.0/latest.json"}]}
        ]))
        .unwrap();
        assert_eq!(
            manifest_url(&releases).unwrap().as_str(),
            "https://x/v0.2.1/latest.json"
        );
        assert!(manifest_url(&[]).is_none());
    }

    #[test]
    fn only_an_unanswered_request_is_blamed_on_the_connection() {
        use tauri_plugin_updater::Error;
        assert!(matches!(
            check_error(&Error::Network("reset".to_owned())),
            CheckError::Offline
        ));
        assert!(matches!(
            check_error(&Error::ReleaseNotFound),
            CheckError::Unavailable
        ));
    }
}
