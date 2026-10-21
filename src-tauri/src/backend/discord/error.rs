use spawned_concurrency::ActorError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DiscordError {
    #[error("Failed to connect to the Discord pipe")]
    PipeConnectionFailed,

    #[error("Discord pipe is not connected")]
    PipeNotConnected,

    #[error("Error reading from the Discord pipe")]
    PipeErrorReading,

    #[error("Error writing to the Discord pipe")]
    PipeWriteError,

    #[error("Discord handshake failed")]
    HandshakeFailed,

    #[error("Client ID not found")]
    ClientIdNotFound,

    #[error("Failed to convert data with Serde: {0}")]
    SerdeConvertionError(#[from] serde_json::Error),

    #[error("Authorization with Discord failed")]
    AuthorizationFailed,

    #[error("Authentication with Discord failed")]
    AuthenticationFailed,

    #[error("No data found in the response")]
    NoDataFound,

    /// An error Discord itself reported, with its own wording preserved.
    #[error("Discord rejected the command: {0}")]
    Rpc(String),

    #[error("Discord did not answer the command in time")]
    CommandTimedOut,

    /// An OAuth2 failure, carrying Discord's own wording plus the setup step to check.
    #[error("Discord rejected the authorisation ({code}): {description}{}", .hint.map(|h| format!(". {h}")).unwrap_or_default())]
    OAuth {
        code: String,
        description: String,
        hint: Option<&'static str>,
    },

    #[error("Discord announced a {0} byte frame, which is beyond any sane payload")]
    FrameTooLarge(u32),

    #[error("Internal communication channel is closed")]
    InternalChannelClosed,

    #[error("Error closing the thread")]
    ErrorClosingThread,

    #[error("Pipe IO error: {0}")]
    PipeIOError(#[from] std::io::Error),

    #[error("Handshake not performed")]
    HandshakeNotDone,

    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("Discord actor error: {0}")]
    ActorError(ActorError),

    #[error("Error Client alredy connected")]
    ClientAlreadyConnected,

    #[error("Error Client not connected")]
    ClientNotConnected,
}
