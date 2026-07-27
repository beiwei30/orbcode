use std::net::SocketAddr;
use std::sync::Arc;

use futures::sink::SinkExt;
use futures::stream::StreamExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message};

use orbcode_app_server::AppServer;
use orbcode_app_server::message_processor::{CHANNEL_SINK_CAPACITY, ChannelSink, MessageProcessor};
use orbcode_app_server_protocol::{ClientMessage, ServerMessage};

use crate::TransportError;

/// Configuration for the WebSocket transport.
#[derive(Clone)]
pub struct WebSocketTransportConfig {
    /// Maximum size in bytes for a single incoming WebSocket text message.
    /// Messages exceeding this limit are logged and skipped.
    pub max_payload_bytes: usize,

    /// Optional list of allowed `Origin` header values. When non-empty, a
    /// connecting client whose `Origin` header does not match any entry is
    /// rejected during the WebSocket handshake with HTTP 403.
    ///
    /// An empty list disables origin checking.
    pub allowed_origins: Vec<String>,

    /// Optional authentication token. When set, the first WebSocket text
    /// message from the client must be exactly this token. Invalid or
    /// missing tokens close the connection immediately.
    pub auth_token: Option<String>,

    /// Timeout for the WebSocket upgrade handshake and auth token.
    /// A TCP client that connects but never sends an upgrade or token
    /// is rejected after this duration so it can't block the accept loop.
    pub handshake_timeout: std::time::Duration,
}

impl Default for WebSocketTransportConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: 10 * 1024 * 1024,
            allowed_origins: Vec::new(),
            auth_token: None,
            handshake_timeout: std::time::Duration::from_secs(30),
        }
    }
}

/// Run the WebSocket transport, accepting client connections sequentially.
///
/// Binds a [`TcpListener`] at `addr` and loops, accepting one WebSocket
/// connection at a time. Auth/origin failures only close that connection;
/// the server continues listening for the next client.
///
/// Returns only on an unrecoverable bind/listen error.
pub async fn run_websocket_transport(
    addr: SocketAddr,
    app_server: AppServer,
    config: WebSocketTransportConfig,
) -> Result<(), TransportError> {
    run_websocket_transport_with_bound_addr(addr, app_server, config, None).await
}

pub async fn run_websocket_transport_with_bound_addr(
    addr: SocketAddr,
    app_server: AppServer,
    config: WebSocketTransportConfig,
    bound_addr_tx: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
) -> Result<(), TransportError> {
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr().map_err(TransportError::Io)?;
    tracing::info!(%bound_addr, "WebSocket transport listening");
    if let Some(tx) = bound_addr_tx {
        let _ = tx.send(bound_addr);
    }

    loop {
        let (tcp_stream, peer_addr) = listener.accept().await?;
        tracing::info!(%peer_addr, "TCP connection accepted, upgrading to WebSocket");

        let mut ws_config = WebSocketConfig::default();
        ws_config.max_message_size = Some(config.max_payload_bytes);
        ws_config.max_frame_size = Some(config.max_payload_bytes);

        let allowed_origins = config.allowed_origins.clone();

        #[allow(clippy::result_large_err)]
        let callback = move |request: &tungstenite::handshake::server::Request,
                             response: tungstenite::handshake::server::Response|
              -> Result<
            tungstenite::handshake::server::Response,
            tungstenite::handshake::server::ErrorResponse,
        > {
            if !allowed_origins.is_empty() {
                let origin = request
                    .headers()
                    .get("origin")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if !allowed_origins.iter().any(|o| o == origin) {
                    tracing::warn!(%origin, "WebSocket origin rejected");
                    let resp = tungstenite::http::Response::builder()
                        .status(403)
                        .body(None)
                        .expect("building 403 response");
                    return Err(resp);
                }
            }
            Ok(response)
        };

        // Timeout the WS upgrade — a TCP client that connects but never
        // sends the HTTP upgrade request would block the accept loop.
        let ws_stream = match tokio::time::timeout(
            config.handshake_timeout,
            tokio_tungstenite::accept_hdr_async_with_config(tcp_stream, callback, Some(ws_config)),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "WebSocket handshake failed, continuing");
                continue;
            }
            Err(_) => {
                tracing::warn!(%peer_addr, timeout_secs = config.handshake_timeout.as_secs(), "WebSocket upgrade timed out, continuing");
                continue;
            }
        };

        tracing::info!(%peer_addr, "WebSocket handshake complete");

        match handle_ws_connection(
            ws_stream,
            app_server.clone(),
            config.max_payload_bytes,
            config.auth_token.clone(),
            config.handshake_timeout,
        )
        .await
        {
            Ok(()) => tracing::info!(%peer_addr, "WebSocket client disconnected"),
            Err(TransportError::AuthenticationFailed(ref reason)) => {
                tracing::warn!(%peer_addr, %reason, "WebSocket auth failed, continuing");
            }
            Err(ref e) => {
                tracing::warn!(%peer_addr, error = %e, "WebSocket error, continuing");
            }
        }
    }
}

/// Internal implementation that handles a single WebSocket connection.
async fn handle_ws_connection<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    app_server: AppServer,
    max_payload_bytes: usize,
    auth_token: Option<String>,
    handshake_timeout: std::time::Duration,
) -> Result<(), TransportError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut ws_writer, mut ws_reader) = ws_stream.split();

    // Token auth: if configured, the first text message must be the token
    // within 30 seconds to prevent idle connections blocking the accept loop.
    if let Some(expected) = &auth_token {
        let first_msg_result = tokio::time::timeout(handshake_timeout, ws_reader.next()).await;
        let first_msg = match first_msg_result {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => {
                return Err(TransportError::WebSocket(e.to_string()));
            }
            Ok(None) => {
                return Err(TransportError::AuthenticationFailed(
                    "connection closed before auth token".into(),
                ));
            }
            Err(_) => {
                return Err(TransportError::AuthenticationFailed(
                    "auth token not received within 30 seconds".into(),
                ));
            }
        };
        let token_text = match &first_msg {
            Message::Text(t) => t.trim().to_string(),
            _ => String::new(),
        };
        if !crate::stdio::constant_time_token_eq(&token_text, expected) {
            let _ = ws_writer
                .send(Message::Close(Some(tungstenite::protocol::CloseFrame {
                    code: tungstenite::protocol::frame::coding::CloseCode::Policy,
                    reason: "invalid auth token".into(),
                })))
                .await;
            return Err(TransportError::AuthenticationFailed(
                "invalid auth token".into(),
            ));
        }
    }

    // Set up the message processor with channel-based sink.
    let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
    let (best_effort_tx, mut best_effort_rx) =
        mpsc::channel::<ServerMessage>(CHANNEL_SINK_CAPACITY);
    let sink = Arc::new(ChannelSink::new(lossless_tx, best_effort_tx));
    let mut processor = MessageProcessor::new(app_server, sink);

    // Writer task: drain server messages and send as WebSocket text frames.
    let mut write_handle: tokio::task::JoinHandle<Result<(), TransportError>> =
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    biased;
                    msg = lossless_rx.recv() => msg,
                    msg = best_effort_rx.recv() => msg,
                };
                let Some(msg) = msg else { break };
                let Ok(line) = serde_json::to_string(&msg) else {
                    continue;
                };
                ws_writer
                    .send(Message::Text(line.into()))
                    .await
                    .map_err(|e| TransportError::WebSocket(e.to_string()))?;
            }
            // Send a clean close frame.
            let _ = ws_writer.send(Message::Close(None)).await;
            Ok(())
        });

    // Reader task: read WebSocket messages and dispatch to the processor.
    let mut reader_handle: tokio::task::JoinHandle<Result<(), TransportError>> =
        tokio::spawn(async move {
            while let Some(msg_result) = ws_reader.next().await {
                let msg = match msg_result {
                    Ok(m) => m,
                    Err(tungstenite::Error::ConnectionClosed) => break,
                    Err(tungstenite::Error::Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    )) => {
                        tracing::debug!("WebSocket reset without close handshake");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "WebSocket read error");
                        return Err(TransportError::WebSocket(e.to_string()));
                    }
                };

                match msg {
                    Message::Text(text) => {
                        if text.len() > max_payload_bytes {
                            tracing::warn!(
                                len = text.len(),
                                max = max_payload_bytes,
                                "WebSocket message exceeds payload limit, skipping"
                            );
                            continue;
                        }
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let message: ClientMessage = match serde_json::from_str(trimmed) {
                            Ok(m) => m,
                            Err(e) => {
                                tracing::warn!(error = %e, "malformed WebSocket message, skipping");
                                continue;
                            }
                        };
                        processor.handle_message(message).await;
                    }
                    Message::Close(_) => {
                        tracing::debug!("WebSocket close frame received");
                        break;
                    }
                    Message::Ping(_) | Message::Pong(_) => {
                        // Handled automatically by tungstenite.
                    }
                    Message::Binary(_) => {
                        tracing::warn!("binary WebSocket frames are not supported, skipping");
                    }
                    Message::Frame(_) => {
                        // Raw frames are not expected in normal operation.
                    }
                }
            }
            // Drop processor to close sink channels, signaling writer to exit.
            drop(processor);
            Ok(())
        });

    // Wait for either side to finish. If the writer fails first (broken
    // pipe / WS error), abort the reader. If the reader finishes first
    // (close frame / EOF), let the writer drain remaining messages.
    tokio::select! {
        reader_res = &mut reader_handle => {
            // Reader done. Wait for writer to drain.
            match write_handle.await {
                Ok(Err(e)) => Err(e),
                _ => match reader_res {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(TransportError::ConnectionClosed),
                },
            }
        }
        writer_res = &mut write_handle => {
            reader_handle.abort();
            match writer_res {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(TransportError::ConnectionClosed),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use serde_json::json;
    use tokio_tungstenite::connect_async;

    use orbcode_app_server_protocol::ServerMessage;

    /// Helper to build an initialize request JSON string.
    fn initialize_json() -> String {
        serde_json::to_string(&json!({
            "type": "request",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "test-ws", "version": "0.1" }
            }
        }))
        .unwrap()
    }

    /// Create a temporary AppServer for testing.
    async fn test_app(label: &str) -> AppServer {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "orbcode-ws-{label}-{}-{unique}",
            std::process::id()
        ));
        let home = base.join("home");
        let cwd = base.join("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        AppServer::new(
            cwd,
            orbcode_config::AppConfigOverrides {
                home_dir: Some(home),
                ..orbcode_config::AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server")
    }

    // -------------------------------------------------------------------
    // 1. Initialize over WebSocket
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn websocket_initialize() {
        let app = test_app("ws-init").await;

        // Bind to an ephemeral port.
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpListener::bind(addr).await.expect("bind");
        let bound_addr = listener.local_addr().expect("local_addr");
        drop(listener); // Free the port for the transport to re-bind.

        let config = WebSocketTransportConfig::default();
        let transport_handle =
            tokio::spawn(async move { run_websocket_transport(bound_addr, app, config).await });

        // Wait briefly for the listener to bind.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect as a WebSocket client.
        let url = format!("ws://{bound_addr}");
        let (mut ws, _resp) = connect_async(&url).await.expect("WS connect");

        // Send initialize request.
        ws.send(Message::Text(initialize_json().into()))
            .await
            .unwrap();

        // Read the response.
        let response_msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout waiting for response")
            .expect("stream ended")
            .expect("WS error");

        let text = match response_msg {
            Message::Text(t) => t,
            other => panic!("expected text frame, got: {other:?}"),
        };

        let server_msg: ServerMessage = serde_json::from_str(&text).expect("parse ServerMessage");

        match &server_msg {
            ServerMessage::Response(resp) => {
                assert_eq!(resp.id, "init-1");
                match &resp.result {
                    orbcode_app_server_protocol::ResponseResult::Success { data: Some(data) } => {
                        assert_eq!(data["server_info"]["name"], "orbcode");
                    }
                    other => panic!("expected Success with data, got: {other:?}"),
                }
            }
            other => panic!("expected Response, got: {other:?}"),
        }

        // Close the WebSocket cleanly. Server loops for next client.
        ws.send(Message::Close(None)).await.unwrap();
        transport_handle.abort();
    }

    // -------------------------------------------------------------------
    // 2. Disconnect without sending data closes transport cleanly
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn websocket_disconnect() {
        let app = test_app("ws-disc").await;

        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpListener::bind(addr).await.expect("bind");
        let bound_addr = listener.local_addr().expect("local_addr");
        drop(listener);

        let config = WebSocketTransportConfig::default();
        let transport_handle =
            tokio::spawn(async move { run_websocket_transport(bound_addr, app, config).await });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let url = format!("ws://{bound_addr}");
        let (ws, _resp) = connect_async(&url).await.expect("WS connect");

        // Drop without sending anything. Server loops for next client.
        drop(ws);
        tokio::time::sleep(Duration::from_millis(200)).await;
        transport_handle.abort();
    }

    // -------------------------------------------------------------------
    // 3. TCP connect without WS upgrade times out, server continues
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn tcp_without_upgrade_times_out_and_server_continues() {
        use tokio::net::TcpStream;

        let app = test_app("ws-tcp-no-upgrade").await;

        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpListener::bind(addr).await.expect("bind");
        let bound_addr = listener.local_addr().expect("local_addr");
        drop(listener);

        // Use a 1-second handshake timeout so the test is fast.
        let config = WebSocketTransportConfig {
            handshake_timeout: Duration::from_secs(1),
            ..Default::default()
        };

        let transport_handle =
            tokio::spawn(async move { run_websocket_transport(bound_addr, app, config).await });

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect via raw TCP — no WS upgrade sent.
        let _tcp = TcpStream::connect(bound_addr).await.expect("TCP connect");

        // Wait for the 1s timeout to fire.
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Now connect with a real WS client — server should still accept.
        let url = format!("ws://{bound_addr}");
        let (mut ws, _) = connect_async(&url).await.expect("WS connect after timeout");

        // Send initialize — should get a response.
        let init_msg = serde_json::to_string(&serde_json::json!({
            "type": "request",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "test", "version": "0.1" }
            }
        }))
        .unwrap();
        ws.send(Message::Text(init_msg.into())).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
            .expect("stream end")
            .expect("WS error");
        let text = match resp {
            Message::Text(t) => t,
            other => panic!("expected text, got: {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(&text).expect("JSON");
        assert!(matches!(parsed, ServerMessage::Response(_)));

        ws.send(Message::Close(None)).await.unwrap();
        transport_handle.abort();
    }
}
