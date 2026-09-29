use bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::messages::voice_settings::VoiceSettingsMessage;

use super::error::{SerialMessageError, SerialPortError};
use super::framing::{FRAME_DELIMITER, decode_frame, encode_frame};
use super::messages::button::ButtonMessage;
use super::messages::device_info::DeviceInfoMessage;
use super::messages::hello::HelloMessage;
use super::messages::rgb::RGBConfigMessage;

/// Wire format revision this application speaks.
///
/// The device reports its own in [`DeviceInfoMessage`], and a mismatch refuses the connection
/// instead of exchanging frames the other side would misread. Bump it with every change to the
/// framing, a message code or a payload layout, and bump the firmware's `PROTOCOL_VERSION` with it.
///
/// Revision 1 introduced COBS framing with a CRC and the hello / device info exchange. Firmware
/// before it is [`crate::messages::device_info::UNVERSIONED_PROTOCOL`].
pub const PROTOCOL_VERSION: u8 = 1;

/// Longest run of bytes the decoder buffers while waiting for a delimiter.
///
/// The largest frame the protocol defines is well under this. Anything longer is not a DS2000
/// talking, and holding on to it would let a chatty device on the wrong port grow the buffer
/// without bound.
const MAX_ENCODED_FRAME: usize = 64;

/// One frame read off the wire: a message, or why the frame had to be discarded.
///
/// A bad frame is an item rather than a decoder error on purpose. `Framed` ends the stream after
/// the first decoder error, so reporting a corrupted frame that way would turn line noise into a
/// disconnection and a reconnect cycle. Errors are kept for I/O failures.
pub type Frame = Result<SerialMessage, SerialMessageError>;

pub struct SerialMessageCodec;

impl Decoder for SerialMessageCodec {
    type Item = Frame;
    type Error = SerialPortError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, SerialPortError> {
        loop {
            let Some(pos) = src.iter().position(|&b| b == FRAME_DELIMITER) else {
                if src.len() > MAX_ENCODED_FRAME {
                    src.clear();
                    return Ok(Some(Err(SerialMessageError::InvalidFraming)));
                }
                return Ok(None);
            };

            let frame = src.split_to(pos + 1);
            let encoded = &frame[..pos];
            if encoded.is_empty() {
                // Back-to-back delimiters carry nothing. The firmware may send one to resync the
                // line, so this is not worth reporting.
                continue;
            }

            return Ok(Some(
                decode_frame(encoded).and_then(|body| SerialMessage::decode(&body)),
            ));
        }
    }
}

impl Encoder<SerialMessage> for SerialMessageCodec {
    type Error = SerialPortError;

    fn encode(&mut self, item: SerialMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut body = Vec::new();
        item.encode(&mut body)
            .map_err(SerialPortError::ErrorEncodingMsg)?;

        let mut wire = Vec::new();
        encode_frame(&body, &mut wire);
        dst.put_slice(&wire);

        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum SerialMessage {
    Hello(HelloMessage),
    DeviceInfo(DeviceInfoMessage),
    Button(ButtonMessage),
    VoiceSettings(VoiceSettingsMessage),
    RGBUpdate(RGBConfigMessage),
}

impl SerialMessage {
    fn code(&self) -> u8 {
        match self {
            SerialMessage::Hello(_) => HelloMessage::CODE,
            SerialMessage::DeviceInfo(_) => DeviceInfoMessage::CODE,
            SerialMessage::Button(_) => ButtonMessage::CODE,
            SerialMessage::VoiceSettings(_) => VoiceSettingsMessage::CODE,
            SerialMessage::RGBUpdate(_) => RGBConfigMessage::CODE,
        }
    }

    /// Decodes a message body: the code followed by its payload, framing already removed.
    pub fn decode(data: &[u8]) -> Result<SerialMessage, SerialMessageError> {
        let Some((&msg_id, payload)) = data.split_first() else {
            return Err(SerialMessageError::MalformedData);
        };

        match msg_id {
            HelloMessage::CODE => Ok(SerialMessage::Hello(HelloMessage::decode(payload)?)),
            DeviceInfoMessage::CODE => Ok(SerialMessage::DeviceInfo(DeviceInfoMessage::decode(
                payload,
            )?)),
            ButtonMessage::CODE => Ok(SerialMessage::Button(ButtonMessage::decode(payload)?)),
            VoiceSettingsMessage::CODE => Ok(SerialMessage::VoiceSettings(
                VoiceSettingsMessage::decode(payload)?,
            )),
            RGBConfigMessage::CODE => {
                Ok(SerialMessage::RGBUpdate(RGBConfigMessage::decode(payload)?))
            }
            _ => Err(SerialMessageError::MalformedData),
        }
    }

    pub fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        buf.put_u8(self.code());
        match self {
            SerialMessage::Hello(msg) => msg.encode(buf),
            SerialMessage::DeviceInfo(msg) => msg.encode(buf),
            SerialMessage::Button(msg) => msg.encode(buf),
            SerialMessage::VoiceSettings(msg) => msg.encode(buf),
            SerialMessage::RGBUpdate(msg) => msg.encode(buf),
        }
    }
}

/// A message body that can travel over the serial link.
///
/// `CODE` is the first byte of the body and identifies the variant. Codes are part of the
/// firmware contract: changing one means bumping [`PROTOCOL_VERSION`].
pub trait SerialFrame: Sized {
    const CODE: u8;

    fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError>;

    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::encode_frame;
    use crate::messages::button::{Button, ButtonMessage};
    use crate::messages::device_info::FirmwareVersion;
    use common::rgb_update::{LedRgb, RGBConfig, RGBMode};
    use tokio_util::codec::{Decoder, Encoder};

    fn device_info() -> DeviceInfoMessage {
        DeviceInfoMessage {
            protocol: PROTOCOL_VERSION,
            firmware: FirmwareVersion {
                major: 0,
                minor: 2,
                patch: 0,
            },
        }
    }

    fn wire(message: SerialMessage) -> BytesMut {
        let mut buffer = BytesMut::new();
        SerialMessageCodec
            .encode(message, &mut buffer)
            .expect("encodes");
        buffer
    }

    fn next(buffer: &mut BytesMut) -> Option<Frame> {
        SerialMessageCodec.decode(buffer).expect("no I/O error")
    }

    fn round_trip(message: SerialMessage) {
        let mut buffer = wire(message.clone());

        assert_eq!(next(&mut buffer), Some(Ok(message)));
        assert!(buffer.is_empty(), "the frame should be consumed");
    }

    /// Exact wire bytes, shared with the firmware's own tests so the two implementations are
    /// checked against the same vectors rather than only against themselves.
    ///
    /// The hello also has to start with `0x01` and contain no `0xFF`: that is what makes it
    /// harmless to pre-protocol-1 firmware ahead of the legacy probe in `port.rs`.
    #[test]
    fn frames_match_the_reference_wire_bytes() {
        assert_eq!(
            &wire(SerialMessage::Hello(HelloMessage {}))[..],
            [0x01, 0x03, 0xE1, 0xF0, 0x00]
        );
        assert_eq!(
            &wire(SerialMessage::DeviceInfo(DeviceInfoMessage {
                protocol: 1,
                firmware: FirmwareVersion {
                    major: 0,
                    minor: 2,
                    patch: 0,
                },
            }))[..],
            [0x03, 0x01, 0x01, 0x02, 0x02, 0x03, 0xAB, 0x8B, 0x00]
        );
        assert_eq!(
            &wire(SerialMessage::Button(ButtonMessage {
                button: Button::MuteButton,
            }))[..],
            [0x02, 0x02, 0x03, 0x7B, 0x6D, 0x00]
        );
    }

    #[test]
    fn every_message_type_survives_a_round_trip() {
        round_trip(SerialMessage::Hello(HelloMessage {}));
        round_trip(SerialMessage::DeviceInfo(device_info()));
        round_trip(SerialMessage::Button(ButtonMessage {
            button: Button::DeafenButton,
        }));
    }

    /// The limitation #25 was about: 255 used to be the delimiter, so full brightness and pure
    /// white could not be sent.
    #[test]
    fn full_brightness_and_pure_white_reach_the_device() {
        let white = LedRgb {
            red: 255,
            green: 255,
            blue: 255,
        };
        round_trip(SerialMessage::RGBUpdate(RGBConfigMessage {
            update: RGBConfig {
                brightness: 255,
                speed: 255,
                rgb_mode: RGBMode::Fixed {
                    led1: white,
                    led2: white,
                },
            },
        }));
    }

    #[test]
    fn a_partial_frame_yields_nothing_until_its_delimiter_arrives() {
        let complete = wire(SerialMessage::Hello(HelloMessage {}));
        let (head, tail) = complete.split_at(complete.len() - 1);

        let mut buffer = BytesMut::from(head);
        assert_eq!(next(&mut buffer), None);

        buffer.extend_from_slice(tail);
        assert_eq!(
            next(&mut buffer),
            Some(Ok(SerialMessage::Hello(HelloMessage {})))
        );
    }

    #[test]
    fn two_frames_in_one_buffer_are_decoded_separately() {
        let mut buffer = wire(SerialMessage::Hello(HelloMessage {}));
        buffer.extend_from_slice(&wire(SerialMessage::DeviceInfo(device_info())));

        assert_eq!(
            next(&mut buffer),
            Some(Ok(SerialMessage::Hello(HelloMessage {})))
        );
        assert_eq!(
            next(&mut buffer),
            Some(Ok(SerialMessage::DeviceInfo(device_info())))
        );
        assert!(buffer.is_empty());
    }

    /// The failure from #25: a mute press missing its first byte used to decode as a valid
    /// ping. It now fails its checksum, and the frame after it is unaffected.
    #[test]
    fn a_frame_missing_a_byte_is_rejected_and_the_next_one_still_decodes() {
        let mut buffer = wire(SerialMessage::Button(ButtonMessage {
            button: Button::MuteButton,
        }));
        let _ = buffer.split_to(1);
        buffer.extend_from_slice(&wire(SerialMessage::Hello(HelloMessage {})));

        assert!(matches!(next(&mut buffer), Some(Err(_))));
        assert_eq!(
            next(&mut buffer),
            Some(Ok(SerialMessage::Hello(HelloMessage {})))
        );
    }

    #[test]
    fn a_truncated_device_info_is_rejected() {
        // Correctly framed and checksummed, so it is the payload length that fails.
        let mut raw = Vec::new();
        encode_frame(&[DeviceInfoMessage::CODE, PROTOCOL_VERSION, 0], &mut raw);
        let mut buffer = BytesMut::from(raw.as_slice());

        assert_eq!(
            next(&mut buffer),
            Some(Err(SerialMessageError::InvalidMessageLength))
        );
    }

    #[test]
    fn an_unknown_message_code_is_rejected() {
        let mut raw = Vec::new();
        encode_frame(&[0x7E], &mut raw);
        let mut buffer = BytesMut::from(raw.as_slice());

        assert_eq!(
            next(&mut buffer),
            Some(Err(SerialMessageError::MalformedData))
        );
    }

    #[test]
    fn empty_frames_between_delimiters_are_skipped() {
        let mut buffer = BytesMut::from(&[FRAME_DELIMITER, FRAME_DELIMITER][..]);
        buffer.extend_from_slice(&wire(SerialMessage::Hello(HelloMessage {})));

        assert_eq!(
            next(&mut buffer),
            Some(Ok(SerialMessage::Hello(HelloMessage {})))
        );
    }

    #[test]
    fn a_stream_that_never_delimits_is_discarded_rather_than_buffered_forever() {
        let mut buffer = BytesMut::from(&[0xAA; MAX_ENCODED_FRAME + 1][..]);

        assert_eq!(
            next(&mut buffer),
            Some(Err(SerialMessageError::InvalidFraming))
        );
        assert!(buffer.is_empty());
    }
}
