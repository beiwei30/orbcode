/// Errors that can occur during transport operation.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// An I/O error occurred while reading from or writing to the transport.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON serialization or deserialization error occurred.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A received payload exceeded the configured maximum size.
    #[error("payload too large: {size} bytes (max {max})")]
    PayloadTooLarge { size: usize, max: usize },

    /// The connection was closed by the remote end.
    #[error("connection closed")]
    ConnectionClosed,

    /// The client failed to provide a valid authentication token.
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),

    /// A WebSocket protocol error occurred.
    #[error("WebSocket error: {0}")]
    WebSocket(String),
}
