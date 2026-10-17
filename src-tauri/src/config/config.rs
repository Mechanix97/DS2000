//! Persistent application settings.
//!
//! Everything here is non-sensitive and stored as readable JSON, which keeps the file easy to
//! inspect and to attach to a bug report. Secrets live in the OS keyring instead — see
//! [`crate::credentials`].

use crate::credentials::{self, CredentialError, Secret};
use crate::language::{self, Language};
use common::rgb_update::RGBConfig;

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};

const CONFIG_SAVE_INTERVAL: Duration = Duration::from_secs(60);
const CONFIG_DIR: &str = "Mechardo";
const CONFIG_FILE: &str = "config.json";

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("could not locate the user's configuration directory")]
    NoConfigDir,

    #[error("configuration I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("configuration is not valid JSON: {0}")]
    Serde(#[from] serde_json::Error),

    #[error(transparent)]
    Credential(#[from] CredentialError),
}

/// Settings persisted to disk. Nothing in here is sensitive.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Settings {
    /// Client id of the Discord application registered by the user.
    ///
    /// Not a secret: Discord displays it in the authorisation modal. The matching client secret
    /// is in the keyring.
    pub discord_client_id: Option<String>,

    /// Serial port used last, tried first on the next launch to skip rescanning.
    pub last_used_port: Option<String>,

    pub rgb_config: RGBConfig,

    /// UI language tag, e.g. `"es"` or `"en"`. `None` means follow the system.
    pub language: Option<String>,

    pub start_with_windows: bool,
    pub start_minimized: bool,
}

pub struct Config {
    inner: Arc<Mutex<Settings>>,
    dirty: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

impl Config {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Settings::default())),
            dirty: Arc::new(AtomicBool::new(false)),
            join_handle: None,
        }
    }

    /// Loads settings from disk.
    ///
    /// A missing, unreadable or malformed file is not fatal: the defaults are kept and the reason
    /// is logged. Losing preferences is a far better outcome than refusing to start, which is
    /// what the previous implementation did.
    pub async fn load(&mut self) {
        let path = match config_file_path() {
            Ok(path) => path,
            Err(err) => {
                warn!("Could not determine the config path, using defaults: {err}");
                return;
            }
        };

        if !path.exists() {
            info!("No configuration file yet, starting with defaults");
            return;
        }

        match read_settings(&path) {
            Ok(settings) => {
                *self.inner.lock().await = settings;
                debug!("Configuration loaded from {}", path.display());
            }
            Err(err) => {
                warn!(
                    "Could not read {}, continuing with defaults: {err}",
                    path.display()
                );
            }
        }
    }

    /// Starts the background task that flushes pending changes periodically.
    pub async fn start(&mut self) {
        debug!("Starting config save task");
        let inner = self.inner.clone();
        let dirty = self.dirty.clone();

        self.join_handle = Some(tokio::spawn(async move {
            loop {
                sleep(CONFIG_SAVE_INTERVAL).await;
                if dirty.swap(false, atomic::Ordering::Relaxed) {
                    let settings = inner.lock().await.clone();
                    if let Err(err) = save_settings(&settings) {
                        warn!("Could not save configuration: {err}");
                        dirty.store(true, atomic::Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    /// Writes the current settings to disk immediately.
    pub async fn save(&self) -> Result<(), ConfigError> {
        let settings = self.inner.lock().await.clone();
        save_settings(&settings)?;
        self.dirty.store(false, atomic::Ordering::Relaxed);
        Ok(())
    }

    pub async fn discord_client_id(&self) -> Option<String> {
        self.inner.lock().await.discord_client_id.clone()
    }

    pub async fn last_used_port(&self) -> Option<String> {
        self.inner.lock().await.last_used_port.clone()
    }

    /// Language the UI should use, resolved against the system when none is stored.
    pub async fn language(&self) -> Language {
        language::resolve(self.inner.lock().await.language.as_deref())
    }

    pub async fn rgb_config(&self) -> RGBConfig {
        self.inner.lock().await.rgb_config.clone()
    }

    pub async fn settings(&self) -> Settings {
        self.inner.lock().await.clone()
    }

    /// Stores the Discord application credentials the user pasted into the UI.
    ///
    /// The id goes to the config file and the secret to the keyring. Any previously stored OAuth
    /// tokens are dropped, because they were issued by whatever application was configured before
    /// and are meaningless for the new one.
    pub async fn set_discord_credentials(
        &mut self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(), ConfigError> {
        credentials::write(Secret::ClientSecret, client_secret)?;
        credentials::clear_tokens()?;

        self.inner.lock().await.discord_client_id = Some(client_id.to_owned());
        self.save().await
    }

    /// Forgets the Discord application entirely: id, secret and tokens.
    pub async fn clear_discord_credentials(&mut self) -> Result<(), ConfigError> {
        credentials::clear_all()?;
        self.inner.lock().await.discord_client_id = None;
        self.save().await
    }

    /// True when both halves of the Discord application registration are present.
    ///
    /// Either half alone is useless, so the workers stay idle until both exist.
    pub async fn has_discord_credentials(&self) -> bool {
        if self.inner.lock().await.discord_client_id.is_none() {
            return false;
        }
        matches!(credentials::read(Secret::ClientSecret), Ok(Some(_)))
    }

    pub async fn update_tokens(
        &mut self,
        access_token: Option<String>,
        refresh_token: Option<String>,
    ) -> Result<(), ConfigError> {
        store_optional_secret(Secret::AccessToken, access_token)?;
        store_optional_secret(Secret::RefreshToken, refresh_token)?;
        Ok(())
    }

    pub async fn update_last_used_port(&mut self, last_used_port: Option<String>) {
        let mut settings = self.inner.lock().await;
        if settings.last_used_port != last_used_port {
            settings.last_used_port = last_used_port;
            self.dirty.store(true, atomic::Ordering::Relaxed);
        }
    }

    pub async fn update_rgb(&mut self, rgb_update: &RGBConfig) {
        let mut settings = self.inner.lock().await;
        if settings.rgb_config != *rgb_update {
            settings.rgb_config = rgb_update.clone();
            self.dirty.store(true, atomic::Ordering::Relaxed);
        }
    }

    pub async fn update_startup_preferences(
        &mut self,
        start_with_windows: bool,
        start_minimized: bool,
    ) {
        let mut settings = self.inner.lock().await;
        if settings.start_with_windows != start_with_windows
            || settings.start_minimized != start_minimized
        {
            settings.start_with_windows = start_with_windows;
            settings.start_minimized = start_minimized;
            self.dirty.store(true, atomic::Ordering::Relaxed);
        }
    }

    pub async fn update_language(&mut self, language: Option<String>) {
        let mut settings = self.inner.lock().await;
        if settings.language != language {
            settings.language = language;
            self.dirty.store(true, atomic::Ordering::Relaxed);
        }
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            handle.abort();
        }
    }
}

fn store_optional_secret(secret: Secret, value: Option<String>) -> Result<(), CredentialError> {
    match value {
        Some(value) => credentials::write(secret, &value),
        None => credentials::clear(secret),
    }
}

fn config_file_path() -> Result<PathBuf, ConfigError> {
    let base_dirs = BaseDirs::new().ok_or(ConfigError::NoConfigDir)?;
    Ok(base_dirs.config_dir().join(CONFIG_DIR).join(CONFIG_FILE))
}

fn read_settings(path: &PathBuf) -> Result<Settings, ConfigError> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn save_settings(settings: &Settings) -> Result<(), ConfigError> {
    let path = config_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(settings)?)?;
    debug!("Configuration saved to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_discord_application_configured() {
        let settings = Settings::default();
        assert!(settings.discord_client_id.is_none());
        assert!(!settings.start_with_windows);
        assert!(!settings.start_minimized);
    }

    #[test]
    fn settings_survive_a_serialisation_round_trip() {
        let settings = Settings {
            discord_client_id: Some("123456789".to_owned()),
            last_used_port: Some("COM3".to_owned()),
            language: Some("es".to_owned()),
            start_with_windows: true,
            ..Settings::default()
        };

        let json = serde_json::to_string(&settings).expect("serialises");
        let parsed: Settings = serde_json::from_str(&json).expect("deserialises");

        assert_eq!(settings, parsed);
    }

    #[test]
    fn unknown_and_missing_fields_fall_back_to_defaults() {
        // Guards forward and backward compatibility: a config written by another version must not
        // stop the app from starting.
        let parsed: Settings = serde_json::from_str(r#"{"language":"en","from_the_future":42}"#)
            .expect("tolerates unknown and missing fields");

        assert_eq!(parsed.language.as_deref(), Some("en"));
        assert_eq!(parsed.rgb_config, RGBConfig::default());
        assert!(parsed.discord_client_id.is_none());
    }

    #[test]
    fn no_secret_is_serialised_into_the_config_file() {
        // The client secret and OAuth tokens belong in the keyring. If a field for them ever
        // appears here, this test should fail and force the decision to be revisited.
        let json = serde_json::to_string(&Settings::default()).expect("serialises");
        assert!(!json.contains("secret"));
        assert!(!json.contains("token"));
    }
}
