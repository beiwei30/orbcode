use orbcode_app_server_protocol::ProtocolError;
use orbcode_core::CoreError;

/// Errors originating from the client layer.
///
/// `ClientError` wraps the underlying `CoreError` from the app-server and adds
/// transport-specific failure modes that will matter once the client talks over
/// a real transport (IPC, WebSocket, HTTP) rather than an in-process shortcut.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// An error forwarded from the app-server / core layer.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// A protocol-level error returned by the server inside a response
    /// envelope.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),

    /// A transport-level failure (connection lost, timeout, etc.).
    #[error("transport error: {0}")]
    Transport(String),

    /// Serialisation round-trip failure (relevant for out-of-process transports).
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The request was cancelled before a response arrived.
    #[error("request cancelled")]
    Cancelled,
}
