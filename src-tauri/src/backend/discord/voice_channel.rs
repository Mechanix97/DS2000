//! The voice channel the user is in, and the one to go back to with Shift + disconnect.
//!
//! Discord pushes `VOICE_CHANNEL_SELECT` with the `channel_id` and `guild_id` whenever the user
//! joins, moves or leaves (`channel_id: null`), from the device or from Discord itself.
//! `GET_SELECTED_VOICE_CHANNEL` and `SELECT_VOICE_CHANNEL` answer with the channel object, which
//! also carries its name.

use serde_json::Value;
use tokio::time::{Duration, Instant};

/// How long a channel that was left can still be rejoined. Longer, and Shift + disconnect would
/// drop someone into a conversation they left long ago.
pub const REJOIN_WINDOW: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VoiceChannel {
    pub id: String,
    /// `None` for a DM or group call.
    pub guild_id: Option<String>,
    /// Only in the channel objects Discord answers commands with, not in the event.
    pub name: Option<String>,
}

impl VoiceChannel {
    /// A server channel, as opposed to a DM or group call. Only these are rejoined: joining a
    /// call rings the other people in it.
    pub fn in_guild(&self) -> bool {
        self.guild_id.is_some()
    }
}

/// Reads a `VOICE_CHANNEL_SELECT` event. `None` means the user left voice.
pub fn parse_channel_select(data: &Value) -> Option<VoiceChannel> {
    Some(VoiceChannel {
        id: data["channel_id"].as_str()?.to_owned(),
        guild_id: data["guild_id"].as_str().map(str::to_owned),
        name: None,
    })
}

/// Reads a channel object, as `GET_SELECTED_VOICE_CHANNEL` and `SELECT_VOICE_CHANNEL` answer.
/// `None` for `null`, which is what the first answers when the user is not in voice.
pub fn parse_channel(data: &Value) -> Option<VoiceChannel> {
    Some(VoiceChannel {
        id: data["id"].as_str()?.to_owned(),
        guild_id: data["guild_id"].as_str().map(str::to_owned),
        name: data["name"].as_str().map(str::to_owned),
    })
}

/// What Shift + disconnect should do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Rejoin {
    /// Join this channel.
    Channel(VoiceChannel),
    /// The user is in a channel already. Moving them out of it is not a shortcut's job.
    AlreadyInChannel,
    /// No server channel was left within [`REJOIN_WINDOW`].
    NothingRecent,
}

/// Follows the user in and out of voice channels.
#[derive(Default, Debug)]
pub struct ChannelMemory {
    current: Option<VoiceChannel>,
    left: Option<(VoiceChannel, Instant)>,
}

impl ChannelMemory {
    /// Records what Discord reported: the channel the user is in now, or `None` once they left.
    pub fn on_selected(&mut self, channel: Option<VoiceChannel>, now: Instant) {
        match channel {
            Some(channel) => self.current = Some(channel),
            None => {
                if let Some(left) = self.current.take().filter(VoiceChannel::in_guild) {
                    self.left = Some((left, now));
                }
            }
        }
    }

    pub fn rejoin(&self, now: Instant) -> Rejoin {
        if self.current.is_some() {
            return Rejoin::AlreadyInChannel;
        }
        match &self.left {
            Some((channel, at)) if now.duration_since(*at) <= REJOIN_WINDOW => {
                Rejoin::Channel(channel.clone())
            }
            _ => Rejoin::NothingRecent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guild_channel(id: &str) -> VoiceChannel {
        VoiceChannel {
            id: id.to_owned(),
            guild_id: Some("guild".to_owned()),
            name: None,
        }
    }

    fn dm_call(id: &str) -> VoiceChannel {
        VoiceChannel {
            id: id.to_owned(),
            guild_id: None,
            name: None,
        }
    }

    #[test]
    fn the_event_and_the_channel_object_are_read() {
        let event = json!({ "channel_id": "123", "guild_id": "456" });
        assert_eq!(
            parse_channel_select(&event),
            Some(VoiceChannel {
                id: "123".to_owned(),
                guild_id: Some("456".to_owned()),
                name: None,
            })
        );
        assert_eq!(
            parse_channel_select(&json!({ "channel_id": null, "guild_id": null })),
            None
        );

        let channel = json!({ "id": "123", "guild_id": "456", "name": "general", "type": 2 });
        assert_eq!(
            parse_channel(&channel).and_then(|channel| channel.name),
            Some("general".to_owned())
        );
        assert_eq!(parse_channel(&Value::Null), None);
    }

    #[test]
    fn a_channel_left_within_the_window_is_rejoined() {
        let start = Instant::now();
        let mut memory = ChannelMemory::default();
        memory.on_selected(Some(guild_channel("a")), start);
        memory.on_selected(None, start);

        assert_eq!(
            memory.rejoin(start + REJOIN_WINDOW),
            Rejoin::Channel(guild_channel("a"))
        );
    }

    #[test]
    fn a_channel_left_too_long_ago_is_not() {
        let start = Instant::now();
        let mut memory = ChannelMemory::default();
        memory.on_selected(Some(guild_channel("a")), start);
        memory.on_selected(None, start);

        assert_eq!(
            memory.rejoin(start + REJOIN_WINDOW + Duration::from_secs(1)),
            Rejoin::NothingRecent
        );
    }

    #[test]
    fn nothing_happens_while_in_a_channel() {
        let start = Instant::now();
        let mut memory = ChannelMemory::default();
        memory.on_selected(Some(guild_channel("a")), start);
        memory.on_selected(None, start);
        memory.on_selected(Some(guild_channel("b")), start);

        assert_eq!(memory.rejoin(start), Rejoin::AlreadyInChannel);
    }

    #[test]
    fn the_last_channel_left_is_the_one_rejoined() {
        // Moving from one channel to another is not leaving; leaving the second one is.
        let start = Instant::now();
        let mut memory = ChannelMemory::default();
        memory.on_selected(Some(guild_channel("a")), start);
        memory.on_selected(Some(guild_channel("b")), start);
        memory.on_selected(None, start);

        assert_eq!(memory.rejoin(start), Rejoin::Channel(guild_channel("b")));
    }

    #[test]
    fn a_dm_call_is_never_rejoined() {
        let start = Instant::now();
        let mut memory = ChannelMemory::default();
        memory.on_selected(Some(dm_call("dm")), start);
        memory.on_selected(None, start);
        assert_eq!(memory.rejoin(start), Rejoin::NothingRecent);

        // Nor does a call made after leaving a server channel replace that channel.
        memory.on_selected(Some(guild_channel("a")), start);
        memory.on_selected(None, start);
        memory.on_selected(Some(dm_call("dm")), start);
        memory.on_selected(None, start);
        assert_eq!(memory.rejoin(start), Rejoin::Channel(guild_channel("a")));
    }

    #[test]
    fn nothing_is_rejoined_before_any_channel_was_left() {
        assert_eq!(
            ChannelMemory::default().rejoin(Instant::now()),
            Rejoin::NothingRecent
        );
    }
}
