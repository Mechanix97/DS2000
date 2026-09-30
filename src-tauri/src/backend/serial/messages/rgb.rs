use bytes::BufMut;
use common::rgb_update::LedRgb;
use common::rgb_update::RGBConfig;
use common::rgb_update::RGBMode;

use crate::error::SerialMessageError;
use crate::serial_message::SerialFrame;

#[derive(Debug, PartialEq, Eq, Clone)]

pub struct RGBConfigMessage {
    pub update: RGBConfig,
}

impl SerialFrame for RGBConfigMessage {
    const CODE: u8 = 0x04;
    /// Frame layout is `[brightness][mode][speed]` followed by six colour bytes for the modes that
    /// use them.
    ///
    /// Speed sits before the colours rather than after so its offset does not depend on the mode.
    /// Appending it would have put it at index 2 for the rainbow and index 8 otherwise, and the
    /// firmware would have to know the mode before it could find it.
    fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        buf.put_u8(self.update.brightness);
        match &self.update.rgb_mode {
            RGBMode::Rainbow => {
                buf.put_u8(0x00);
                buf.put_u8(self.update.speed);
            }
            RGBMode::Fixed { led1, led2 } => {
                buf.put_u8(0x01);
                buf.put_u8(self.update.speed);
                buf.put_u8(led1.red);
                buf.put_u8(led1.green);
                buf.put_u8(led1.blue);
                buf.put_u8(led2.red);
                buf.put_u8(led2.green);
                buf.put_u8(led2.blue);
            }
            RGBMode::Breathing { led1, led2 } => {
                buf.put_u8(0x02);
                buf.put_u8(self.update.speed);
                buf.put_u8(led1.red);
                buf.put_u8(led1.green);
                buf.put_u8(led1.blue);
                buf.put_u8(led2.red);
                buf.put_u8(led2.green);
                buf.put_u8(led2.blue);
            }
        }
        Ok(())
    }

    /// Decodes symmetrically to [`Self::encode`].
    ///
    /// This used to ignore its input and return `RGBConfig::default()`, so any payload — including
    /// a truncated or corrupt one — silently produced a valid-looking default configuration. The
    /// device never sends this message today, but a decoder that cannot fail hides real framing
    /// bugs, which is exactly how the ping delimiter problem stayed invisible.
    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError> {
        let (&brightness, rest) = msg_data
            .split_first()
            .ok_or(SerialMessageError::InvalidMessageLength)?;
        let (&mode, rest) = rest
            .split_first()
            .ok_or(SerialMessageError::InvalidMessageLength)?;
        let (&speed, colors) = rest
            .split_first()
            .ok_or(SerialMessageError::InvalidMessageLength)?;

        let rgb_mode = match mode {
            0x00 => RGBMode::Rainbow,
            0x01 | 0x02 => {
                if colors.len() < 6 {
                    return Err(SerialMessageError::InvalidMessageLength);
                }
                let led1 = LedRgb {
                    red: colors[0],
                    green: colors[1],
                    blue: colors[2],
                };
                let led2 = LedRgb {
                    red: colors[3],
                    green: colors[4],
                    blue: colors[5],
                };
                if mode == 0x01 {
                    RGBMode::Fixed { led1, led2 }
                } else {
                    RGBMode::Breathing { led1, led2 }
                }
            }
            _ => return Err(SerialMessageError::MalformedData),
        };

        Ok(Self {
            update: RGBConfig {
                brightness,
                speed,
                rgb_mode,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(update: RGBConfig) {
        let mut buffer = Vec::new();
        RGBConfigMessage {
            update: update.clone(),
        }
        .encode(&mut buffer)
        .expect("encodes");

        assert_eq!(
            RGBConfigMessage::decode(&buffer).expect("decodes").update,
            update
        );
    }

    #[test]
    fn every_mode_survives_a_round_trip() {
        let led1 = LedRgb {
            red: 10,
            green: 20,
            blue: 30,
        };
        let led2 = LedRgb {
            red: 40,
            green: 50,
            blue: 60,
        };

        round_trip(RGBConfig {
            brightness: 128,
            speed: 200,
            rgb_mode: RGBMode::Rainbow,
        });
        round_trip(RGBConfig {
            brightness: 200,
            speed: 0,
            rgb_mode: RGBMode::Fixed { led1, led2 },
        });
        round_trip(RGBConfig {
            brightness: 1,
            speed: 254,
            rgb_mode: RGBMode::Breathing { led1, led2 },
        });
    }

    /// Pins docs/PROTOCOL.md, "RGB (0x04)".
    ///
    /// Pins the byte order, which is a contract with the firmware rather than an implementation
    /// detail. Speed has to sit at a fixed offset: appending it instead would move it depending on
    /// the mode, and the device would have to decide what the frame is before it could find it.
    #[test]
    fn speed_travels_between_the_mode_and_the_colours() {
        let mut buffer = Vec::new();
        RGBConfigMessage {
            update: RGBConfig {
                brightness: 10,
                speed: 20,
                rgb_mode: RGBMode::Fixed {
                    led1: LedRgb {
                        red: 1,
                        green: 2,
                        blue: 3,
                    },
                    led2: LedRgb {
                        red: 4,
                        green: 5,
                        blue: 6,
                    },
                },
            },
        }
        .encode(&mut buffer)
        .expect("encodes");

        assert_eq!(buffer, vec![10, 0x01, 20, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn a_truncated_colour_payload_is_rejected() {
        // Brightness, mode and speed arrived; the six colour bytes did not.
        // Previously this returned a default configuration and the corruption went unnoticed.
        assert_eq!(
            RGBConfigMessage::decode(&[128, 0x01, 10, 20]),
            Err(SerialMessageError::InvalidMessageLength)
        );
    }

    #[test]
    fn a_payload_without_a_speed_byte_is_rejected() {
        assert_eq!(
            RGBConfigMessage::decode(&[128, 0x00]),
            Err(SerialMessageError::InvalidMessageLength)
        );
    }

    #[test]
    fn an_unknown_mode_is_rejected() {
        // Long enough to be well formed, so it is the mode that is rejected and not the length.
        assert_eq!(
            RGBConfigMessage::decode(&[128, 0x09, 50]),
            Err(SerialMessageError::MalformedData)
        );
    }

    #[test]
    fn an_empty_payload_is_rejected() {
        assert_eq!(
            RGBConfigMessage::decode(&[]),
            Err(SerialMessageError::InvalidMessageLength)
        );
    }
}
