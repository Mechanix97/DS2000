use std::fmt;

use bytes::BufMut;

use crate::error::SerialMessageError;
use crate::serial_message::SerialFrame;

/// Protocol revision assigned to firmware that predates versioning.
///
/// That firmware frames with a bare `0xFF` and cannot answer a hello at all. The handshake
/// recognises it by a separate probe and reports it under this revision, so it shows up as out of
/// date rather than as no device.
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

/// The device's answer to a hello: `[protocol][major][minor][patch]`.
///
/// The payload is what lets the application refuse a device whose wire format it does not speak,
/// instead of accepting it and then silently misreading its frames.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct DeviceInfoMessage {
    pub protocol: u8,
    pub firmware: FirmwareVersion,
}

impl DeviceInfoMessage {
    pub fn device(&self) -> DeviceVersion {
        DeviceVersion {
            protocol: self.protocol,
            firmware: Some(self.firmware),
        }
    }
}

impl SerialFrame for DeviceInfoMessage {
    const CODE: u8 = 0x01;

    /// Only the device sends this; encoding exists for the tests and for symmetry with `decode`.
    fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        buf.put_u8(self.protocol);
        buf.put_u8(self.firmware.major);
        buf.put_u8(self.firmware.minor);
        buf.put_u8(self.firmware.patch);
        Ok(())
    }

    /// The protocol byte comes first so that a future revision can grow the payload: whatever
    /// else changes, a device can always be told apart by it.
    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError> {
        let [protocol, major, minor, patch] = msg_data else {
            return Err(SerialMessageError::InvalidMessageLength);
        };
        Ok(Self {
            protocol: *protocol,
            firmware: FirmwareVersion {
                major: *major,
                minor: *minor,
                patch: *patch,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_carries_both_versions() {
        let info = DeviceInfoMessage::decode(&[1, 0, 2, 3]).expect("decodes");

        assert_eq!(info.protocol, 1);
        assert_eq!(info.firmware.to_string(), "0.2.3");
    }

    #[test]
    fn a_payload_of_the_wrong_length_is_rejected() {
        for length in [0, 1, 2, 3, 5] {
            assert_eq!(
                DeviceInfoMessage::decode(&[1, 0, 2, 3, 4][..length]),
                Err(SerialMessageError::InvalidMessageLength),
                "{length} bytes"
            );
        }
    }
}
