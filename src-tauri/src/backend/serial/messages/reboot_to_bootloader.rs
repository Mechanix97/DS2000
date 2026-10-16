use bytes::BufMut;

use crate::error::SerialMessageError;
use crate::serial_message::SerialFrame;

/// Asks the device to reboot into USB boot, where it mounts as a drive and takes a UF2 image.
///
/// This is how the application updates firmware without anyone touching the board: the rev A
/// board has no BOOTSEL button. The device does not answer; its serial port simply goes away.
///
/// The payload is fixed and checked by the firmware. The CRC already rejects corrupted frames, but
/// a message that takes the device offline should not hang on a single code byte either.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RebootToBootloaderMessage {}

impl RebootToBootloaderMessage {
    pub const MAGIC: [u8; 4] = *b"BOOT";
}

impl SerialFrame for RebootToBootloaderMessage {
    const CODE: u8 = 0x05;

    fn encode(&self, buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        buf.put_slice(&Self::MAGIC);
        Ok(())
    }

    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError> {
        if msg_data != Self::MAGIC {
            return Err(SerialMessageError::MalformedData);
        }
        Ok(Self {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_magic_is_accepted() {
        assert!(RebootToBootloaderMessage::decode(b"BOOT").is_ok());
        for payload in [&b""[..], b"BOO", b"BOOTS", b"boot"] {
            assert!(RebootToBootloaderMessage::decode(payload).is_err());
        }
    }
}
