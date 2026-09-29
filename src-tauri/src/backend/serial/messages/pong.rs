use std::fmt;

use bytes::BufMut;

use crate::error::SerialMessageError;
use crate::serial_message::SerialFrame;

/// Protocol revision reported by firmware that predates versioning.
///
/// Such a device answers the handshake with an empty pong. It is still recognisably a DS2000, so
/// it is reported as out of date rather than ignored like any other device on the machine.
pub const UNVERSIONED_PROTOCOL: u8 = 0;

/// Firmware release, as reported by the device.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
}

impl fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What the device on the other end says it is.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct DeviceVersion {
    /// Wire format revision. The application only talks to a device whose revision matches its
    /// own [`crate::serial_message::PROTOCOL_VERSION`].
    pub protocol: u8,
    /// `None` for firmware that predates versioning.
    pub firmware: Option<FirmwareVersion>,
}

/// The device's answer to a ping: `[protocol][major][minor][patch]`.
///
/// The payload is what lets the application refuse a device whose wire format it does not speak,
/// instead of accepting it and then silently misreading its frames.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct PongMessage {
    pub device: DeviceVersion,
}

impl SerialFrame for PongMessage {
    const CODE: u8 = 0x01;

    /// Only the device sends pongs; this exists for the tests and for symmetry with `decode`.
    fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        let Some(firmware) = self.device.firmware else {
            return Ok(());
        };
        buf.put_u8(self.device.protocol);
        buf.put_u8(firmware.major);
        buf.put_u8(firmware.minor);
        buf.put_u8(firmware.patch);
        Ok(())
    }

    /// An empty payload is firmware from before versioning. Anything else has to be complete: a
    /// pong cut short would otherwise be read as a different, perfectly plausible version.
    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError> {
        match msg_data {
            [] => Ok(Self {
                device: DeviceVersion {
                    protocol: UNVERSIONED_PROTOCOL,
                    firmware: None,
                },
            }),
            [protocol, major, minor, patch] => Ok(Self {
                device: DeviceVersion {
                    protocol: *protocol,
                    firmware: Some(FirmwareVersion {
                        major: *major,
                        minor: *minor,
                        patch: *patch,
                    }),
                },
            }),
            _ => Err(SerialMessageError::InvalidMessageLength),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_versioned_pong_carries_both_versions() {
        let pong = PongMessage::decode(&[1, 0, 2, 3]).expect("decodes");

        assert_eq!(pong.device.protocol, 1);
        assert_eq!(
            pong.device.firmware.map(|version| version.to_string()),
            Some("0.2.3".to_owned())
        );
    }

    #[test]
    fn an_empty_pong_is_an_unversioned_device() {
        let pong = PongMessage::decode(&[]).expect("decodes");

        assert_eq!(pong.device.protocol, UNVERSIONED_PROTOCOL);
        assert_eq!(pong.device.firmware, None);
    }

    #[test]
    fn a_truncated_pong_is_rejected() {
        for length in 1..4 {
            assert_eq!(
                PongMessage::decode(&[1, 0, 2, 3][..length]),
                Err(SerialMessageError::InvalidMessageLength),
                "{length} bytes"
            );
        }
    }

    #[test]
    fn an_overlong_pong_is_rejected() {
        assert_eq!(
            PongMessage::decode(&[1, 0, 2, 3, 4]),
            Err(SerialMessageError::InvalidMessageLength)
        );
    }
}
