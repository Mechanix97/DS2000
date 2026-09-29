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

/// Hardware the firmware was built for.
///
/// A firmware update has to pick the image built for the same board, and the version alone cannot
/// say which that is. The ids are set per build environment in the firmware's `platformio.ini`
/// (`custom_board_id`); `docs/PROTOCOL.md` lists them.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Board {
    /// Waveshare RP2350-Zero, the module the prototype is built on.
    Rp2350Zero,
    /// Raspberry Pi Pico 2.
    Pico2,
    /// Raspberry Pi Pico (RP2040).
    Pico,
    /// The DS2000 rev A board.
    Ds2000RevA,
    /// An id this application does not know, from firmware newer than it.
    Unknown(u8),
}

impl Board {
    pub fn id(self) -> u8 {
        match self {
            Board::Rp2350Zero => 0x01,
            Board::Pico2 => 0x02,
            Board::Pico => 0x03,
            Board::Ds2000RevA => 0x10,
            Board::Unknown(id) => id,
        }
    }

    pub fn from_id(id: u8) -> Self {
        match id {
            0x01 => Board::Rp2350Zero,
            0x02 => Board::Pico2,
            0x03 => Board::Pico,
            0x10 => Board::Ds2000RevA,
            other => Board::Unknown(other),
        }
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Board::Rp2350Zero => write!(f, "RP2350-Zero"),
            Board::Pico2 => write!(f, "Pico 2"),
            Board::Pico => write!(f, "Pico"),
            Board::Ds2000RevA => write!(f, "DS2000 rev A"),
            Board::Unknown(id) => write!(f, "board 0x{id:02X}"),
        }
    }
}

/// What the device on the other end says it is.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct DeviceIdentity {
    /// Wire format revision. The application only talks to a device whose revision matches its
    /// own [`crate::serial_message::PROTOCOL_VERSION`].
    pub protocol: u8,
    /// `None` for firmware that predates versioning.
    pub board: Option<Board>,
    /// `None` for firmware that predates versioning.
    pub firmware: Option<FirmwareVersion>,
}

impl DeviceIdentity {
    /// Firmware from before protocol 1, which can report nothing about itself.
    pub fn unversioned() -> Self {
        Self {
            protocol: UNVERSIONED_PROTOCOL,
            board: None,
            firmware: None,
        }
    }
}

/// The device's answer to a hello: `[protocol][board][major][minor][patch]`.
///
/// The payload is what lets the application refuse a device whose wire format it does not speak,
/// instead of accepting it and then silently misreading its frames.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct DeviceInfoMessage {
    pub protocol: u8,
    pub board: Board,
    pub firmware: FirmwareVersion,
}

impl DeviceInfoMessage {
    pub fn identity(&self) -> DeviceIdentity {
        DeviceIdentity {
            protocol: self.protocol,
            board: Some(self.board),
            firmware: Some(self.firmware),
        }
    }
}

impl SerialFrame for DeviceInfoMessage {
    const CODE: u8 = 0x01;

    /// Only the device sends this; encoding exists for the tests and for symmetry with `decode`.
    fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        buf.put_u8(self.protocol);
        buf.put_u8(self.board.id());
        buf.put_u8(self.firmware.major);
        buf.put_u8(self.firmware.minor);
        buf.put_u8(self.firmware.patch);
        Ok(())
    }

    /// The protocol byte comes first and is read before anything else is trusted: a device on
    /// another revision may lay out the rest differently, and still has to be told apart by it.
    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError> {
        let [protocol, board, major, minor, patch] = msg_data else {
            return Err(SerialMessageError::InvalidMessageLength);
        };
        Ok(Self {
            protocol: *protocol,
            board: Board::from_id(*board),
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
    fn device_info_carries_the_protocol_board_and_firmware() {
        let info = DeviceInfoMessage::decode(&[1, 0x01, 0, 2, 3]).expect("decodes");

        assert_eq!(info.protocol, 1);
        assert_eq!(info.board, Board::Rp2350Zero);
        assert_eq!(info.firmware.to_string(), "0.2.3");
    }

    #[test]
    fn every_board_id_round_trips_and_unknown_ones_are_kept() {
        for id in 0..=u8::MAX {
            assert_eq!(Board::from_id(id).id(), id);
        }
        assert_eq!(Board::from_id(0x7F), Board::Unknown(0x7F));
    }

    #[test]
    fn a_payload_of_the_wrong_length_is_rejected() {
        for length in [0, 1, 2, 3, 4, 6] {
            assert_eq!(
                DeviceInfoMessage::decode(&[1, 1, 0, 2, 3, 4][..length]),
                Err(SerialMessageError::InvalidMessageLength),
                "{length} bytes"
            );
        }
    }
}
