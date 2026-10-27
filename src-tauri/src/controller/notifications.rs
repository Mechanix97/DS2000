//! Desktop notifications while the window is not on screen.
//!
//! The status notices in the sidebar only reach someone looking at the window, and with "start
//! minimised" there is no window at all, so the device or Discord dropping goes unnoticed until a
//! key does nothing. These are the same problems, told through the operating system instead.
//!
//! Three parts:
//! - [`NoticeTracker`] decides what is worth a notification and when. It is plain state, so the
//!   rules are tested without a window or a notification server.
//! - [`DesktopNotifier`] is how a notification is shown. [`SystemNotifier`] uses
//!   `tauri-plugin-notification`: toasts on Windows, Notification Center on macOS, the freedesktop
//!   D-Bus service on Linux. A platform that needs something else gets its own implementation.
//! - The coordinator feeds the tracker and, when one is due, shows it unless the window is up or
//!   the user turned notifications off.

use crate::coordinator::SerialState;

use config::language::Language;
use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;
use tokio::time::{Duration, Instant};

/// How long a lost connection has to last before it is worth a notification. Discord restarting
/// to update, or a cable reseated, comes back within it.
const GRACE: Duration = Duration::from_secs(10);

/// Something the user is told about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Notice {
    DeviceDisconnected,
    /// Back after [`Notice::DeviceDisconnected`] was shown.
    DeviceConnected,
    DeviceFirmwareOutdated,
    DeviceAppOutdated,
    DiscordDisconnected,
    /// Back after [`Notice::DiscordDisconnected`] was shown.
    DiscordConnected,
    DiscordAwaitingAuthorization,
}

impl Notice {
    /// Title and body, in `language`.
    pub fn text(self, language: Language) -> (&'static str, &'static str) {
        let strings = language.notifications();
        let text = match self {
            Notice::DeviceDisconnected => &strings.device_disconnected,
            Notice::DeviceConnected => &strings.device_connected,
            Notice::DeviceFirmwareOutdated => &strings.device_firmware_outdated,
            Notice::DeviceAppOutdated => &strings.device_app_outdated,
            Notice::DiscordDisconnected => &strings.discord_disconnected,
            Notice::DiscordConnected => &strings.discord_connected,
            Notice::DiscordAwaitingAuthorization => &strings.discord_awaiting,
        };
        (&text.title, &text.body)
    }
}

/// Shows a notification through the operating system.
pub trait DesktopNotifier: Send {
    fn show(&self, title: &str, body: &str) -> Result<(), String>;
}

/// The operating system's own notifications, through `tauri-plugin-notification`.
pub struct SystemNotifier<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> DesktopNotifier for SystemNotifier<R> {
    fn show(&self, title: &str, body: &str) -> Result<(), String> {
        self.0
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|err| err.to_string())
    }
}

/// Turns state changes into notifications.
///
/// Only a change from working to broken counts: the app starting with no device or with Discord
/// closed is how it is used, not news. A loss waits out [`GRACE`] and is dropped if it recovers.
/// Recovering is only news to someone who was told about the loss.
#[derive(Default)]
pub struct NoticeTracker {
    device: SerialState,
    device_was_connected: bool,
    discord_connected: bool,
    discord_was_connected: bool,
    /// `None` until the Discord worker has looked.
    discord_running: Option<bool>,
    /// Whether the loss was shown, so its recovery is too.
    device_loss_shown: bool,
    discord_loss_shown: bool,
    pending: Vec<(Notice, Instant)>,
}

impl NoticeTracker {
    pub fn on_device(&mut self, state: SerialState, now: Instant) {
        self.device = state;
        self.cancel(|notice| {
            matches!(
                notice,
                Notice::DeviceDisconnected
                    | Notice::DeviceConnected
                    | Notice::DeviceFirmwareOutdated
                    | Notice::DeviceAppOutdated
            )
        });
        match state {
            SerialState::Connected => {
                self.device_was_connected = true;
                if std::mem::take(&mut self.device_loss_shown) {
                    self.schedule(Notice::DeviceConnected, now);
                }
            }
            SerialState::Disconnected if self.device_was_connected => {
                self.device_was_connected = false;
                self.schedule(Notice::DeviceDisconnected, now + GRACE);
            }
            SerialState::Disconnected => {}
            // Nothing works until one side is updated, so it is news even at start.
            SerialState::FirmwareOutdated => self.schedule(Notice::DeviceFirmwareOutdated, now),
            SerialState::AppOutdated => self.schedule(Notice::DeviceAppOutdated, now),
        }
    }

    pub fn on_discord_connection(&mut self, connected: bool, now: Instant) {
        self.discord_connected = connected;
        if connected {
            self.discord_was_connected = true;
            self.cancel(|notice| {
                matches!(
                    notice,
                    Notice::DiscordDisconnected | Notice::DiscordAwaitingAuthorization
                )
            });
            if std::mem::take(&mut self.discord_loss_shown) {
                self.schedule(Notice::DiscordConnected, now);
            }
        } else if self.discord_was_connected {
            self.discord_was_connected = false;
            self.schedule(Notice::DiscordDisconnected, now + GRACE);
        }
    }

    pub fn on_discord_running(&mut self, running: bool) {
        self.discord_running = Some(running);
    }

    /// Discord put its authorisation modal up and waits for the user, who may not be looking.
    pub fn on_awaiting_authorization(&mut self, now: Instant) {
        self.schedule(Notice::DiscordAwaitingAuthorization, now);
    }

    /// Records that `notice` reached the user.
    pub fn shown(&mut self, notice: Notice) {
        match notice {
            Notice::DeviceDisconnected => self.device_loss_shown = true,
            Notice::DiscordDisconnected => self.discord_loss_shown = true,
            _ => {}
        }
    }

    /// When the next notification is due, for the coordinator to wake up then.
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.iter().map(|(_, due)| *due).min()
    }

    /// Takes the notifications due by `now` whose problem is still there.
    pub fn take_due(&mut self, now: Instant) -> Vec<Notice> {
        let (due, later): (Vec<_>, Vec<_>) = self.pending.drain(..).partition(|(_, at)| *at <= now);
        self.pending = later;
        due.into_iter()
            .map(|(notice, _)| notice)
            .filter(|notice| self.still_true(*notice))
            .collect()
    }

    fn still_true(&self, notice: Notice) -> bool {
        match notice {
            Notice::DeviceDisconnected => self.device == SerialState::Disconnected,
            Notice::DeviceConnected => self.device == SerialState::Connected,
            Notice::DeviceFirmwareOutdated => self.device == SerialState::FirmwareOutdated,
            Notice::DeviceAppOutdated => self.device == SerialState::AppOutdated,
            // Closing Discord is the user's own doing, not a lost connection.
            Notice::DiscordDisconnected => {
                !self.discord_connected && self.discord_running != Some(false)
            }
            Notice::DiscordConnected => self.discord_connected,
            Notice::DiscordAwaitingAuthorization => !self.discord_connected,
        }
    }

    fn schedule(&mut self, notice: Notice, at: Instant) {
        if !self.pending.iter().any(|(pending, _)| *pending == notice) {
            self.pending.push((notice, at));
        }
    }

    fn cancel(&mut self, matches: impl Fn(Notice) -> bool) {
        self.pending.retain(|(notice, _)| !matches(*notice));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn later(now: Instant) -> Instant {
        now + GRACE
    }

    #[test]
    fn nothing_is_reported_for_how_the_app_starts() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_device(SerialState::Disconnected, now);
        tracker.on_discord_running(false);
        tracker.on_discord_connection(false, now);
        assert_eq!(tracker.next_due(), None);
    }

    #[test]
    fn a_device_that_stays_unplugged_is_reported_once() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_device(SerialState::Connected, now);
        tracker.on_device(SerialState::Disconnected, now);
        assert_eq!(tracker.take_due(now), []);
        assert_eq!(tracker.take_due(later(now)), [Notice::DeviceDisconnected]);
        // Still unplugged: no repeat until it has been connected again.
        tracker.on_device(SerialState::Disconnected, later(now));
        assert_eq!(tracker.next_due(), None);
    }

    #[test]
    fn a_device_back_within_the_grace_is_not_reported() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_device(SerialState::Connected, now);
        tracker.on_device(SerialState::Disconnected, now);
        tracker.on_device(SerialState::Connected, now);
        assert_eq!(tracker.take_due(later(now)), []);
    }

    #[test]
    fn a_device_back_is_reported_only_after_its_loss_was() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_device(SerialState::Connected, now);
        tracker.on_device(SerialState::Disconnected, now);
        for notice in tracker.take_due(later(now)) {
            tracker.shown(notice);
        }
        tracker.on_device(SerialState::Connected, later(now));
        assert_eq!(tracker.take_due(later(now)), [Notice::DeviceConnected]);

        // A loss that was not shown (the window was up) is not followed by a recovery.
        tracker.on_device(SerialState::Disconnected, later(now));
        tracker.take_due(later(later(now)));
        tracker.on_device(SerialState::Connected, later(later(now)));
        assert_eq!(tracker.next_due(), None);
    }

    #[test]
    fn discord_back_is_reported_after_its_loss_was() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_discord_connection(true, now);
        tracker.on_discord_connection(false, now);
        tracker.shown(Notice::DiscordDisconnected);
        tracker.on_discord_connection(true, now);
        assert_eq!(tracker.take_due(now), [Notice::DiscordConnected]);
    }

    #[test]
    fn a_protocol_mismatch_is_reported_at_once() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_device(SerialState::FirmwareOutdated, now);
        assert_eq!(tracker.take_due(now), [Notice::DeviceFirmwareOutdated]);
    }

    #[test]
    fn a_lost_discord_connection_is_reported_while_discord_runs() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_discord_running(true);
        tracker.on_discord_connection(true, now);
        tracker.on_discord_connection(false, now);
        assert_eq!(tracker.next_due(), Some(later(now)));
        assert_eq!(tracker.take_due(later(now)), [Notice::DiscordDisconnected]);
    }

    #[test]
    fn closing_discord_is_not_a_lost_connection() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_discord_running(true);
        tracker.on_discord_connection(true, now);
        tracker.on_discord_connection(false, now);
        tracker.on_discord_running(false);
        assert_eq!(tracker.take_due(later(now)), []);
    }

    #[test]
    fn discord_reconnecting_within_the_grace_is_not_reported() {
        let now = Instant::now();
        let mut tracker = NoticeTracker::default();
        tracker.on_discord_connection(true, now);
        tracker.on_discord_connection(false, now);
        tracker.on_discord_connection(true, now);
        assert_eq!(tracker.take_due(later(now)), []);
    }

    #[test]
    fn every_notice_has_text_in_every_language() {
        let notices = [
            Notice::DeviceDisconnected,
            Notice::DeviceConnected,
            Notice::DeviceFirmwareOutdated,
            Notice::DeviceAppOutdated,
            Notice::DiscordDisconnected,
            Notice::DiscordConnected,
            Notice::DiscordAwaitingAuthorization,
        ];
        for language in [Language::English, Language::Spanish] {
            for notice in notices {
                let (title, body) = notice.text(language);
                assert!(
                    !title.is_empty() && !body.is_empty(),
                    "{language:?} {notice:?}"
                );
            }
        }
    }
}
