//! Device firmware updates from the app (#35).
//!
//! The firmware repository's release workflow attaches one UF2 per board,
//! `DS2000-Firmware_<version>_<environment>.uf2`, its minisign signature
//! (`<image>.uf2.minisig`), and a `SHA256SUMS` next to them. Checking finds the latest release and
//! the image built for the board the device reports; installing downloads it, checks its checksum
//! and its signature, sends the device to USB boot, copies the image to the drive it mounts as, and
//! waits for the device to come back with the new version.
//!
//! The serial worker is paused from the reboot until the device is expected back: while the device
//! is in USB boot its port is gone, and reopening the first one that appears could catch it
//! half-way through.

use crate::controller::Controller;

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use serial::messages::device_info::{Board, DeviceInfoMessage, FirmwareVersion};
use serial::serial_message::PROTOCOL_VERSION;
use serial::serial_state::DeviceStatus;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use tauri::ipc::Channel;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Mechanix97/DS2000-Firmware/releases/latest";
/// The firmware's changelog, from `main` rather than a tag: the releases so far were tagged
/// before the file existed. Entries past the latest published release are left out by the
/// About tab, so a release pull request merged ahead of publishing does not show early.
const CHANGELOG_URL: &str =
    "https://raw.githubusercontent.com/Mechanix97/DS2000-Firmware/main/CHANGELOG.md";
const CHECKSUMS: &str = "SHA256SUMS";
/// The firmware repository's signing key (its `minisign.pub`). Only images signed with the matching
/// secret key are written to the device: the checksums come from the same release as the image, so
/// they catch a broken download but not a replaced release. Never change it without a plan:
/// released apps refuse firmware signed with any other key.
const FIRMWARE_PUBLIC_KEY: &str = "RWRypq+y05cyTVMXLmszCfvG4yt2sBUB7noOHBIIivM4DsE1UqbVGxJ9";

/// How long checking may take. The release JSON is a few KB, so anything slower is a hang.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the image download may take in total. A UF2 is well under a megabyte.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the device has to show up as a drive after being sent to USB boot.
const DRIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the device has to come back on its serial port after the image is written.
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The release found by the last check, kept so installing writes the image the user was shown.
#[derive(Default)]
pub struct PendingFirmware(Mutex<Option<FirmwareRelease>>);

#[derive(Clone, Debug)]
pub struct FirmwareRelease {
    version: FirmwareVersion,
    image_url: String,
    image_name: String,
    signature_url: String,
    checksums_url: String,
}

/// What the About tab shows for an available firmware update.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareUpdateInfo {
    pub version: String,
}

/// The firmware's CHANGELOG.md and its latest published version, for About → Changelog.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareChangelog {
    pub markdown: String,
    pub latest: String,
}

/// Why checking failed, which decides what the About tab tells the user.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FirmwareCheckError {
    /// No DS2000 attached, or one newer than this app that only an app update can talk to.
    NoDevice,
    /// The device's board has no firmware build.
    UnknownBoard,
    /// The request did not complete: offline, a timeout, DNS.
    Offline,
    /// GitHub answered without a usable release.
    Unavailable,
}

/// Install progress, streamed to the About tab.
#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum FirmwareProgress {
    Downloading,
    /// Sending the device to USB boot and waiting for its drive.
    Rebooting,
    /// Writing the image to the drive. Unplugging now is what can go wrong.
    Flashing,
    /// The image is written; waiting for the device to come back with it.
    Restarting,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// The firmware's changelog, fetched when asked for: firmware releases come out on their own
/// schedule, so a copy bundled with the app would go stale. Needs no device.
#[tauri::command]
pub async fn firmware_changelog() -> Result<FirmwareChangelog, FirmwareCheckError> {
    let fetch = async {
        let (release, markdown) = tokio::try_join!(latest_release(), fetch_changelog())?;
        let latest = parse_version(&release.tag_name).ok_or_else(|| {
            warn!("Firmware release tag {} is not a version", release.tag_name);
            FirmwareCheckError::Unavailable
        })?;
        Ok(FirmwareChangelog {
            markdown,
            latest: latest.to_string(),
        })
    };
    tokio::time::timeout(CHECK_TIMEOUT, fetch)
        .await
        .map_err(|_| {
            warn!("Timed out fetching the firmware changelog");
            FirmwareCheckError::Offline
        })?
}

async fn fetch_changelog() -> Result<String, FirmwareCheckError> {
    let response = reqwest::Client::new()
        .get(CHANGELOG_URL)
        .header(reqwest::header::USER_AGENT, "DS2000")
        .send()
        .await
        .map_err(|err| {
            warn!("Could not fetch the firmware changelog: {err}");
            FirmwareCheckError::Offline
        })?;
    response
        .error_for_status()
        .map_err(|err| {
            warn!("Could not fetch the firmware changelog: {err}");
            FirmwareCheckError::Unavailable
        })?
        .text()
        .await
        .map_err(|err| {
            warn!("Could not read the firmware changelog: {err}");
            FirmwareCheckError::Unavailable
        })
}

/// Looks for newer firmware for the attached device. `None` means it is up to date.
#[tauri::command]
pub async fn firmware_check(
    controller: State<'_, Arc<Mutex<Controller>>>,
    pending: State<'_, PendingFirmware>,
) -> Result<Option<FirmwareUpdateInfo>, FirmwareCheckError> {
    *pending.0.lock().await = None;
    let device = updatable_device(&controller).await?;
    let environment = board_environment(device.board).ok_or_else(|| {
        info!("No firmware is built for {}", device.board);
        FirmwareCheckError::UnknownBoard
    })?;

    let release = tokio::time::timeout(CHECK_TIMEOUT, latest_release())
        .await
        .map_err(|_| {
            warn!("Timed out checking for firmware updates");
            FirmwareCheckError::Offline
        })??;
    let Some(release) = release_for(&release, environment)? else {
        return Ok(None);
    };

    // An outdated protocol is always worth replacing; otherwise only a newer version is.
    let newer = device.protocol < PROTOCOL_VERSION || release.version > device.firmware;
    if !newer {
        info!("Firmware {} is up to date", device.firmware);
        return Ok(None);
    }
    info!(
        "Firmware {} available for {} (running {})",
        release.version, device.board, device.firmware
    );
    let info = FirmwareUpdateInfo {
        version: release.version.to_string(),
    };
    *pending.0.lock().await = Some(release);
    Ok(Some(info))
}

/// Installs the firmware found by the last check. Resolves with the version the device came back
/// with.
#[tauri::command]
pub async fn firmware_install(
    controller: State<'_, Arc<Mutex<Controller>>>,
    pending: State<'_, PendingFirmware>,
    on_progress: Channel<FirmwareProgress>,
) -> Result<String, String> {
    let Some(release) = pending.0.lock().await.take() else {
        return Err("No firmware to install: check for updates first".to_owned());
    };

    report(&on_progress, FirmwareProgress::Downloading);
    let image = download_image(&release).await.inspect_err(|err| {
        warn!("Could not download firmware {}: {err}", release.version);
    })?;

    report(&on_progress, FirmwareProgress::Rebooting);
    let entered = controller
        .lock()
        .await
        .serial_worker
        .enter_bootloader()
        .await
        .map_err(|err| err.to_string())?;
    if !entered {
        return Err("No device to update".to_owned());
    }

    // From here the serial worker is paused, and has to be resumed whatever happens.
    let flashed = flash(&release, &image, &on_progress).await;
    if let Err(err) = controller.lock().await.serial_worker.resume().await {
        error!("Could not resume the serial worker after the firmware update: {err}");
    }
    flashed.inspect_err(|err| warn!("Could not flash firmware {}: {err}", release.version))?;

    report(&on_progress, FirmwareProgress::Restarting);
    wait_for_version(&controller, release.version)
        .await
        .inspect_err(|err| warn!("Firmware update not confirmed: {err}"))?;
    info!("Firmware updated to {}", release.version);
    Ok(release.version.to_string())
}

/// A device this app can update: connected, or a DS2000 on an older protocol.
async fn updatable_device(
    controller: &Mutex<Controller>,
) -> Result<DeviceInfoMessage, FirmwareCheckError> {
    let status = controller
        .lock()
        .await
        .serial_worker
        .device_status()
        .await
        .map_err(|err| {
            error!("Could not read the device status: {err}");
            FirmwareCheckError::NoDevice
        })?;
    match status {
        DeviceStatus::Connected(device) => Ok(device),
        DeviceStatus::Incompatible(device) if device.protocol < PROTOCOL_VERSION => Ok(device),
        _ => Err(FirmwareCheckError::NoDevice),
    }
}

/// The firmware's build environment for a board, which names its image. The table is in
/// `docs/PROTOCOL.md`, Board ids.
fn board_environment(board: Board) -> Option<&'static str> {
    match board {
        Board::Rp2350Zero => Some("rp2350_zero"),
        Board::Pico2 => Some("pico2"),
        Board::Pico => Some("pico"),
        Board::Ds2000RevA => Some("rev_a"),
        Board::Unknown(_) => None,
    }
}

async fn latest_release() -> Result<Release, FirmwareCheckError> {
    let response = reqwest::Client::new()
        .get(LATEST_RELEASE_API)
        // GitHub rejects API requests without a user agent.
        .header(reqwest::header::USER_AGENT, "DS2000")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| {
            warn!("Could not fetch the latest firmware release: {err}");
            FirmwareCheckError::Offline
        })?;
    response
        .error_for_status()
        .map_err(|err| {
            warn!("Could not fetch the latest firmware release: {err}");
            FirmwareCheckError::Unavailable
        })?
        .json()
        .await
        .map_err(|err| {
            warn!("Could not read the latest firmware release: {err}");
            FirmwareCheckError::Unavailable
        })
}

/// The image in `release` for `environment`, or `None` when the release speaks another protocol.
fn release_for(
    release: &Release,
    environment: &str,
) -> Result<Option<FirmwareRelease>, FirmwareCheckError> {
    let version = parse_version(&release.tag_name).ok_or_else(|| {
        warn!("Firmware release tag {} is not a version", release.tag_name);
        FirmwareCheckError::Unavailable
    })?;

    // Flashing firmware on another protocol would leave a device this app cannot talk to.
    if let Some(protocol) = release.body.as_deref().and_then(release_protocol)
        && protocol != PROTOCOL_VERSION
    {
        info!(
            "Firmware {version} speaks protocol {protocol}, this app speaks {PROTOCOL_VERSION}; \
             not offering it"
        );
        return Ok(None);
    }

    let image_name = format!("DS2000-Firmware_{version}_{environment}.uf2");
    let url = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
    };
    let Some(image_url) = url(&image_name) else {
        warn!("Firmware {version} has no {image_name}");
        return Err(FirmwareCheckError::UnknownBoard);
    };
    let signature_name = format!("{image_name}.minisig");
    let (Some(signature_url), Some(checksums_url)) = (url(&signature_name), url(CHECKSUMS)) else {
        warn!("Firmware {version} has no {signature_name} or {CHECKSUMS}");
        return Err(FirmwareCheckError::Unavailable);
    };
    Ok(Some(FirmwareRelease {
        version,
        image_url,
        image_name,
        signature_url,
        checksums_url,
    }))
}

/// `v1.2.3` or `1.2.3`.
fn parse_version(tag: &str) -> Option<FirmwareVersion> {
    let mut parts = tag.strip_prefix('v').unwrap_or(tag).split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = FirmwareVersion {
        major: next()?,
        minor: next()?,
        patch: next()?,
    };
    parts.next().is_none().then_some(version)
}

/// The protocol the release notes say the firmware speaks: the workflow writes
/// "speaks **serial protocol N**".
fn release_protocol(body: &str) -> Option<u8> {
    let rest = &body[body.find("serial protocol ")? + "serial protocol ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

async fn download_image(release: &FirmwareRelease) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;
    let fetch = |url: String| {
        let client = client.clone();
        async move {
            client
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|err| err.to_string())?
                .bytes()
                .await
                .map_err(|err| err.to_string())
        }
    };
    let checksums = fetch(release.checksums_url.clone()).await?;
    let signature = fetch(release.signature_url.clone()).await?;
    let image = fetch(release.image_url.clone()).await?;

    let checksums = String::from_utf8_lossy(&checksums);
    let expected = expected_checksum(&checksums, &release.image_name)
        .ok_or_else(|| format!("{CHECKSUMS} has no entry for {}", release.image_name))?;
    let actual = hex(&Sha256::digest(&image));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "{} does not match its checksum: expected {expected}, got {actual}",
            release.image_name
        ));
    }
    verify_signature(&image, &String::from_utf8_lossy(&signature)).map_err(|err| {
        format!(
            "{} is not signed with the firmware key: {err}",
            release.image_name
        )
    })?;
    debug!("{} verified, {} bytes", release.image_name, image.len());
    Ok(image.to_vec())
}

/// The hash for `name` in a `sha256sum` listing: `<hash>  <name>` per line, or `<hash> *<name>`
/// for files hashed in binary mode.
fn expected_checksum<'a>(checksums: &'a str, name: &str) -> Option<&'a str> {
    checksums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name).then_some(hash)
    })
}

/// Checks a `.minisig` file against [`FIRMWARE_PUBLIC_KEY`]. Only prehashed signatures, the kind
/// the release workflow makes, are accepted.
fn verify_signature(image: &[u8], signature: &str) -> Result<(), String> {
    let key = PublicKey::from_base64(FIRMWARE_PUBLIC_KEY).map_err(|err| err.to_string())?;
    let signature = Signature::decode(signature).map_err(|err| err.to_string())?;
    key.verify(image, &signature, false)
        .map_err(|err| err.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Waits for the USB boot drive and writes the image to it.
async fn flash(
    release: &FirmwareRelease,
    image: &[u8],
    on_progress: &Channel<FirmwareProgress>,
) -> Result<(), String> {
    let drive = wait_for_drive().await?;
    info!("USB boot drive at {}", drive.display());

    report(on_progress, FirmwareProgress::Flashing);
    // The device reboots as soon as the last block lands, which can take the drive away before
    // the write call returns. Whether it worked is decided by the version it comes back with.
    if let Err(err) = tokio::fs::write(drive.join(&release.image_name), image).await {
        debug!("Write to the USB boot drive ended with an error: {err}");
    }
    Ok(())
}

async fn wait_for_drive() -> Result<PathBuf, String> {
    let deadline = tokio::time::Instant::now() + DRIVE_TIMEOUT;
    loop {
        for root in drive_roots() {
            if is_usb_boot_drive(&root).await {
                return Ok(root);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("The device did not show up as a USB drive".to_owned());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The RP2040 and RP2350 boot ROMs put an `INFO_UF2.TXT` on their drive naming the chip, which
/// tells them apart from any other removable drive whatever its letter or label.
async fn is_usb_boot_drive(root: &Path) -> bool {
    match tokio::fs::read_to_string(root.join("INFO_UF2.TXT")).await {
        Ok(info) => info.contains("RP2350") || info.contains("RPI-RP2"),
        Err(_) => false,
    }
}

/// Where a newly mounted drive can appear.
#[cfg(windows)]
fn drive_roots() -> Vec<PathBuf> {
    // A and B are floppy letters, and C is the system drive.
    (b'D'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .collect()
}

#[cfg(not(windows))]
fn drive_roots() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    [
        "/Volumes".to_owned(),
        format!("/media/{user}"),
        format!("/run/media/{user}"),
    ]
    .iter()
    .filter_map(|dir| std::fs::read_dir(dir).ok())
    .flatten()
    .filter_map(|entry| Some(entry.ok()?.path()))
    .collect()
}

/// Waits for the device to reconnect and checks it runs `version`.
async fn wait_for_version(
    controller: &Mutex<Controller>,
    version: FirmwareVersion,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + RECONNECT_TIMEOUT;
    loop {
        let status = controller
            .lock()
            .await
            .serial_worker
            .device_status()
            .await
            .map_err(|err| err.to_string())?;
        match status {
            DeviceStatus::Connected(device) | DeviceStatus::Incompatible(device)
                if device.firmware == version =>
            {
                return Ok(());
            }
            DeviceStatus::Connected(device) | DeviceStatus::Incompatible(device) => {
                return Err(format!(
                    "The device came back with firmware {}, not {version}",
                    device.firmware
                ));
            }
            DeviceStatus::Disconnected => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("The device did not come back after the update".to_owned());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Sends a progress message to the About tab. A failure means the webview is gone, which does not
/// stop the update, so it is logged rather than returned.
fn report(on_progress: &Channel<FirmwareProgress>, progress: FirmwareProgress) {
    if let Err(err) = on_progress.send(progress) {
        debug!("Could not send firmware progress to the About tab: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, body: &str, assets: &[&str]) -> Release {
        serde_json::from_value(json!({
            "tag_name": tag,
            "body": body,
            "assets": assets
                .iter()
                .map(|name| json!({"name": name, "browser_download_url": format!("https://x/{name}")}))
                .collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    const NOTES: &str = "## Compatibility\n\nThis release speaks **serial protocol 1**. Pair it";

    #[test]
    fn versions_parse_with_or_without_the_v() {
        let expected = FirmwareVersion {
            major: 0,
            minor: 3,
            patch: 1,
        };
        assert_eq!(parse_version("v0.3.1"), Some(expected));
        assert_eq!(parse_version("0.3.1"), Some(expected));
        for bad in ["v0.3", "v0.3.1.2", "v0.3.x", "latest", "v0.3.1-beta"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_protocol_is_read_from_the_release_notes() {
        assert_eq!(release_protocol(NOTES), Some(1));
        assert_eq!(release_protocol("speaks **serial protocol 12**"), Some(12));
        assert_eq!(release_protocol("no compatibility section"), None);
    }

    #[test]
    fn the_image_for_the_board_is_picked() {
        let release = release(
            "v0.3.0",
            NOTES,
            &[
                "DS2000-Firmware_0.3.0_pico.uf2",
                "DS2000-Firmware_0.3.0_pico.uf2.minisig",
                "DS2000-Firmware_0.3.0_rp2350_zero.uf2",
                "DS2000-Firmware_0.3.0_rp2350_zero.uf2.minisig",
                "SHA256SUMS",
            ],
        );
        let found = release_for(&release, "rp2350_zero").unwrap().unwrap();
        assert_eq!(found.image_name, "DS2000-Firmware_0.3.0_rp2350_zero.uf2");
        assert_eq!(
            found.image_url,
            "https://x/DS2000-Firmware_0.3.0_rp2350_zero.uf2"
        );
        assert_eq!(
            found.signature_url,
            "https://x/DS2000-Firmware_0.3.0_rp2350_zero.uf2.minisig"
        );
        assert_eq!(found.checksums_url, "https://x/SHA256SUMS");

        assert_eq!(
            release_for(&release, "pico2").unwrap_err(),
            FirmwareCheckError::UnknownBoard
        );
    }

    #[test]
    fn an_unsigned_image_is_not_offered() {
        let release = release(
            "v0.3.0",
            NOTES,
            &["DS2000-Firmware_0.3.0_rp2350_zero.uf2", "SHA256SUMS"],
        );
        assert_eq!(
            release_for(&release, "rp2350_zero").unwrap_err(),
            FirmwareCheckError::Unavailable
        );
    }

    // Made the way the release workflow signs an image, with the firmware key, over SIGNED_IMAGE.
    const SIGNED_IMAGE: &[u8] = b"DS2000 test image\n";
    const SIGNATURE: &str = "untrusted comment: signature from tauri secret key
RURypq+y05cyTW0N5e/tTX6ldnYRh4Nou7htAqgXxwdyQN4fFn+BnYgoY7sb/7nW9iC+nvnxOR9zR/X/xhnjbC/Fv7V+GMlw9AE=
trusted comment: timestamp:1792251393\tfile:image.uf2
AK5mtdxZZ41BoQfbvyPkqh4qzz6lTxVmKI9eeMSx9kqFv+w9rRYrSJSOeDuxqnlZtAyfuPudWSih6kLUKGGICQ==
";

    #[test]
    fn an_image_signed_with_the_firmware_key_verifies() {
        assert_eq!(verify_signature(SIGNED_IMAGE, SIGNATURE), Ok(()));
    }

    #[test]
    fn a_changed_image_or_a_broken_signature_does_not_verify() {
        assert!(verify_signature(b"DS2000 test image!\n", SIGNATURE).is_err());
        assert!(verify_signature(SIGNED_IMAGE, "not a signature").is_err());
        let tampered = SIGNATURE.replace("AK5m", "BK5m");
        assert!(verify_signature(SIGNED_IMAGE, &tampered).is_err());
    }

    #[test]
    fn firmware_on_another_protocol_is_not_offered() {
        let notes = NOTES.replace("protocol 1", "protocol 2");
        let release = release(
            "v0.4.0",
            &notes,
            &["DS2000-Firmware_0.4.0_rp2350_zero.uf2", "SHA256SUMS"],
        );
        assert!(release_for(&release, "rp2350_zero").unwrap().is_none());
    }

    #[test]
    fn checksums_are_found_by_file_name() {
        let sums =
            "aa11  DS2000-Firmware_0.3.0_pico.uf2\nbb22 *DS2000-Firmware_0.3.0_rp2350_zero.uf2\n";
        assert_eq!(
            expected_checksum(sums, "DS2000-Firmware_0.3.0_rp2350_zero.uf2"),
            Some("bb22")
        );
        assert_eq!(
            expected_checksum(sums, "DS2000-Firmware_0.3.0_pico.uf2"),
            Some("aa11")
        );
        assert_eq!(
            expected_checksum(sums, "DS2000-Firmware_0.3.0_pico2.uf2"),
            None
        );
    }

    #[test]
    fn the_digest_is_lowercase_hex() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn every_buildable_board_has_an_environment() {
        assert_eq!(board_environment(Board::Rp2350Zero), Some("rp2350_zero"));
        assert_eq!(board_environment(Board::Pico2), Some("pico2"));
        assert_eq!(board_environment(Board::Pico), Some("pico"));
        assert_eq!(board_environment(Board::Ds2000RevA), Some("rev_a"));
        assert_eq!(board_environment(Board::Unknown(0x42)), None);
    }

    // firmwareUpdater.js switches on these strings.
    #[test]
    fn messages_have_the_shape_the_about_tab_reads() {
        assert_eq!(
            serde_json::to_value(FirmwareProgress::Flashing).unwrap(),
            json!({"event": "flashing"})
        );
        assert_eq!(
            serde_json::to_value(FirmwareCheckError::UnknownBoard).unwrap(),
            json!("unknownBoard")
        );
    }
}
