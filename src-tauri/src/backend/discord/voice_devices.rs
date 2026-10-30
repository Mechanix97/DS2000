//! Discord's audio devices: which ones it offers, which one is in use, and which comes next.
//!
//! `GET_VOICE_SETTINGS` carries an `input` and an `output` object, each with the `device_id` in
//! use and the `available_devices` Discord can pick from. `SET_VOICE_SETTINGS` takes the same
//! `device_id` back to switch.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::DiscordError;

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
    let available = side["available_devices"]
        .as_array()
        .ok_or(DiscordError::NoDataFound)?
        .iter()
        // A device without an id cannot be selected, so it is not worth offering.
        .filter_map(|device| {
            Some(AudioDevice {
                id: device["id"].as_str()?.to_owned(),
                name: device["name"].as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect();
    Ok(DeviceList { current, available })
}

/// The device to switch to: the one after `current` among those the user chose, in the order
/// they were chosen, skipping any Discord does not offer right now (unplugged).
///
/// When `current` is not one of them, the first available choice. `None` when there is nothing to
/// switch to: no choice is available, or the only one is already in use.
pub fn next_device<'a>(list: &'a DeviceList, chosen: &[String]) -> Option<&'a AudioDevice> {
    let candidates: Vec<&AudioDevice> = chosen
        .iter()
        .filter_map(|id| list.available.iter().find(|device| device.id == *id))
        .collect();

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
                    { "id": "default", "name": "Default" },
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
        assert_eq!(
            devices.input.available[1],
            AudioDevice {
                id: "{mic}".to_owned(),
                name: "Desk mic".to_owned()
            }
        );
        assert_eq!(devices.of(DeviceKind::Output).current, "{hp}");
        assert_eq!(devices.output.available.len(), 1);
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
    fn nothing_to_switch_to_is_none() {
        // No choices.
        assert_eq!(next_device(&list("a", &["a", "b"]), &[]), None);
        // Every choice unplugged.
        assert_eq!(next_device(&list("a", &["a"]), &chosen(&["x", "y"])), None);
        // The only available choice is already in use.
        assert_eq!(
            next_device(&list("a", &["a", "b"]), &chosen(&["a", "x"])),
            None
        );
    }
}
