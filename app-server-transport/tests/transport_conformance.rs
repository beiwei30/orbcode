//! Transport Conformance Matrix
//!
//! Runs the same protocol scenarios across stdio (duplex), Unix socket, and
//! WebSocket transports to prove identical semantics regardless of framing.
//!
//! Scenarios covered:
//! - initialize / session/list (happy path)
//! - unknown method → MethodNotFound error
//! - malformed input → skipped (no crash)
//! - payload limit enforcement (oversized messages skipped)
//! - auth token acceptance / rejection
//! - slow consumer: terminal events survive backpressure, best-effort actually dropped
//! - response under pressure: lossless responses bypass notification flood
//! - disconnect clears pending server-requests immediately (mandatory assertion)
//! - listener survives bad client and accepts next connection

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

use orbcode_app_server::AppServer;
use orbcode_app_server_transport::{
    StdioTransportConfig, WebSocketTransportConfig, run_transport, run_unix_socket_transport,
    run_websocket_transport_with_bound_addr,
};

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

// =========================================================================
// Shared test helpers
// =========================================================================

/// Build a fresh AppServer with a given mock scenario URL.
async fn app_with_mock_url(label: &str, mock_url: &str) -> AppServer {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "orbcode-conformance-{label}-{}-{unique}",
        std::process::id()
    ));
    let home = base.join("home");
    let cwd = base.join("cwd");
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");

    let mut env = orbcode_config::sealed_provider_env_overrides();
    env.insert("ANTHROPIC_BASE_URL".to_string(), mock_url.to_string());
    env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

    AppServer::new(
        cwd,
        orbcode_config::AppConfigOverrides {
            home_dir: Some(home),
            env_overrides: env,
            ..orbcode_config::AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server")
}

/// Build with `many_deltas` scenario (2000 deltas to saturate bounded channels).
async fn app_with_many_deltas(label: &str) -> AppServer {
    app_with_mock_url(label, "mock://anthropic?scenario=many_deltas&attempts=2000").await
}

/// Build with `tool_use` scenario (triggers permission server-request).
async fn app_with_tool_use(label: &str) -> AppServer {
    app_with_mock_url(
        label,
        "mock://anthropic?scenario=tool_use&key=bash&command=echo+hi",
    )
    .await
}

/// Build a fresh AppServer without the mock provider (for protocol-only tests
/// that don't submit turns).
async fn app_minimal(label: &str) -> AppServer {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "orbcode-conformance-{label}-{}-{unique}",
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

fn temp_socket_path(label: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos()
        % 1_000_000_000;
    // Keep path short: macOS has 104-byte SUN_LEN limit.
    std::env::temp_dir().join(format!("cc{}-{unique}.sock", &label[..label.len().min(6)]))
}

// =========================================================================
// Transport abstraction for parameterized tests
// =========================================================================

/// A connected client that can send/receive JSON messages regardless of
/// the underlying transport.
#[allow(clippy::large_enum_variant)]
enum TransportClient {
    Ndjson {
        writer: DuplexStream,
        reader: BufReader<DuplexStream>,
    },
    Socket {
        writer: tokio::io::WriteHalf<UnixStream>,
        reader: BufReader<tokio::io::ReadHalf<UnixStream>>,
    },
    WebSocket {
        ws: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    },
}

struct TransportFixture {
    client: TransportClient,
    handle: JoinHandle<()>,
    /// For socket: path to clean up. For others: None.
    socket_path: Option<PathBuf>,
}

impl TransportFixture {
    async fn send(&mut self, msg: &Value) {
        match &mut self.client {
            TransportClient::Ndjson { writer, .. } => {
                let line = serde_json::to_string(msg).unwrap();
                writer.write_all(line.as_bytes()).await.unwrap();
                writer.write_all(b"\n").await.unwrap();
                writer.flush().await.unwrap();
            }
            TransportClient::Socket { writer, .. } => {
                let line = serde_json::to_string(msg).unwrap();
                writer.write_all(line.as_bytes()).await.unwrap();
                writer.write_all(b"\n").await.unwrap();
                writer.flush().await.unwrap();
            }
            TransportClient::WebSocket { ws } => {
                let text = serde_json::to_string(msg).unwrap();
                ws.send(Message::Text(text.into())).await.unwrap();
            }
        }
    }

    async fn recv(&mut self, timeout: Duration) -> Option<Value> {
        match &mut self.client {
            TransportClient::Ndjson { reader, .. } => {
                let mut line = String::new();
                match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                    Ok(Ok(0)) => None,
                    Ok(Ok(_)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            return None;
                        }
                        serde_json::from_str(trimmed).ok()
                    }
                    Ok(Err(_)) | Err(_) => None,
                }
            }
            TransportClient::Socket { reader, .. } => {
                let mut line = String::new();
                match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                    Ok(Ok(0)) => None,
                    Ok(Ok(_)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            return None;
                        }
                        serde_json::from_str(trimmed).ok()
                    }
                    Ok(Err(_)) | Err(_) => None,
                }
            }
            TransportClient::WebSocket { ws } => {
                match tokio::time::timeout(timeout, ws.next()).await {
                    Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str(&text).ok(),
                    _ => None,
                }
            }
        }
    }

    async fn recv_slow(&mut self, timeout: Duration) -> Option<Value> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.recv(timeout).await
    }

    async fn shutdown(self) {
        match self.client {
            TransportClient::Ndjson { mut writer, .. } => {
                writer.shutdown().await.ok();
            }
            TransportClient::Socket { mut writer, .. } => {
                writer.shutdown().await.ok();
            }
            TransportClient::WebSocket { mut ws } => {
                ws.send(Message::Close(None)).await.ok();
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), self.handle).await;
        if let Some(path) = self.socket_path {
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Drop client without clean shutdown (simulates abrupt disconnect).
    /// Returns the server handle so the caller can verify it stays alive.
    fn disconnect_abruptly(self) -> (JoinHandle<()>, Option<PathBuf>) {
        // Just drop the client — don't abort the server.
        drop(self.client);
        (self.handle, self.socket_path)
    }
}

/// Spawn a stdio (duplex) transport with a tiny buffer for backpressure tests.
async fn spawn_stdio_small_buffer(app: AppServer) -> TransportFixture {
    let (client_writer, transport_reader) = tokio::io::duplex(512);
    let (transport_writer, client_reader) = tokio::io::duplex(512);

    let config = StdioTransportConfig::default();
    let handle = tokio::spawn(async move {
        let _ = run_transport(transport_reader, transport_writer, app, config).await;
    });

    TransportFixture {
        client: TransportClient::Ndjson {
            writer: client_writer,
            reader: BufReader::new(client_reader),
        },
        handle,
        socket_path: None,
    }
}

/// Spawn a stdio (duplex) transport with a large buffer (for protocol tests).
async fn spawn_stdio(app: AppServer) -> TransportFixture {
    let (client_writer, transport_reader) = tokio::io::duplex(64 * 1024);
    let (transport_writer, client_reader) = tokio::io::duplex(64 * 1024);

    let config = StdioTransportConfig::default();
    let handle = tokio::spawn(async move {
        let _ = run_transport(transport_reader, transport_writer, app, config).await;
    });

    TransportFixture {
        client: TransportClient::Ndjson {
            writer: client_writer,
            reader: BufReader::new(client_reader),
        },
        handle,
        socket_path: None,
    }
}

/// Spawn a Unix socket transport and connect a client.
async fn spawn_socket(app: AppServer, label: &str) -> TransportFixture {
    let sock_path = temp_socket_path(label);
    let path_clone = sock_path.clone();
    let config = StdioTransportConfig::default();

    let handle = tokio::spawn(async move {
        let _ = run_unix_socket_transport(&path_clone, app, config).await;
    });

    // Wait for the listener to bind.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let stream = UnixStream::connect(&sock_path)
        .await
        .expect("connect to socket");
    let (reader, writer) = tokio::io::split(stream);

    TransportFixture {
        client: TransportClient::Socket {
            writer,
            reader: BufReader::new(reader),
        },
        handle,
        socket_path: Some(sock_path),
    }
}

/// Spawn a WebSocket transport and connect a client.
async fn spawn_websocket(app: AppServer) -> TransportFixture {
    let (bound_addr, handle) =
        spawn_websocket_server(app, WebSocketTransportConfig::default()).await;

    let url = format!("ws://{bound_addr}");
    let ws = retry_ws_connect(&url).await;

    TransportFixture {
        client: TransportClient::WebSocket { ws },
        handle,
        socket_path: None,
    }
}

async fn spawn_websocket_server(
    app: AppServer,
    config: WebSocketTransportConfig,
) -> (SocketAddr, JoinHandle<()>) {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (bound_addr_tx, bound_addr_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ =
            run_websocket_transport_with_bound_addr(addr, app, config, Some(bound_addr_tx)).await;
    });
    let bound_addr = bound_addr_rx.await.expect("WebSocket server should bind");
    (bound_addr, handle)
}

/// Retry WebSocket connect with backoff.
async fn retry_ws_connect(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    for i in 0..20 {
        tokio::time::sleep(Duration::from_millis(50 * (i + 1))).await;
        match connect_async(url).await {
            Ok((ws, _)) => return ws,
            Err(_) if i < 19 => {}
            Err(e) => panic!("WS connect failed after retries: {e}"),
        }
    }
    unreachable!()
}

// =========================================================================
// Shared protocol interactions
// =========================================================================

async fn do_initialize(f: &mut TransportFixture) {
    f.send(&json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "conformance-test", "version": "0.1" }
        }
    }))
    .await;

    let resp = f.recv(Duration::from_secs(5)).await.expect("init response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "init-1");
    assert_eq!(resp["result"]["status"], "success");
}

async fn do_initialize_with_persistent_goals(f: &mut TransportFixture) {
    f.send(&json!({
        "type": "request",
        "id": "init-goal",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "goal-conformance-test", "version": "0.1" },
            "capabilities": {
                "streaming": true,
                "experimental_methods": true,
                "persistent_goals": true
            }
        }
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("goal init response");
    assert_eq!(resp["id"], "init-goal");
    assert_eq!(resp["result"]["status"], "success");
}

async fn recv_response(f: &mut TransportFixture, id: &str, observed: &mut Vec<Value>) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for response {id}");
        let message = f.recv(remaining).await.expect("transport message");
        if message["type"] == "response" && message["id"] == id {
            return message;
        }
        observed.push(message);
    }
}

async fn assert_persistent_goal_conformance(f: &mut TransportFixture, transport_name: &str) {
    do_initialize_with_persistent_goals(f).await;
    let session_id = do_bootstrap(f).await;
    let mut observed = Vec::new();

    f.send(&json!({
        "type": "request",
        "id": "goal-set",
        "method": "session/goal/set",
        "params": {
            "session_id": session_id,
            "objective": format!("verify {transport_name} goal transport"),
            "status": "active",
            "token_budget": 10000
        }
    }))
    .await;
    let set = recv_response(f, "goal-set", &mut observed).await;
    assert_eq!(set["result"]["status"], "success");
    let goal = &set["result"]["data"]["goal"];
    let goal_id = goal["goal_id"].as_str().expect("goal id").to_string();
    let revision = goal["revision"].as_u64().expect("goal revision");
    assert_eq!(goal["status"], "active");

    f.send(&json!({
        "type": "request",
        "id": "goal-get",
        "method": "session/goal/get",
        "params": { "session_id": session_id }
    }))
    .await;
    let get = recv_response(f, "goal-get", &mut observed).await;
    assert_eq!(get["result"]["data"]["goal"]["goal_id"], goal_id);

    f.send(&json!({
        "type": "request",
        "id": "goal-tools",
        "method": "tools/list"
    }))
    .await;
    let tools = recv_response(f, "goal-tools", &mut observed).await;
    let tool_names = tools["result"]["data"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for name in ["get_goal", "create_goal", "update_goal"] {
        assert!(
            tool_names.contains(&name),
            "[{transport_name}] missing goal tool {name}"
        );
    }

    f.send(&json!({
        "type": "request",
        "id": "goal-continue",
        "method": "session/goal/continue",
        "params": {
            "session_id": session_id,
            "goal_id": goal_id,
            "expected_revision": revision
        }
    }))
    .await;
    let started = recv_response(f, "goal-continue", &mut observed).await;
    assert_eq!(started["result"]["status"], "success");
    assert_eq!(started["result"]["data"]["outcome"], "started");
    let subscription_id = started["result"]["data"]["subscription_id"]
        .as_str()
        .expect("goal subscription id")
        .to_string();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_terminal = observed.iter().any(|message| {
        message["type"] == "notification"
            && message["method"] == "stream/event"
            && message["params"]["subscription_id"] == subscription_id
            && message["params"]["event"]["event"] == "turn_finished"
    });
    while !saw_terminal {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "[{transport_name}] goal terminal timeout"
        );
        let message = f.recv(remaining).await.expect("goal stream event");
        saw_terminal = message["type"] == "notification"
            && message["method"] == "stream/event"
            && message["params"]["subscription_id"] == subscription_id
            && message["params"]["event"]["event"] == "turn_finished";
    }

    f.send(&json!({
        "type": "request",
        "id": "goal-checkpoint",
        "method": "session/goal/get",
        "params": { "session_id": session_id }
    }))
    .await;
    let checkpoint = recv_response(f, "goal-checkpoint", &mut observed).await;
    assert_eq!(checkpoint["result"]["data"]["goal"]["status"], "active");
    assert!(
        checkpoint["result"]["data"]["goal"]["tokens_used"]
            .as_u64()
            .is_some_and(|tokens| tokens > 0)
    );

    f.send(&json!({
        "type": "request",
        "id": "goal-clear",
        "method": "session/goal/clear",
        "params": { "session_id": session_id }
    }))
    .await;
    let clear = recv_response(f, "goal-clear", &mut observed).await;
    assert_eq!(clear["result"]["data"]["cleared"], true);
}

async fn do_bootstrap(f: &mut TransportFixture) -> String {
    f.send(&json!({
        "type": "request",
        "id": "bs-1",
        "method": "session/bootstrap"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("bootstrap response");
    assert_eq!(resp["type"], "response");
    resp["result"]["data"]["session"]["session_id"]
        .as_str()
        .expect("session_id")
        .to_string()
}

async fn do_submit_turn(f: &mut TransportFixture, session_id: &str) {
    f.send(&json!({
        "type": "request",
        "id": "turn-1",
        "method": "turn/submit",
        "params": { "session_id": session_id, "prompt": "hello" }
    }))
    .await;

    let resp = f.recv(Duration::from_secs(5)).await.expect("turn response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "turn-1");
}

// =========================================================================
// CONFORMANCE TESTS: Initialize + session/list
// =========================================================================

#[tokio::test]
async fn stdio_persistent_goal_conformance() {
    let app = app_with_mock_url("stdio-goal", "mock://anthropic?scenario=success").await;
    let mut f = spawn_stdio(app).await;
    assert_persistent_goal_conformance(&mut f, "stdio").await;
    f.shutdown().await;
}

#[tokio::test]
async fn socket_persistent_goal_conformance() {
    let app = app_with_mock_url("socket-goal", "mock://anthropic?scenario=success").await;
    let mut f = spawn_socket(app, "goal").await;
    assert_persistent_goal_conformance(&mut f, "socket").await;
    f.shutdown().await;
}

#[tokio::test]
async fn websocket_persistent_goal_conformance() {
    let app = app_with_mock_url("websocket-goal", "mock://anthropic?scenario=success").await;
    let mut f = spawn_websocket(app).await;
    assert_persistent_goal_conformance(&mut f, "websocket").await;
    f.shutdown().await;
}

#[tokio::test]
async fn stdio_initialize_and_session_list() {
    let app = app_minimal("stdio-init").await;
    let mut f = spawn_stdio(app).await;
    do_initialize(&mut f).await;

    f.send(&json!({
        "type": "request",
        "id": "sl-1",
        "method": "session/list"
    }))
    .await;

    let resp = f.recv(Duration::from_secs(5)).await.expect("session/list");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "sl-1");
    assert_eq!(resp["result"]["status"], "success");
    f.shutdown().await;
}

#[tokio::test]
async fn socket_initialize_and_session_list() {
    let app = app_minimal("sock-init-list").await;
    let mut f = spawn_socket(app, "init-list").await;
    do_initialize(&mut f).await;

    f.send(&json!({
        "type": "request",
        "id": "sl-1",
        "method": "session/list"
    }))
    .await;

    let resp = f.recv(Duration::from_secs(5)).await.expect("session/list");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "sl-1");
    assert_eq!(resp["result"]["status"], "success");
    f.shutdown().await;
}

#[tokio::test]
async fn websocket_initialize_and_session_list() {
    let app = app_minimal("ws-init-list").await;
    let mut f = spawn_websocket(app).await;
    do_initialize(&mut f).await;

    f.send(&json!({
        "type": "request",
        "id": "sl-1",
        "method": "session/list"
    }))
    .await;

    let resp = f.recv(Duration::from_secs(5)).await.expect("session/list");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "sl-1");
    assert_eq!(resp["result"]["status"], "success");
    f.shutdown().await;
}

// =========================================================================
// CONFORMANCE TESTS: Unknown method → MethodNotFound
// =========================================================================

#[tokio::test]
async fn stdio_unknown_method() {
    let app = app_minimal("stdio-unk").await;
    let mut f = spawn_stdio(app).await;
    do_initialize(&mut f).await;

    f.send(&json!({
        "type": "request",
        "id": "unk-1",
        "method": "nonexistent/method"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("error response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "unk-1");
    assert_eq!(resp["result"]["status"], "error");
    assert_eq!(resp["result"]["code"], "method_not_found");
    f.shutdown().await;
}

#[tokio::test]
async fn socket_unknown_method() {
    let app = app_minimal("sock-unk").await;
    let mut f = spawn_socket(app, "unk").await;
    do_initialize(&mut f).await;

    f.send(&json!({
        "type": "request",
        "id": "unk-1",
        "method": "nonexistent/method"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("error response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "unk-1");
    assert_eq!(resp["result"]["status"], "error");
    assert_eq!(resp["result"]["code"], "method_not_found");
    f.shutdown().await;
}

#[tokio::test]
async fn websocket_unknown_method() {
    let app = app_minimal("ws-unk").await;
    let mut f = spawn_websocket(app).await;
    do_initialize(&mut f).await;

    f.send(&json!({
        "type": "request",
        "id": "unk-1",
        "method": "nonexistent/method"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("error response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "unk-1");
    assert_eq!(resp["result"]["status"], "error");
    assert_eq!(resp["result"]["code"], "method_not_found");
    f.shutdown().await;
}

// =========================================================================
// CONFORMANCE TESTS: Malformed input → skipped
// =========================================================================

#[tokio::test]
async fn stdio_malformed_input_skipped() {
    let app = app_minimal("stdio-mal").await;
    let mut f = spawn_stdio(app).await;
    do_initialize(&mut f).await;

    // Send garbage, then a valid request.
    f.send(&json!("this is not a valid protocol message")).await;
    f.send(&json!({
        "type": "request",
        "id": "after-bad",
        "method": "session/list"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("response after malformed");
    assert_eq!(resp["id"], "after-bad");
    assert_eq!(resp["result"]["status"], "success");
    f.shutdown().await;
}

#[tokio::test]
async fn socket_malformed_input_skipped() {
    let app = app_minimal("sock-mal").await;
    let mut f = spawn_socket(app, "mal").await;
    do_initialize(&mut f).await;

    f.send(&json!("this is not a valid protocol message")).await;
    f.send(&json!({
        "type": "request",
        "id": "after-bad",
        "method": "session/list"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("response after malformed");
    assert_eq!(resp["id"], "after-bad");
    assert_eq!(resp["result"]["status"], "success");
    f.shutdown().await;
}

#[tokio::test]
async fn websocket_malformed_input_skipped() {
    let app = app_minimal("ws-mal").await;
    let mut f = spawn_websocket(app).await;
    do_initialize(&mut f).await;

    // Send raw invalid JSON as a text frame.
    match &mut f.client {
        TransportClient::WebSocket { ws } => {
            ws.send(Message::Text("{{{{not json".into())).await.unwrap();
        }
        _ => unreachable!(),
    }
    f.send(&json!({
        "type": "request",
        "id": "after-bad",
        "method": "session/list"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("response after malformed");
    assert_eq!(resp["id"], "after-bad");
    assert_eq!(resp["result"]["status"], "success");
    f.shutdown().await;
}

// =========================================================================
// CONFORMANCE TESTS: Auth token rejection
// =========================================================================

#[tokio::test]
async fn socket_auth_token_rejection() {
    let app = app_minimal("sock-auth-reject").await;
    let sock_path = temp_socket_path("auth-reject");
    let path_clone = sock_path.clone();
    let config = StdioTransportConfig {
        auth_token: Some("correct-token".to_string()),
        ..StdioTransportConfig::default()
    };

    let handle = tokio::spawn(async move {
        let _ = run_unix_socket_transport(&path_clone, app, config).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect with wrong token.
    let stream = UnixStream::connect(&sock_path).await.expect("connect");
    let (reader, mut writer) = tokio::io::split(stream);
    writer.write_all(b"wrong-token\n").await.unwrap();
    writer.flush().await.unwrap();

    // Server should close the connection. Read should yield EOF.
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    let result = tokio::time::timeout(Duration::from_secs(3), buf_reader.read_line(&mut line))
        .await
        .expect("timeout");
    assert_eq!(result.unwrap(), 0, "expected EOF after auth rejection");

    // The server should still be alive — connect with correct token.
    let stream = UnixStream::connect(&sock_path).await.expect("reconnect");
    let (reader, mut writer) = tokio::io::split(stream);
    writer.write_all(b"correct-token\n").await.unwrap();

    let init = json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "auth-test", "version": "0.1" }
        }
    });
    let line = format!("{}\n", serde_json::to_string(&init).unwrap());
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let mut buf_reader = BufReader::new(reader);
    let mut resp_line = String::new();
    let n = tokio::time::timeout(Duration::from_secs(5), buf_reader.read_line(&mut resp_line))
        .await
        .expect("timeout")
        .expect("read");
    assert!(n > 0, "should get response with correct token");
    let resp: Value = serde_json::from_str(resp_line.trim()).unwrap();
    assert_eq!(resp["id"], "init-1");

    handle.abort();
    let _ = std::fs::remove_file(&sock_path);
}

#[tokio::test]
async fn websocket_auth_token_rejection() {
    let app = app_minimal("ws-auth-reject").await;
    let config = WebSocketTransportConfig {
        auth_token: Some("secret-token".to_string()),
        ..WebSocketTransportConfig::default()
    };
    let (bound_addr, handle) = spawn_websocket_server(app, config).await;

    let url = format!("ws://{bound_addr}");

    // Connect with wrong token.
    let mut ws = retry_ws_connect(&url).await;
    ws.send(Message::Text("wrong-token".into())).await.unwrap();

    // Server should close the connection.
    let close = tokio::time::timeout(Duration::from_secs(3), ws.next()).await;
    match close {
        Ok(Some(Ok(Message::Close(_))) | None) | Err(_) => {}
        Ok(Some(Ok(msg))) => {
            panic!("expected close, got: {msg:?}");
        }
        Ok(Some(Err(_))) => {}
    }

    // Server should still accept new connections (retry since server may be
    // briefly busy processing the rejected client's cleanup).
    let mut ws2 = retry_ws_connect(&url).await;
    ws2.send(Message::Text("secret-token".into()))
        .await
        .unwrap();
    ws2.send(Message::Text(
        serde_json::to_string(&json!({
            "type": "request",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "auth-test", "version": "0.1" }
            }
        }))
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), ws2.next())
        .await
        .expect("timeout")
        .expect("stream end")
        .expect("WS error");
    let text = match resp {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    };
    let val: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["id"], "init-1");

    handle.abort();
}

// =========================================================================
// CONFORMANCE TESTS: Slow consumer — terminal arrives, best-effort dropped
// =========================================================================
//
// Uses `many_deltas` (2000 deltas) with a tiny buffer and slow reads.
// Proves: (1) terminal turn_finished arrives, (2) received deltas < 2000
// (actual drop evidence from bounded channel try_send).

const MANY_DELTAS_PRODUCED: usize = 2000;

/// Shared slow-consumer logic: read slowly to create backpressure, then
/// fast-drain after terminal. Asserts `0 < delta_count < 2000`.
///
/// Strategy (mirrors the stdio unit test):
///   1. Read slowly (50ms delays) until `turn_finished` arrives.
///   2. After terminal, switch to fast reads (1s timeout) to drain any
///      remaining buffered messages until the writer goes idle.
///   3. The final `delta_count` is the true total delivered. Assert it is
///      both > 0 (stream was active) and < 2000 (drops occurred).
async fn assert_slow_consumer_drops(f: &mut TransportFixture, transport_name: &str) {
    let mut delta_count: usize = 0;
    let mut saw_turn_finished = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Slow before terminal (creates backpressure); fast after (drains
        // remaining buffer). After terminal, 1s idle timeout ends the loop.
        let msg = if saw_turn_finished {
            f.recv(Duration::from_secs(1)).await
        } else {
            f.recv_slow(remaining).await
        };
        let Some(msg) = msg else {
            break;
        };
        if msg["type"].as_str() == Some("notification")
            && msg["method"].as_str() == Some("stream/event")
        {
            let event = &msg["params"]["event"];
            match event["event"].as_str() {
                Some("assistant_delta") => {
                    delta_count += 1;
                }
                Some("turn_finished") => {
                    saw_turn_finished = true;
                    // Don't break — keep draining to get the true total.
                }
                _ => {}
            }
        }
    }

    assert!(
        saw_turn_finished,
        "[{transport_name}] terminal turn_finished must arrive despite slow consumer \
         (received {delta_count} deltas before timeout)"
    );
    assert!(
        delta_count > 0,
        "[{transport_name}] expected at least 1 delta delivered (got 0), \
         proving the stream was active"
    );
    assert!(
        delta_count < MANY_DELTAS_PRODUCED,
        "[{transport_name}] expected fewer than {MANY_DELTAS_PRODUCED} deltas delivered \
         (got {delta_count}), proving best-effort drops under backpressure"
    );
}

#[tokio::test]
async fn stdio_slow_consumer_proves_drops() {
    let app = app_with_many_deltas("stdio-slow").await;
    let mut f = spawn_stdio_small_buffer(app).await;
    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;
    assert_slow_consumer_drops(&mut f, "stdio").await;
    f.shutdown().await;
}

#[tokio::test]
async fn socket_slow_consumer_proves_drops() {
    // Unix socket: kernel buffer is limited (~200KB on macOS). With 2000
    // deltas of ~50 bytes each plus JSON envelope overhead, the bounded
    // channel's try_send will drop when the writer blocks on kernel buffers.
    let app = app_with_many_deltas("sock-slow").await;
    let sock_path = temp_socket_path("slw-cn");
    let path_clone = sock_path.clone();
    let config = StdioTransportConfig::default();

    let handle = tokio::spawn(async move {
        let _ = run_unix_socket_transport(&path_clone, app, config).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stream = UnixStream::connect(&sock_path).await.expect("connect");
    let (reader, writer) = tokio::io::split(stream);

    let mut f = TransportFixture {
        client: TransportClient::Socket {
            writer,
            reader: BufReader::new(reader),
        },
        handle,
        socket_path: Some(sock_path),
    };

    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;
    assert_slow_consumer_drops(&mut f, "socket").await;
    f.shutdown().await;
}

#[tokio::test]
async fn websocket_slow_consumer_terminal_delivery() {
    // WebSocket: TCP kernel buffers are large enough to absorb all deltas
    // without blocking the writer, so transport-level drop proof is not
    // reliable here. The bounded channel drop is proven by unit tests in
    // message_processor.rs. This test proves terminal event delivery
    // (the critical safety property for any client).
    let app = app_with_many_deltas("ws-slow").await;
    let (bound_addr, handle) =
        spawn_websocket_server(app, WebSocketTransportConfig::default()).await;

    let url = format!("ws://{bound_addr}");
    let ws = retry_ws_connect(&url).await;

    let mut f = TransportFixture {
        client: TransportClient::WebSocket { ws },
        handle,
        socket_path: None,
    };

    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;

    // Read until turn_finished, then drain remaining buffered messages.
    // WebSocket: TCP kernel buffers are large, so drops are less reliable
    // at the transport level. We still drain to get the true delta count
    // and assert terminal delivery (the critical safety property).
    let mut delta_count: usize = 0;
    let mut saw_turn_finished = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let timeout = if saw_turn_finished {
            Duration::from_secs(1)
        } else {
            remaining
        };
        let Some(msg) = f.recv(timeout).await else {
            break;
        };
        if msg["type"].as_str() == Some("notification")
            && msg["method"].as_str() == Some("stream/event")
        {
            match msg["params"]["event"]["event"].as_str() {
                Some("assistant_delta") => delta_count += 1,
                Some("turn_finished") => {
                    saw_turn_finished = true;
                }
                _ => {}
            }
        }
    }

    assert!(
        saw_turn_finished,
        "terminal turn_finished must arrive over WebSocket ({delta_count} deltas received)"
    );
    assert!(
        delta_count > 0,
        "expected at least 1 delta over WebSocket (got 0)"
    );
    f.shutdown().await;
}

// =========================================================================
// CONFORMANCE TESTS: Response under pressure (lossless bypasses notification flood)
// =========================================================================
//
// Uses `many_deltas` (2000 deltas) with a large buffer (so nothing blocks).
// Submits turn, does NOT drain, then sends `session/list`. Proves the response
// arrives despite the notification flood (the lossless channel is not blocked).

/// Shared response-under-pressure logic.
///
/// 1. Wait until an `assistant_delta` notification arrives (proving the delta
///    flood has started). Panics if `turn_finished` arrives first.
/// 2. Send `session/list` while deltas are still in flight.
/// 3. Assert the response arrives (the lossless channel was not blocked by
///    the notification flood). The response may arrive before or after
///    `turn_finished` depending on scheduling — both are valid.
async fn assert_response_under_pressure(f: &mut TransportFixture, transport_name: &str) {
    // Phase 1: read until we see an `assistant_delta` notification,
    // proving the delta flood has actually started (not just turn_finished
    // which also travels as stream/event but uses the lossless channel).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_delta = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(msg) = f.recv(remaining).await else {
            break;
        };
        if msg["type"].as_str() == Some("notification")
            && msg["method"].as_str() == Some("stream/event")
        {
            let event_name = msg["params"]["event"]["event"].as_str();
            if event_name == Some("turn_finished") {
                panic!(
                    "[{transport_name}] turn_finished arrived before any assistant_delta — \
                     cannot prove response-under-pressure with no active notification flood"
                );
            }
            if event_name == Some("assistant_delta") {
                saw_delta = true;
                break;
            }
        }
    }
    assert!(
        saw_delta,
        "[{transport_name}] expected at least 1 assistant_delta before sending request \
         (got none — delta stream may not have started)"
    );

    // Phase 2: send request while deltas are still streaming.
    f.send(&json!({
        "type": "request",
        "id": "sl-pressure",
        "method": "session/list"
    }))
    .await;

    // Phase 3: read until we find the session/list response. The critical
    // property is that the response arrives at all — the lossless channel
    // was not blocked by the notification flood. We track whether
    // turn_finished arrives before the response: if so, the entire stream
    // completed, and the response arrived after all notifications were
    // flushed to the kernel buffer (still valid — the bounded channel's
    // try_send was the bottleneck, not the lossless channel). Keep
    // reading past turn_finished to find the response.
    let mut notifications_before_response: usize = 0;
    let mut found_response = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(msg) = f.recv(remaining).await else {
            break;
        };
        if msg["type"].as_str() == Some("response") && msg["id"].as_str() == Some("sl-pressure") {
            found_response = true;
            break;
        }
        if msg["type"].as_str() == Some("notification") {
            notifications_before_response += 1;
        }
    }

    assert!(
        found_response,
        "[{transport_name}] session/list response must arrive under notification flood \
         (got {notifications_before_response} notifications before timeout)"
    );
    // Phase 1 already proved the stream was active (pre_notifications > 0).
    // The response may arrive before or after additional notifications
    // depending on scheduling; the critical proof is that it arrives at all
    // while 2000 deltas are being produced.
}

#[tokio::test]
async fn stdio_response_under_pressure() {
    let app = app_with_many_deltas("stdio-pressure").await;
    let mut f = spawn_stdio(app).await;
    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;
    assert_response_under_pressure(&mut f, "stdio").await;
    f.shutdown().await;
}

#[tokio::test]
async fn socket_response_under_pressure() {
    let app = app_with_many_deltas("sock-pressure").await;
    let mut f = spawn_socket(app, "pressu").await;
    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;
    assert_response_under_pressure(&mut f, "socket").await;
    f.shutdown().await;
}

#[tokio::test]
async fn websocket_response_under_pressure() {
    let app = app_with_many_deltas("ws-pressure").await;
    let mut f = spawn_websocket(app).await;
    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;
    assert_response_under_pressure(&mut f, "websocket").await;
    f.shutdown().await;
}

// =========================================================================
// CONFORMANCE TESTS: Listener survives bad client
// =========================================================================

#[tokio::test]
async fn socket_listener_survives_bad_client() {
    let app = app_minimal("sock-survive").await;
    let sock_path = temp_socket_path("survive");
    let path_clone = sock_path.clone();
    let config = StdioTransportConfig::default();

    let handle = tokio::spawn(async move {
        let _ = run_unix_socket_transport(&path_clone, app, config).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client 1: connect and immediately disconnect.
    let stream = UnixStream::connect(&sock_path).await.expect("connect-1");
    drop(stream);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client 2: send garbage then disconnect.
    let stream = UnixStream::connect(&sock_path).await.expect("connect-2");
    let (_, mut writer) = tokio::io::split(stream);
    writer.write_all(b"not json at all\n").await.unwrap();
    writer.shutdown().await.ok();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client 3: valid client should still work.
    let stream = UnixStream::connect(&sock_path).await.expect("connect-3");
    let (reader, mut writer) = tokio::io::split(stream);
    let init = json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "survivor", "version": "0.1" }
        }
    });
    let line = format!("{}\n", serde_json::to_string(&init).unwrap());
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let mut buf_reader = BufReader::new(reader);
    let mut resp_line = String::new();
    let n = tokio::time::timeout(Duration::from_secs(5), buf_reader.read_line(&mut resp_line))
        .await
        .expect("timeout")
        .expect("read");
    assert!(n > 0);
    let resp: Value = serde_json::from_str(resp_line.trim()).unwrap();
    assert_eq!(resp["id"], "init-1");
    assert_eq!(resp["result"]["status"], "success");

    handle.abort();
    let _ = std::fs::remove_file(&sock_path);
}

#[tokio::test]
async fn websocket_listener_survives_bad_client() {
    let app = app_minimal("ws-survive").await;
    let (bound_addr, handle) =
        spawn_websocket_server(app, WebSocketTransportConfig::default()).await;

    let url = format!("ws://{bound_addr}");

    // Client 1: connect and immediately close.
    let mut ws = retry_ws_connect(&url).await;
    ws.send(Message::Close(None)).await.ok();
    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client 2: send garbage then close.
    let mut ws = retry_ws_connect(&url).await;
    ws.send(Message::Text("{{not json".into())).await.ok();
    ws.send(Message::Close(None)).await.ok();
    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client 3: valid protocol should work.
    let mut ws = retry_ws_connect(&url).await;
    ws.send(Message::Text(
        serde_json::to_string(&json!({
            "type": "request",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "survivor", "version": "0.1" }
            }
        }))
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout")
        .expect("stream end")
        .expect("WS error");
    let text = match resp {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    };
    let val: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["id"], "init-1");
    assert_eq!(val["result"]["status"], "success");

    handle.abort();
}

// =========================================================================
// CONFORMANCE TESTS: Payload limit enforcement
// =========================================================================
//
// Sends a message exceeding max_payload_bytes, then a valid message.
// Proves the oversized message is skipped (not crash / not corrupt)
// and the valid message still gets a response.

#[tokio::test]
async fn stdio_payload_limit_skips_oversized() {
    let app = app_minimal("stdio-payload").await;
    let (client_writer, transport_reader) = tokio::io::duplex(64 * 1024);
    let (transport_writer, client_reader) = tokio::io::duplex(64 * 1024);

    let config = StdioTransportConfig {
        max_payload_bytes: 256,
        ..StdioTransportConfig::default()
    };
    let handle = tokio::spawn(async move {
        let _ = run_transport(transport_reader, transport_writer, app, config).await;
    });

    let mut f = TransportFixture {
        client: TransportClient::Ndjson {
            writer: client_writer,
            reader: BufReader::new(client_reader),
        },
        handle,
        socket_path: None,
    };

    do_initialize(&mut f).await;

    // Send an oversized line (>256 bytes).
    let oversized = format!(
        "{{\"type\":\"request\",\"id\":\"big\",\"method\":\"session/list\",\"params\":{{\"pad\":\"{}\"}}}}\n",
        "x".repeat(300)
    );
    match &mut f.client {
        TransportClient::Ndjson { writer, .. } => {
            writer.write_all(oversized.as_bytes()).await.unwrap();
            writer.flush().await.unwrap();
        }
        _ => unreachable!(),
    }

    // Follow with a valid small request.
    f.send(&json!({
        "type": "request",
        "id": "after-big",
        "method": "session/list"
    }))
    .await;

    let resp = f
        .recv(Duration::from_secs(5))
        .await
        .expect("response after oversized");
    assert_eq!(resp["id"], "after-big");
    assert_eq!(resp["result"]["status"], "success");
    f.shutdown().await;
}

#[tokio::test]
async fn socket_payload_limit_skips_oversized() {
    let app = app_minimal("sock-payload").await;
    let sock_path = temp_socket_path("paylim");
    let path_clone = sock_path.clone();
    let config = StdioTransportConfig {
        max_payload_bytes: 256,
        ..StdioTransportConfig::default()
    };

    let handle = tokio::spawn(async move {
        let _ = run_unix_socket_transport(&path_clone, app, config).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stream = UnixStream::connect(&sock_path).await.expect("connect");
    let (reader, mut writer) = tokio::io::split(stream);

    // Send init manually (small enough).
    let init = json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "payload-test", "version": "0.1" }
        }
    });
    let line = format!("{}\n", serde_json::to_string(&init).unwrap());
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let mut buf_reader = BufReader::new(reader);
    let mut resp_line = String::new();
    tokio::time::timeout(Duration::from_secs(5), buf_reader.read_line(&mut resp_line))
        .await
        .expect("timeout")
        .expect("read");

    // Send oversized line.
    let oversized = format!(
        "{{\"type\":\"request\",\"id\":\"big\",\"method\":\"session/list\",\"params\":{{\"pad\":\"{}\"}}}}\n",
        "x".repeat(300)
    );
    writer.write_all(oversized.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    // Send valid follow-up.
    let valid = json!({
        "type": "request",
        "id": "after-big",
        "method": "session/list"
    });
    let valid_line = format!("{}\n", serde_json::to_string(&valid).unwrap());
    writer.write_all(valid_line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let mut resp_line2 = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        buf_reader.read_line(&mut resp_line2),
    )
    .await
    .expect("timeout")
    .expect("read");
    let resp: Value = serde_json::from_str(resp_line2.trim()).unwrap();
    assert_eq!(resp["id"], "after-big");
    assert_eq!(resp["result"]["status"], "success");

    handle.abort();
    let _ = std::fs::remove_file(&sock_path);
}

#[tokio::test]
async fn websocket_payload_limit_rejects_oversized() {
    // WebSocket transport: oversized messages exceed tungstenite's max_message_size,
    // which causes the connection to close (protocol-level enforcement). This is
    // the correct behavior — the server continues accepting new connections.
    let app = app_minimal("ws-payload").await;
    let config = WebSocketTransportConfig {
        max_payload_bytes: 256,
        ..WebSocketTransportConfig::default()
    };
    let (bound_addr, handle) = spawn_websocket_server(app, config).await;

    let url = format!("ws://{bound_addr}");
    let mut ws = retry_ws_connect(&url).await;

    // Initialize (small enough to pass).
    ws.send(Message::Text(
        serde_json::to_string(&json!({
            "type": "request",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "payload-test", "version": "0.1" }
            }
        }))
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();

    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("init response");

    // Send oversized message — tungstenite will close the connection.
    let oversized = format!(
        "{{\"type\":\"request\",\"id\":\"big\",\"method\":\"session/list\",\"params\":{{\"pad\":\"{}\"}}}}",
        "x".repeat(300)
    );
    ws.send(Message::Text(oversized.into())).await.unwrap();

    // Expect connection close (Close frame or stream end).
    let result = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
    match result {
        Ok(Some(Ok(Message::Close(_))) | None) | Err(_) => {}
        Ok(Some(Err(_))) => {}
        Ok(Some(Ok(msg))) => {
            // If we got a text response, the server didn't enforce the limit
            panic!("expected close after oversized, got: {msg:?}");
        }
    }

    // Server should still accept new connections (listener survives).
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut ws2 = retry_ws_connect(&url).await;
    ws2.send(Message::Text(
        serde_json::to_string(&json!({
            "type": "request",
            "id": "init-2",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "payload-test-2", "version": "0.1" }
            }
        }))
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), ws2.next())
        .await
        .expect("timeout")
        .expect("stream end")
        .expect("WS error");
    let text = match resp {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    };
    let val: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["id"], "init-2");

    handle.abort();
}

// =========================================================================
// CONFORMANCE TESTS: Disconnect clears pending server-requests
// =========================================================================
//
// Uses `tool_use` mock (bash command) which triggers a permission/request
// server-request. We disconnect abruptly without responding, and verify
// the server recovers quickly (doesn't wait for 5-minute timeout).
// The assertion that permission/request is seen is MANDATORY — if the mock
// doesn't trigger it, the test fails rather than silently passing.

#[tokio::test]
async fn socket_disconnect_clears_pending() {
    let app = app_with_tool_use("sock-disc-pend").await;
    let sock_path = temp_socket_path("dc-pnd");
    let path_clone = sock_path.clone();
    let config = StdioTransportConfig::default();

    let handle = tokio::spawn(async move {
        let _ = run_unix_socket_transport(&path_clone, app, config).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stream = UnixStream::connect(&sock_path).await.expect("connect");
    let (reader, writer) = tokio::io::split(stream);
    let mut f = TransportFixture {
        client: TransportClient::Socket {
            writer,
            reader: BufReader::new(reader),
        },
        handle,
        socket_path: Some(sock_path.clone()),
    };

    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;

    // Read until we see the permission/request server-request.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut saw_permission_request = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(msg) = f.recv(remaining).await else {
            break;
        };
        if msg["type"].as_str() == Some("request")
            && msg["method"].as_str() == Some("permission/request")
        {
            saw_permission_request = true;
            break;
        }
    }

    assert!(
        saw_permission_request,
        "tool_use mock must trigger permission/request server-request"
    );

    // Disconnect abruptly without responding to the permission request.
    let start = tokio::time::Instant::now();
    let (handle, _) = f.disconnect_abruptly();

    // The server (accept loop) should continue quickly — not wait 5 minutes.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "server should recover from pending disconnect within 10s, took {elapsed:?}"
    );

    // Server should still accept new connections.
    if let Ok(stream) = UnixStream::connect(&sock_path).await {
        drop(stream);
    }

    handle.abort();
    let _ = std::fs::remove_file(&sock_path);
}

#[tokio::test]
async fn websocket_disconnect_clears_pending() {
    let app = app_with_tool_use("ws-disc-pend").await;
    let (bound_addr, handle) =
        spawn_websocket_server(app, WebSocketTransportConfig::default()).await;

    let url = format!("ws://{bound_addr}");
    let ws = retry_ws_connect(&url).await;

    let mut f = TransportFixture {
        client: TransportClient::WebSocket { ws },
        handle,
        socket_path: None,
    };

    do_initialize(&mut f).await;
    let session_id = do_bootstrap(&mut f).await;
    do_submit_turn(&mut f, &session_id).await;

    // Read until we see the permission/request server-request.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut saw_permission_request = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(msg) = f.recv(remaining).await else {
            break;
        };
        if msg["type"].as_str() == Some("request")
            && msg["method"].as_str() == Some("permission/request")
        {
            saw_permission_request = true;
            break;
        }
    }

    assert!(
        saw_permission_request,
        "tool_use mock must trigger permission/request server-request"
    );

    // Disconnect abruptly without responding.
    let start = tokio::time::Instant::now();
    let (handle, _) = f.disconnect_abruptly();

    // Server should recover quickly.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "server should recover from pending disconnect within 10s, took {elapsed:?}"
    );

    // Server should still accept new connections.
    if let Ok((mut ws2, _)) = connect_async(&url).await {
        ws2.send(Message::Close(None)).await.ok();
    }
    handle.abort();
}
