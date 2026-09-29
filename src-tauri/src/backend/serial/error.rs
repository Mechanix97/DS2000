use spawned_concurrency::error::GenServerError;
use thiserror::Error;

use crate::messages::device_info::DeviceVersion;

#[derive(Error, Debug)]
pub enum SerialPortError {
    #[error("Port is not available")]
    PortNotAvailable,
    #[error("Port is already connected")]
    PortAlreadyConnected,
    #[error("Port is not connected")]
    PortNotConnected,
    #[error("Internal channel closed unexpectedly")]
    InternalChannelClosed,
    #[error("Error closing thread")]
    ErrorClosingThread,
    #[error("Operation timed out")]
    TimedOut,
    #[error("Authentication failed")]
    AuthenticationFailed,
    /// A DS2000 answered, but speaks a protocol revision this application does not.
    #[error("Device speaks protocol {}, this application speaks {}", .0.protocol, crate::serial_message::PROTOCOL_VERSION)]
    IncompatibleDevice(DeviceVersion),
    #[error("Error reading from port")]
    ErrorReadingPort,
    #[error("Message encoding error: {0}")]
    ErrorEncodingMsg(SerialMessageError),
    #[error("Message decoding error: {0}")]
    ErrorDecodingMsg(SerialMessageError),
    #[error("Internal error occurred")]
    InternalError,
    #[error("I/O error: {0}")]
    IoError(std::io::Error),

    #[error("Spawned GenServer Error")]
    GenServerError(GenServerError),
}

impl PartialEq for SerialPortError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (SerialPortError::PortNotAvailable, SerialPortError::PortNotAvailable) => true,
            (SerialPortError::PortAlreadyConnected, SerialPortError::PortAlreadyConnected) => true,
            (SerialPortError::PortNotConnected, SerialPortError::PortNotConnected) => true,
            (SerialPortError::InternalChannelClosed, SerialPortError::InternalChannelClosed) => {
                true
            }
            (SerialPortError::ErrorClosingThread, SerialPortError::ErrorClosingThread) => true,
            (SerialPortError::TimedOut, SerialPortError::TimedOut) => true,
            (SerialPortError::AuthenticationFailed, SerialPortError::AuthenticationFailed) => true,
            (SerialPortError::IncompatibleDevice(d1), SerialPortError::IncompatibleDevice(d2)) => {
                d1 == d2
            }
            (SerialPortError::ErrorReadingPort, SerialPortError::ErrorReadingPort) => true,
            (SerialPortError::ErrorEncodingMsg(e1), SerialPortError::ErrorEncodingMsg(e2)) => {
                e1 == e2
            }
            (SerialPortError::ErrorDecodingMsg(e1), SerialPortError::ErrorDecodingMsg(e2)) => {
                e1 == e2
            }
            (SerialPortError::InternalError, SerialPortError::InternalError) => true,
            // Compare IoError based on ErrorKind
            (SerialPortError::IoError(e1), SerialPortError::IoError(e2)) => e1.kind() == e2.kind(),
            (SerialPortError::GenServerError(_e1), SerialPortError::GenServerError(_e2)) => true,
            _ => false,
        }
    }
}

impl Eq for SerialPortError {}

impl From<std::io::Error> for SerialPortError {
    fn from(err: std::io::Error) -> Self {
        SerialPortError::IoError(err)
    }
}

impl From<SerialMessageError> for SerialPortError {
    fn from(err: SerialMessageError) -> Self {
        SerialPortError::ErrorDecodingMsg(err)
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]

pub enum SerialMessageError {
    #[error("Error malformed data")]
    MalformedData,

    #[error("Error invalid message length")]
    InvalidMessageLength,

    /// The COBS encoding does not hold together: bytes were lost or added on the wire.
    #[error("Error invalid framing")]
    InvalidFraming,

    /// The frame decoded but its CRC does not match its contents.
    #[error("Error checksum mismatch")]
    ChecksumMismatch,
}
