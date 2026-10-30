use crate::credentials::DiscordCredentials;
use crate::discord_state::{
    DiscordState, DiscordStateHandler, DiscordStateProtocol, DiscordVoiceSettings,
    DiscordWorkerEvent,
};
use crate::error::DiscordError;
use crate::voice_devices::{DeviceKind, VoiceDevices};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc};

pub struct DiscordWorker {
    discord_handler: DiscordStateHandler,
    /// Shared with the state machine so reading the current voice state does not need a
    /// round-trip through the actor mailbox.
    voice_settings: Arc<Mutex<DiscordVoiceSettings>>,
    /// Also shared, and for a sharper reason: a connection attempt waits on the user clicking
    /// Discord's authorisation modal, and asking the actor during that window would block until
    /// the call timed out.
    connected: Arc<AtomicBool>,
}

impl DiscordWorker {
    /// Creates the worker. `credentials` is `None` until the user registers a Discord
    /// application; the worker stays idle in that case and starts on [`Self::set_credentials`].
    ///
    /// Changes are announced on `observer` as they happen; nothing has to poll this worker.
    pub async fn new(
        credentials: Option<DiscordCredentials>,
        observer: mpsc::UnboundedSender<DiscordWorkerEvent>,
    ) -> Self {
        let voice_settings = Arc::new(Mutex::new(DiscordVoiceSettings::default()));
        let connected = Arc::new(AtomicBool::new(false));
        let discord_handler = DiscordState::spawn(
            credentials,
            voice_settings.clone(),
            observer,
            connected.clone(),
        )
        .await;

        Self {
            discord_handler,
            voice_settings,
            connected,
        }
    }

    /// Begins connecting. Safe to call with no credentials: it is then a no-op.
    pub async fn start(&mut self) -> Result<(), DiscordError> {
        self.discord_handler
            .connect()
            .map_err(DiscordError::ActorError)
    }

    /// Applies credentials entered at runtime. Passing `None` stops the connection.
    pub async fn set_credentials(
        &mut self,
        credentials: Option<DiscordCredentials>,
    ) -> Result<(), DiscordError> {
        self.discord_handler
            .set_credentials(credentials)
            .map_err(DiscordError::ActorError)
    }

    pub async fn get_voice_settings(&self) -> DiscordVoiceSettings {
        *self.voice_settings.lock().await
    }

    pub async fn set_voice_settings(
        &mut self,
        mute: bool,
        deafen: bool,
    ) -> Result<(), DiscordError> {
        self.discord_handler
            .set_voice_setting(mute, deafen)
            .map_err(DiscordError::ActorError)
    }

    /// The audio devices Discord offers and the ones in use. Fails while not connected.
    pub async fn voice_devices(&self) -> Result<VoiceDevices, DiscordError> {
        self.discord_handler
            .voice_devices()
            .await
            .map_err(DiscordError::ActorError)?
    }

    /// Switches Discord's microphone or output.
    pub async fn set_voice_device(
        &self,
        kind: DeviceKind,
        device_id: String,
    ) -> Result<(), DiscordError> {
        self.discord_handler
            .set_voice_device(kind, device_id)
            .await
            .map_err(DiscordError::ActorError)?
    }

    /// Whether an authenticated RPC session is open.
    ///
    /// Reads a shared flag rather than messaging the actor, so it answers even while a connection
    /// attempt is parked on the user clicking Discord's authorisation modal.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub async fn disconnect(&mut self) -> Result<(), DiscordError> {
        self.discord_handler
            .disconnect_channel()
            .map_err(DiscordError::ActorError)
    }

    pub async fn get_access_token(&mut self) -> Result<Option<String>, DiscordError> {
        self.discord_handler
            .access_token()
            .await
            .map_err(DiscordError::ActorError)
    }

    pub async fn get_refresh_token(&mut self) -> Result<Option<String>, DiscordError> {
        self.discord_handler
            .refresh_token()
            .await
            .map_err(DiscordError::ActorError)
    }

    pub async fn shutdown(&mut self) -> Result<(), DiscordError> {
        self.discord_handler
            .shutdown()
            .await
            .map_err(DiscordError::ActorError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn a_worker_without_credentials_reports_itself_disconnected() {
        let (observer, _observed) = mpsc::unbounded_channel();
        let mut worker = DiscordWorker::new(None, observer).await;

        worker.start().await.expect("start is accepted");

        assert!(!worker.is_connected());
        assert!(worker.get_access_token().await.expect("readable").is_none());
    }

    #[tokio::test]
    async fn an_idle_worker_never_reports_a_connection_on_its_own() {
        // Guards the "stay parked without credentials" behaviour: a regression here would mean
        // burning CPU retrying a connection that cannot succeed.
        let (observer, _observed) = mpsc::unbounded_channel();
        let mut worker = DiscordWorker::new(None, observer).await;
        worker.start().await.expect("start is accepted");

        sleep(Duration::from_millis(300)).await;

        assert!(!worker.is_connected());
        assert_eq!(
            worker.get_voice_settings().await,
            DiscordVoiceSettings::default()
        );
    }

    /// Waits for an authenticated session, giving up rather than hanging forever.
    ///
    /// The first run needs a human to accept Discord's authorisation modal, so the window is
    /// generous — but a test that never returns tells you nothing.
    async fn wait_until_connected(worker: &DiscordWorker) {
        let deadline = std::time::Instant::now() + Duration::from_secs(150);
        while std::time::Instant::now() < deadline {
            if worker.is_connected() {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("no authenticated session after 150 s — was the authorisation modal accepted?");
    }

    /// Installs a log subscriber so `RUST_LOG` works in these tests.
    ///
    /// Without it the interactive tests are silent, and a connection stuck waiting on the
    /// authorisation modal is indistinguishable from one failing for any other reason.
    fn init_logging() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "discord=debug".into()),
            )
            .with_test_writer()
            .try_init();
    }

    /// Where the interactive tests cache the tokens they obtain.
    ///
    /// Discord shows its authorisation modal on every AUTHORIZE, so without a cache each run of
    /// these tests needs a human to click again — which makes iterating on them impractical. The
    /// real app has the same need and solves it with the OS keyring; a gitignored file next to
    /// `discord.env`, which already holds the client secret in the clear, is enough here.
    fn token_cache_path() -> PathBuf {
        PathBuf::from("../../../../discord-tokens.json")
    }

    fn cached_tokens() -> (Option<String>, Option<String>) {
        let Ok(raw) = std::fs::read_to_string(token_cache_path()) else {
            return (None, None);
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return (None, None);
        };
        (
            parsed["access_token"].as_str().map(str::to_owned),
            parsed["refresh_token"].as_str().map(str::to_owned),
        )
    }

    /// Saves whatever the connection produced, so the next run skips the modal.
    async fn cache_tokens(worker: &mut DiscordWorker) {
        let access = worker.get_access_token().await.ok().flatten();
        let refresh = worker.get_refresh_token().await.ok().flatten();
        if access.is_none() {
            return;
        }
        let json = serde_json::json!({ "access_token": access, "refresh_token": refresh });
        let _ = std::fs::write(token_cache_path(), json.to_string());
    }

    /// Reads the developer's own Discord application credentials, the same way a user supplies
    /// theirs through the UI.
    ///
    /// Cached tokens are attached when present, which is what lets a second run connect without
    /// anyone touching Discord.
    fn credentials_from_env_file() -> DiscordCredentials {
        let file = File::open(PathBuf::from("../../../../discord.env")).expect("discord.env");
        for line in BufReader::new(file).lines() {
            let line = line.expect("readable line");
            if line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                unsafe { std::env::set_var(key, value) };
            }
        }

        let (access, refresh) = cached_tokens();
        DiscordCredentials::new(
            std::env::var("DISCORD_CLIENT_ID").expect("DISCORD_CLIENT_ID"),
            std::env::var("DISCORD_SECRET_KEY").expect("DISCORD_SECRET_KEY"),
            "http://localhost/".to_owned(),
        )
        .with_tokens(access, refresh)
    }

    #[tokio::test]
    #[ignore = "needs a running Discord client and a local discord.env; run with --ignored"]
    async fn credentials_supplied_at_runtime_drive_a_full_connection() {
        init_logging();
        let (observer, _observed) = mpsc::unbounded_channel();
        let mut worker = DiscordWorker::new(None, observer).await;
        worker.start().await.expect("start is accepted");

        // Nothing should happen until credentials arrive: this is the state a fresh install is
        // in, and it must not connect on its own.
        sleep(Duration::from_secs(1)).await;
        assert!(!worker.is_connected());

        worker
            .set_credentials(Some(credentials_from_env_file()))
            .await
            .expect("credentials accepted");

        wait_until_connected(&worker).await;
        cache_tokens(&mut worker).await;

        worker
            .set_voice_settings(true, false)
            .await
            .expect("voice settings accepted");
        sleep(Duration::from_secs(1)).await;
        assert!(worker.get_voice_settings().await.mute);

        worker
            .set_voice_settings(false, false)
            .await
            .expect("voice settings accepted");
        sleep(Duration::from_secs(1)).await;
        assert!(!worker.get_voice_settings().await.mute);

        worker.shutdown().await.expect("shuts down");
    }

    /// Checks that Discord takes `input.device_id` / `output.device_id` in `SET_VOICE_SETTINGS`, by
    /// switching each side to another device it lists and back.
    ///
    /// Audible: for a moment Discord uses another microphone and another output.
    #[tokio::test]
    #[ignore = "switches Discord's audio devices for a moment; run with --ignored"]
    async fn discord_switches_audio_devices_on_request() {
        use crate::voice_devices::DeviceKind;

        init_logging();
        let (observer, _observed) = mpsc::unbounded_channel();
        let mut worker = DiscordWorker::new(Some(credentials_from_env_file()), observer).await;
        worker.start().await.expect("start is accepted");
        wait_until_connected(&worker).await;
        cache_tokens(&mut worker).await;

        for kind in [DeviceKind::Input, DeviceKind::Output] {
            let before = worker.voice_devices().await.expect("devices readable");
            let list = before.of(kind);
            let Some(other) = list
                .available
                .iter()
                .find(|device| device.id != list.current)
            else {
                tracing::warn!("Only one {} device, nothing to switch to", kind.key());
                continue;
            };

            worker
                .set_voice_device(kind, other.id.clone())
                .await
                .expect("Discord accepts the device");
            let after = worker.voice_devices().await.expect("devices readable");
            assert_eq!(after.of(kind).current, other.id, "{} switched", kind.key());

            worker
                .set_voice_device(kind, list.current.clone())
                .await
                .expect("Discord accepts the original device back");
            let restored = worker.voice_devices().await.expect("devices readable");
            assert_eq!(restored.of(kind).current, list.current);
        }

        worker.shutdown().await.expect("shuts down");
    }

    /// Exercises the event path rather than the command path: with the subscription in place,
    /// muting from Discord's own UI must reach the app without it polling anything.
    ///
    /// This is the empirical check that `VOICE_SETTINGS_UPDATE` actually fires, which the whole
    /// event-driven design depends on.
    #[tokio::test]
    #[ignore = "interactive: toggle mute in Discord within 30 seconds; run with --ignored"]
    async fn muting_from_discord_reaches_the_app_as_a_pushed_event() {
        init_logging();
        let (observer, _observed) = mpsc::unbounded_channel();
        let mut worker = DiscordWorker::new(Some(credentials_from_env_file()), observer).await;
        worker.start().await.expect("start is accepted");

        wait_until_connected(&worker).await;
        cache_tokens(&mut worker).await;

        let initial = worker.get_voice_settings().await;
        println!("Connected. Toggle mute in Discord now — waiting up to 30 s...");

        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if worker.get_voice_settings().await != initial {
                worker.shutdown().await.expect("shuts down");
                return;
            }
            sleep(Duration::from_millis(200)).await;
        }

        panic!("no VOICE_SETTINGS_UPDATE arrived within 30 s — the subscription is not working");
    }
}
