use bytes::BufMut;

use crate::error::SerialMessageError;
use crate::serial_message::SerialFrame;

/// Opens a session: the application introduces itself and the device answers with
/// [`crate::messages::device_info::DeviceInfoMessage`].
///
/// It is not a liveness check. The answer is what the application decides compatibility on, so
/// the exchange is named for what it does rather than after ping/pong.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct HelloMessage {}

impl SerialFrame for HelloMessage {
    const CODE: u8 = 0x00;

    /// A hello has no payload.
    fn encode(&self, _buf: &mut dyn BufMut) -> Result<(), SerialMessageError> {
        Ok(())
    }

    /// Nothing may follow the code: a payload here means the frame is not what it claims to be.
    fn decode(msg_data: &[u8]) -> Result<Self, SerialMessageError> {
        if !msg_data.is_empty() {
            return Err(SerialMessageError::InvalidMessageLength);
        }
        Ok(Self {})
    }
}
