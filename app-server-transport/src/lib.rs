mod error;
#[cfg(unix)]
pub mod socket;
pub mod stdio;
pub mod websocket;

pub use error::TransportError;
#[cfg(unix)]
pub use socket::{run_unix_socket_transport, run_unix_socket_transport_with_ready};
pub use stdio::{StdioTransportConfig, run_stdio_transport, run_transport};
pub use websocket::{
    WebSocketTransportConfig, run_websocket_transport, run_websocket_transport_with_bound_addr,
};

#[cfg(not(unix))]
pub async fn run_unix_socket_transport(
    socket_path: &std::path::Path,
    app_server: orbcode_app_server::AppServer,
    config: StdioTransportConfig,
) -> Result<(), TransportError> {
    run_unix_socket_transport_with_ready(socket_path, app_server, config, None).await
}

#[cfg(not(unix))]
pub async fn run_unix_socket_transport_with_ready(
    _socket_path: &std::path::Path,
    _app_server: orbcode_app_server::AppServer,
    _config: StdioTransportConfig,
    _ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<(), TransportError> {
    Err(TransportError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix socket transport is not supported on this platform",
    )))
}
