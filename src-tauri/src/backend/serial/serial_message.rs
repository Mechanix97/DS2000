use bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::messages::voice_settings::VoiceSettingsMessage;

use super::error::{SerialMessageError, SerialPortError};
use super::messages::button::ButtonMessage;
use super::messages::ping::PingMessage;
use super::messages::pong::PongMessage;
use super::messages::rgb::RGBConfigMessage;

/// Wire format revision this application speaks.
///
/// The device reports its own in the pong, and a mismatch refuses the connection instead of
/// exchanging frames the other side would misread. Bump it with every change to the framing, a
/// message code or a payload layout, and bump the firmware's `PROTOCOL_VERSION` with it.
///
/// Revision 1 is the first to report versions at all; firmware before it is
/// [`crate::messages::pong::UNVERSIONED_PROTOCOL`].
pub const PROTOCOL_VERSION: u8 = 1;

pub struct SerialMessageCodec;

impl Decoder for SerialMessageCodec {
    type Item = SerialMessage;
    type Error = SerialPortError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, SerialPortError> {
        if let Some(pos) = src.iter().position(|&b| b == 0xFF) {
            let frame = src.split_to(pos + 1);

            let data = &frame[..frame.len() - 1];
            Ok(Some(SerialMessage::decode(data)?))
        } else {
            Ok(None)
        }
    }
}

impl Encoder<SerialMessage> for SerialMessageCodec {
    type Error = SerialPortError;

    fn encode(&mut self, item: SerialMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst)?;
        dst.put_u8(0xFF);

        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum SerialMessage {
    Ping(PingMessage),
    Pong(PongMessage),
    Button(ButtonMessage),
    VoiceSettings(VoiceSettingsMessage),
    RGBUpdate(RGBConfigMessage),
}

impl SerialMessage {
    fn code(&self) -> u8 {
        match self {
            SerialMessage::Ping(_) => PingMessage::CODE,
            SerialMessage::Pong(_) => PongMessage::CODE,
            SerialMessage::Button(_) => ButtonMessage::CODE,
            SerialMessage::VoiceSettings(_) => VoiceSettingsMessage::CODE,
            SerialMessage::RGBUpdate(_) => RGBConfigMessage::CODE,
        }
    }

    pub fn decode(data: &[u8]) -> Result<SerialMessage, SerialMessageError> {
        if data.is_empty() {
            return Err(SerialMessageError::MalformedData);
        }

        let msg_id = data[0];
        match msg_id {
            PingMessage::CODE => Ok(SerialMessage::Ping(PingMessage::decode(&data[1..])?)),
            PongMessage::CODE => Ok(SerialMessage::Pong(PongMessage::decode(&data[1..])?)),
            ButtonMessage::CODE => Ok(SerialMessage::Button(ButtonMessage::decode(&data[1..])?)),
            VoiceSettingsMessage::CODE => Ok(SerialMessage::VoiceSettings(
                VoiceSettingsMessage::decode(&data[1..])?,
            )),
            RGBConfigMessage::CODE => Ok(SerialMessage::RGBUpdate(RGBConfigMessage::decode(
                &data[1..],
            )?)),
            _ => Err(SerialMessageError::MalformedData),
        }
    }

    pub fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        buf.put_u8(self.code());
        match self {
            SerialMessage::Ping(msg) => msg.encode(buf),
            SerialMessage::Pong(msg) => msg.encode(buf),
            SerialMessage::Button(msg) => msg.encode(buf),
            SerialMessage::VoiceSettings(msg) => msg.encode(buf),
            SerialMessage::RGBUpdate(msg) => msg.encode(buf),
        }
    }
}

/// A message body that can travel over the serial link.
///
/// `CODE` is the first byte of the frame and identifies the variant. Codes are part of the
/// firmware contract: changing one breaks every device already flashed.
pub trait SerialFrame: Sized {
    const CODE: u8;

    fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError>;

    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::button::{Button, ButtonMessage};
    use crate::messages::ping::PingMessage;
    use crate::messages::pong::{DeviceVersion, FirmwareVersion, PongMessage};
    use tokio_util::codec::{Decoder, Encoder};

    fn pong() -> PongMessage {
        PongMessage {
            device: DeviceVersion {
                protocol: PROTOCOL_VERSION,
                firmware: Some(FirmwareVersion {
                    major: 0,
                    minor: 2,
                    patch: 0,
                }),
            },
        }
    }

    fn round_trip(message: SerialMessage) {
        let mut buffer = BytesMut::new();
        SerialMessageCodec
            .encode(message.clone(), &mut buffer)
            .expect("encodes");

        let decoded = SerialMessageCodec
            .decode(&mut buffer)
            .expect("decodes")
            .expect("a complete frame");

        assert_eq!(decoded, message);
        assert!(buffer.is_empty(), "the frame should be consumed");
    }

    #[test]
    fn every_message_type_survives_a_round_trip() {
        round_trip(SerialMessage::Ping(PingMessage {}));
        round_trip(SerialMessage::Pong(pong()));
        round_trip(SerialMessage::Pong(PongMessage {
            device: DeviceVersion {
                protocol: crate::messages::pong::UNVERSIONED_PROTOCOL,
                firmware: None,
            },
        }));
        round_trip(SerialMessage::Button(ButtonMessage {
            button: Button::DeafenButton,
        }));
    }

    #[test]
    fn a_partial_frame_yields_nothing_until_its_delimiter_arrives() {
        let mut buffer = BytesMut::new();
        buffer.extend_from_slice(&[PingMessage::CODE]);

        // No 0xFF yet, so the frame is incomplete rather than malformed.
        assert!(
            SerialMessageCodec
                .decode(&mut buffer)
                .expect("no error")
                .is_none()
        );

        buffer.extend_from_slice(&[0xFF]);
        assert!(
            SerialMessageCodec
                .decode(&mut buffer)
                .expect("no error")
                .is_some()
        );
    }

    #[test]
    fn two_frames_in_one_buffer_are_decoded_separately() {
        let mut buffer = BytesMut::new();
        SerialMessageCodec
            .encode(SerialMessage::Ping(PingMessage {}), &mut buffer)
            .expect("encodes");
        SerialMessageCodec
            .encode(SerialMessage::Pong(pong()), &mut buffer)
            .expect("encodes");

        assert_eq!(
            SerialMessageCodec.decode(&mut buffer).unwrap().unwrap(),
            SerialMessage::Ping(PingMessage {})
        );
        assert_eq!(
            SerialMessageCodec.decode(&mut buffer).unwrap().unwrap(),
            SerialMessage::Pong(pong())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn an_unknown_message_code_is_rejected() {
        let mut buffer = BytesMut::new();
        buffer.extend_from_slice(&[0x7E, 0xFF]);

        assert!(SerialMessageCodec.decode(&mut buffer).is_err());
    }

    /// The case the versioned pong exists for: a frame cut short must not pass for a different
    /// version, and the stream has to carry on with the next frame afterwards.
    #[test]
    fn a_truncated_pong_is_rejected_and_the_next_frame_still_decodes() {
        let mut buffer = BytesMut::new();
        buffer.extend_from_slice(&[PongMessage::CODE, PROTOCOL_VERSION, 0, 0xFF]);
        SerialMessageCodec
            .encode(SerialMessage::Ping(PingMessage {}), &mut buffer)
            .expect("encodes");

        assert!(SerialMessageCodec.decode(&mut buffer).is_err());
        assert_eq!(
            SerialMessageCodec.decode(&mut buffer).unwrap().unwrap(),
            SerialMessage::Ping(PingMessage {})
        );
    }

    #[test]
    fn an_empty_frame_is_rejected() {
        let mut buffer = BytesMut::new();
        buffer.extend_from_slice(&[0xFF]);

        assert!(SerialMessageCodec.decode(&mut buffer).is_err());
    }

    /// Documents a known protocol limitation rather than asserting desired behaviour.
    ///
    /// `0xFF` is the frame delimiter and is not escaped, so no payload byte may be 255. That is
    /// why `RGBConfig::check_255` exists, and why full brightness and pure white are unreachable.
    /// Fixing it properly needs byte stuffing on both sides, and the firmware lives in another
    /// repository.
    #[test]
    fn a_payload_byte_of_255_would_split_the_frame() {
        let mut buffer = BytesMut::new();
        buffer.extend_from_slice(&[crate::messages::rgb::RGBConfigMessage::CODE, 0xFF, 0x01]);

        // The delimiter is found inside what should have been the payload, so the frame is cut
        // short and decodes as malformed instead of carrying the value 255.
        assert!(SerialMessageCodec.decode(&mut buffer).is_err());
    }
}
