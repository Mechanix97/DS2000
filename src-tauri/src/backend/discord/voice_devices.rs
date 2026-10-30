//! Discord's audio devices: which ones it offers, which one is in use, and which comes next.
//!
//! `GET_VOICE_SETTINGS` carries an `input` and an `output` object, each with the `device_id` in
//! use and the `available_devices` Discord can pick from. `SET_VOICE_SETTINGS` takes the same
//! `device_id` back to switch.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::DiscordError;

/// Id of Discord's "Default" entry, which follows the system's default device. It is always the
/// same hardware as another entry in the list, so switching to it changes nothing audible.
const SYSTEM_DEFAULT_ID: &str = "default";

/// Microphone or headphones.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Input,
    Output,
}

impl DeviceKind {
    /// The key Discord uses for this side in voice settings payloads.
    pub const fn key(self) -> &'static str {
        match self {
            DeviceKind::Input => "input",
            DeviceKind::Output => "output",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

/// One side of Discord's audio settings.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DeviceList {
    /// Id of the device Discord uses now.
    pub current: String,
    pub available: Vec<AudioDevice>,
    /// Id of the device the system uses by default, when Discord says which.
    pub system_default: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct VoiceDevices {
    pub input: DeviceList,
    pub output: DeviceList,
}

impl VoiceDevices {
    pub fn of(&self, kind: DeviceKind) -> &DeviceList {
        match kind {
            DeviceKind::Input => &self.input,
            DeviceKind::Output => &self.output,
        }
    }
}

/// Reads both device lists out of a voice settings payload.
pub fn parse_voice_devices(data: &Value) -> Result<VoiceDevices, DiscordError> {
    Ok(VoiceDevices {
        input: parse_device_list(&data[DeviceKind::Input.key()])?,
        output: parse_device_list(&data[DeviceKind::Output.key()])?,
    })
}

fn parse_device_list(side: &Value) -> Result<DeviceList, DiscordError> {
    let current = side["device_id"]
        .as_str()
        .ok_or(DiscordError::NoDataFound)?
        .to_owned();
    let listed = side["available_devices"]
        .as_array()
        .ok_or(DiscordError::NoDataFound)?;
    let default_name = listed
        .iter()
        .find(|device| device["id"].as_str() == Some(SYSTEM_DEFAULT_ID))
        .and_then(|device| device["name"].as_str())
        .and_then(system_default_name);
    let available: Vec<AudioDevice> = listed
        .iter()
        // A device without an id cannot be selected, so it is not worth offering. Nor is the
        // system default: it duplicates a real device.
        .filter_map(|device| {
            let id = device["id"]
                .as_str()
                .filter(|id| *id != SYSTEM_DEFAULT_ID)?;
            Some(AudioDevice {
                id: id.to_owned(),
                name: device["name"].as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect();
    let system_default = default_name.and_then(|name| {
        available
            .iter()
            .find(|device| device.name == name)
            .map(|device| device.id.clone())
    });
    Ok(DeviceList {
        current,
        available,
        system_default,
    })
}

/// The device a "Default" entry stands for, read from its name: Discord calls it
/// `Default (<device name>)`, with the first word in Discord's own language.
fn system_default_name(name: &str) -> Option<&str> {
    name.split_once(" (")?.1.strip_suffix(')')
}

/// The device to switch to: the one after `current` among those the user chose, in the order
/// they were chosen, skipping any Discord does not offer right now (unplugged).
///
/// No choices means every device Discord offers, in its order: that is the default, so a device
/// plugged in later is included without visiting Settings.
///
/// When `current` is not one of them, the first available choice. `None` when there is nothing to
/// switch to: no choice is available, or the only one is already in use.
pub fn next_device<'a>(list: &'a DeviceList, chosen: &[String]) -> Option<&'a AudioDevice> {
    let candidates: Vec<&AudioDevice> = if chosen.is_empty() {
        list.available.iter().collect()
    } else {
        chosen
            .iter()
            .filter_map(|id| list.available.iter().find(|device| device.id == *id))
            .collect()
    };

    let next = match candidates
        .iter()
        .position(|device| device.id == list.current)
    {
        Some(index) => candidates[(index + 1) % candidates.len()],
        None => *candidates.first()?,
    };
    (next.id != list.current).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn device(id: &str) -> AudioDevice {
        AudioDevice {
            id: id.to_owned(),
            name: format!("Device {id}"),
        }
    }

    fn list(current: &str, available: &[&str]) -> DeviceList {
        DeviceList {
            current: current.to_owned(),
            available: available.iter().map(|id| device(id)).collect(),
            system_default: None,
        }
    }

    fn chosen(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn devices_are_read_out_of_a_voice_settings_payload() {
        let data = json!({
            "mute": false,
            "deaf": false,
            "input": {
                "device_id": "default",
                "volume": 100,
                "available_devices": [
                    { "id": "default", "name": "Default (Desk mic)" },
                    { "id": "{mic}", "name": "Desk mic" },
                ],
            },
            "output": {
                "device_id": "{hp}",
                "volume": 100,
                "available_devices": [{ "id": "{hp}", "name": "Headphones" }],
            },
        });

        let devices = parse_voice_devices(&data).expect("parses");

        assert_eq!(devices.input.current, "default");
        // The system default is left out: it is the same hardware as another entry.
        assert_eq!(devices.input.available.len(), 1);
        assert_eq!(devices.input.system_default.as_deref(), Some("{mic}"));
        assert_eq!(
            devices.input.available[0],
            AudioDevice {
                id: "{mic}".to_owned(),
                name: "Desk mic".to_owned()
            }
        );
        assert_eq!(devices.of(DeviceKind::Output).current, "{hp}");
        assert_eq!(devices.output.available.len(), 1);
    }

    #[test]
    fn the_system_default_is_read_from_the_default_entrys_name() {
        assert_eq!(
            system_default_name("Default (Microphone (4- Logitech G933 Gaming Wireless Headset))"),
            Some("Microphone (4- Logitech G933 Gaming Wireless Headset)")
        );
        // Discord in another language names the entry in it.
        assert_eq!(
            system_default_name("Predeterminado (Altavoces)"),
            Some("Altavoces")
        );
        assert_eq!(system_default_name("Default"), None);
    }

    #[test]
    fn a_payload_without_devices_is_an_error_not_an_empty_list() {
        assert!(parse_voice_devices(&json!({ "mute": true, "deaf": false })).is_err());
    }

    #[test]
    fn the_next_chosen_device_follows_the_current_one_and_wraps() {
        let devices = list("b", &["a", "b", "c", "hdmi"]);
        let order = chosen(&["a", "b", "c"]);

        assert_eq!(next_device(&devices, &order), Some(&device("c")));
        assert_eq!(
            next_device(&list("c", &["a", "b", "c"]), &order),
            Some(&device("a"))
        );
    }

    #[test]
    fn the_order_is_the_users_not_discords() {
        let devices = list("a", &["a", "b", "c"]);
        assert_eq!(
            next_device(&devices, &chosen(&["a", "c", "b"])),
            Some(&device("c"))
        );
    }

    #[test]
    fn an_unchosen_current_device_moves_to_the_first_choice() {
        let devices = list("hdmi", &["a", "b", "hdmi"]);
        assert_eq!(
            next_device(&devices, &chosen(&["b", "a"])),
            Some(&device("b"))
        );
    }

    #[test]
    fn an_unplugged_choice_is_skipped() {
        let devices = list("a", &["a", "c"]);
        assert_eq!(
            next_device(&devices, &chosen(&["a", "b", "c"])),
            Some(&device("c"))
        );
    }

    #[test]
    fn the_list_changing_under_the_current_device_still_finds_a_choice() {
        // The device in use was unplugged and Discord fell back to one the user did not choose.
        let devices = list("default", &["default", "c"]);
        assert_eq!(
            next_device(&devices, &chosen(&["a", "c"])),
            Some(&device("c"))
        );
    }

    #[test]
    fn no_choices_go_through_every_device_discord_offers() {
        let devices = list("b", &["a", "b", "c"]);
        assert_eq!(next_device(&devices, &[]), Some(&device("c")));
        assert_eq!(
            next_device(&list("c", &["a", "b", "c"]), &[]),
            Some(&device("a"))
        );
    }

    #[test]
    fn nothing_to_switch_to_is_none() {
        // Discord offers a single device.
        assert_eq!(next_device(&list("a", &["a"]), &[]), None);
        // Every choice unplugged.
        assert_eq!(next_device(&list("a", &["a"]), &chosen(&["x", "y"])), None);
        // The only available choice is already in use.
        assert_eq!(
            next_device(&list("a", &["a", "b"]), &chosen(&["a", "x"])),
            None
        );
    }
}
