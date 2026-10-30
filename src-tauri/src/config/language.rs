//! UI language: which one is in effect, and the strings the backend itself has to render.
//!
//! Almost all user-facing text lives in the frontend. The tray menu and the desktop notifications
//! do not — both exist before any window does when the app starts minimised — so their text is
//! read here, from the very same translation files the frontend bundles. One file per language,
//! two consumers.

use serde::Deserialize;
use std::sync::LazyLock;
use tracing::warn;

/// Languages the UI ships with.
///
/// An enum rather than a string: an unknown tag can then only enter through `from_tag`, which
/// resolves it, instead of reaching the frontend and quietly rendering nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Language {
    /// The fallback, and the language whose translation file has to be complete.
    #[default]
    English,
    Spanish,
}

impl Language {
    pub const fn tag(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Spanish => "es",
        }
    }

    /// Matches a BCP 47 tag, by its primary subtag so that `es-AR` and `es-419` are Spanish.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let primary = tag.split(['-', '_']).next().unwrap_or_default();
        match primary.to_ascii_lowercase().as_str() {
            "en" => Some(Language::English),
            "es" => Some(Language::Spanish),
            _ => None,
        }
    }

    fn strings(self) -> &'static Strings {
        match self {
            Language::English => &TRANSLATIONS.en,
            Language::Spanish => &TRANSLATIONS.es,
        }
    }

    pub fn tray_show(self) -> &'static str {
        &self.strings().tray.show
    }

    pub fn tray_quit(self) -> &'static str {
        &self.strings().tray.quit
    }

    /// Text for the desktop notifications.
    pub fn notifications(self) -> &'static NotificationStrings {
        &self.strings().notifications
    }
}

/// Decides which language to use.
///
/// A stored preference wins. Otherwise the system's language is used when the UI has it, and
/// English is the fallback for everything else.
pub fn resolve(stored: Option<&str>) -> Language {
    if let Some(tag) = stored {
        match Language::from_tag(tag) {
            Some(language) => return language,
            // Reachable by hand-editing the config file, which is plain JSON on purpose.
            None => warn!("Configured language {tag:?} is not one the UI ships with, ignoring it"),
        }
    }

    sys_locale::get_locale()
        .and_then(|locale| Language::from_tag(&locale))
        .unwrap_or_default()
}

/// The subset of a translation file the backend reads.
///
/// Everything else in the file is ignored: adding a key for the frontend must not mean touching
/// Rust, so unknown fields are simply not named here.
#[derive(Deserialize)]
struct TrayStrings {
    show: String,
    quit: String,
}

/// A desktop notification's title and body.
#[derive(Deserialize)]
pub struct NotificationText {
    pub title: String,
    pub body: String,
}

/// The `notifications` section of a translation file.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationStrings {
    pub device_disconnected: NotificationText,
    pub device_connected: NotificationText,
    pub device_firmware_outdated: NotificationText,
    pub device_app_outdated: NotificationText,
    pub discord_disconnected: NotificationText,
    pub discord_connected: NotificationText,
    pub discord_awaiting: NotificationText,
    /// Title when Shift + mute switched Discord's microphone; the body is the device's name.
    pub input_device: String,
    /// Title when Shift + deafen switched Discord's output; the body is the device's name.
    pub output_device: String,
}

#[derive(Deserialize)]
struct Strings {
    tray: TrayStrings,
    notifications: NotificationStrings,
}

struct Catalogs {
    en: Strings,
    es: Strings,
}

// Embedded from the frontend's translation files so that the tray and the window cannot drift
// apart: there is one file per language and both sides read it.
static EN_JSON: &str = include_str!("../../frontend/translations/en.json");
static ES_JSON: &str = include_str!("../../frontend/translations/es.json");

static TRANSLATIONS: LazyLock<Catalogs> = LazyLock::new(|| Catalogs {
    en: parse(EN_JSON, "en"),
    es: parse(ES_JSON, "es"),
});

/// Panics on a malformed translation file.
///
/// The files are compiled into the binary, so a failure here is a build that should never have
/// shipped rather than anything a user can cause. A test parses both to catch it before then.
fn parse(json: &str, tag: &str) -> Strings {
    serde_json::from_str(json)
        .unwrap_or_else(|err| panic!("translations/{tag}.json is not valid: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_translation_files_parse_and_have_tray_labels() {
        // The files are include_str!'d, so a typo in either is a panic at first use rather than a
        // compile error. This is what turns that into a failing test.
        for language in [Language::English, Language::Spanish] {
            assert!(!language.tray_show().is_empty(), "{language:?} show");
            assert!(!language.tray_quit().is_empty(), "{language:?} quit");
        }

        assert_eq!(Language::English.tray_quit(), "Quit");
        assert_eq!(Language::Spanish.tray_quit(), "Salir");
    }

    /// Collects every leaf path, e.g. `discord.connect`.
    fn key_paths(value: &serde_json::Value, prefix: &str, into: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    key_paths(child, &path, into);
                }
            }
            _ => into.push(prefix.to_owned()),
        }
    }

    #[test]
    fn every_language_defines_exactly_the_same_keys() {
        // A key missing from one file falls back to English at runtime, silently and only on the
        // screen nobody opened while testing. This is the one i18n bug worth a test.
        let mut english = Vec::new();
        let mut spanish = Vec::new();
        key_paths(
            &serde_json::from_str(EN_JSON).expect("en.json parses"),
            "",
            &mut english,
        );
        key_paths(
            &serde_json::from_str(ES_JSON).expect("es.json parses"),
            "",
            &mut spanish,
        );
        english.sort();
        spanish.sort();

        let missing_in_spanish: Vec<_> = english.iter().filter(|k| !spanish.contains(k)).collect();
        let missing_in_english: Vec<_> = spanish.iter().filter(|k| !english.contains(k)).collect();

        assert!(
            missing_in_spanish.is_empty(),
            "keys absent from es.json: {missing_in_spanish:?}"
        );
        assert!(
            missing_in_english.is_empty(),
            "keys absent from en.json: {missing_in_english:?}"
        );
    }

    #[test]
    fn no_translation_is_left_empty() {
        // An empty string passes the key comparison above but renders as a blank label.
        for (json, tag) in [(EN_JSON, "en"), (ES_JSON, "es")] {
            let mut paths = Vec::new();
            let value: serde_json::Value = serde_json::from_str(json).expect("parses");
            key_paths(&value, "", &mut paths);

            for path in paths {
                let text = path
                    .split('.')
                    .fold(&value, |node, part| &node[part])
                    .as_str()
                    .unwrap_or_else(|| panic!("{tag}.json: {path} is not a string"));
                assert!(!text.trim().is_empty(), "{tag}.json: {path} is empty");
            }
        }
    }

    #[test]
    fn regional_tags_resolve_to_their_language() {
        assert_eq!(Language::from_tag("es-AR"), Some(Language::Spanish));
        assert_eq!(Language::from_tag("es_ES"), Some(Language::Spanish));
        assert_eq!(Language::from_tag("en-GB"), Some(Language::English));
        assert_eq!(Language::from_tag("ES"), Some(Language::Spanish));
        assert_eq!(Language::from_tag("pt-BR"), None);
        assert_eq!(Language::from_tag(""), None);
    }

    #[test]
    fn a_stored_preference_wins_over_the_system() {
        assert_eq!(resolve(Some("es")), Language::Spanish);
        assert_eq!(resolve(Some("en")), Language::English);
    }

    #[test]
    fn an_unusable_stored_language_falls_back_instead_of_failing() {
        // Hand-editing the config to something unsupported must not leave the UI blank. The
        // result depends on the machine's locale, so this only asserts that one is chosen.
        let resolved = resolve(Some("tlh"));
        assert!(matches!(resolved, Language::English | Language::Spanish));
    }
}
