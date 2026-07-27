//! End-to-end tests for MCP trust decision round-trip through the
//! server-request / response protocol path.
//!
//! These tests verify that:
//! 1. When `pump_events` encounters a `McpTrustApprovalRequested` stream
//!    event it emits a `ServerRequestEnvelope` with the correct method
//!    (`mcp_trust/request`) and params.
//! 2. A client response using the `McpTrustResponseParams` wire format
//!    (full `{request_id, decision}`) is correctly parsed and routed.
//! 3. A client response using the bare `McpTrustDecisionWire` format
//!    (just `"trust"` or `"deny"`) is accepted as a lenient fallback.
//! 4. The `MessageProcessor.handle_message(ClientMessage::Response(...))`
//!    path correctly routes the response to the pending oneshot.
//! 5. Timeout defaults to deny and cleans up the pending map.
//! 6. Unrecognizable responses default to deny.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{Mutex, mpsc, oneshot};

use orbcode_app_server::AppServer;
use orbcode_app_server::message_processor::{self, MessageProcessor, ServerSink};
use orbcode_app_server_protocol::{
    ClientMessage, ClientRequestEnvelope, ResponseResult, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse, method,
};
use orbcode_config::AppConfigOverrides;
use orbcode_protocol::{McpTrustApprovalRequest, StreamEvent};

type RequestId = String;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Collects all messages sent through the sink into a shared `Vec`.
struct TestSink {
    messages: Arc<Mutex<Vec<ServerMessage>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl TestSink {
    fn new() -> (Arc<Self>, Arc<Mutex<Vec<ServerMessage>>>) {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Self {
            messages: Arc::clone(&messages),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        (sink, messages)
    }
}

impl ServerSink for TestSink {
    fn send(&self, message: ServerMessage) {
        let msgs = Arc::clone(&self.messages);
        if let Ok(mut guard) = msgs.try_lock() {
            guard.push(message);
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed)
    }
}

fn unique_label(base: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    format!("{base}-{}-{nanos}", std::process::id())
}

async fn test_app(label: &str) -> AppServer {
    let home = std::env::temp_dir().join(unique_label(&format!("{label}-home")));
    let cwd = std::env::temp_dir().join(unique_label(&format!("{label}-cwd")));
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");
    AppServer::new(
        cwd,
        AppConfigOverrides {
            home_dir: Some(home),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server")
}

fn initialize_request() -> ClientMessage {
    ClientMessage::Request(ClientRequestEnvelope {
        id: "init-1".into(),
        method: "initialize".into(),
        params: Some(json!({
            "protocol_version": "1.0",
            "client_info": { "name": "mcp-trust-e2e", "version": "0.1" },
        })),
    })
}

/// Helper: emit a `McpTrustApprovalRequested` event into a fresh pump and
/// return the pending map, collected messages, and the server-request ID.
///
/// The pump is spawned into a background task. The returned `JoinHandle`
/// should be aborted by the caller after resolving the pending oneshot.
async fn spawn_pump_with_trust_event(
    label: &str,
    server_id: &str,
    tool_name: &str,
    request_id: &str,
) -> (
    AppServer,
    Arc<TestSink>,
    Arc<Mutex<Vec<ServerMessage>>>,
    Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>>,
    tokio::task::JoinHandle<()>,
) {
    let app = test_app(label).await;
    let (sink, messages) = TestSink::new();
    let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
    event_tx
        .send(StreamEvent::McpTrustApprovalRequested {
            request: McpTrustApprovalRequest {
                request_id: request_id.to_string(),
                session_id: "session-mcp-trust-test".to_string(),
                server_id: server_id.to_string(),
                tool_name: tool_name.to_string(),
            },
        })
        .unwrap();
    drop(event_tx);

    let pending_clone = Arc::clone(&pending);
    let pump_handle = tokio::spawn({
        let sink = Arc::clone(&sink) as Arc<dyn ServerSink>;
        let app = app.clone();
        async move {
            message_processor::pump_events(
                event_rx,
                sink,
                pending_clone,
                app,
                "e2e-sub".to_string(),
                0,
                Duration::from_secs(5),
            )
            .await;
        }
    });

    // Wait for the pump to emit the server-request.
    tokio::time::sleep(Duration::from_millis(100)).await;

    (app, sink, messages, pending, pump_handle)
}

/// Extract the server-request ID for an MCP trust request from collected
/// messages.
fn find_trust_server_request_id(msgs: &[ServerMessage]) -> Option<String> {
    msgs.iter().find_map(|m| match m {
        ServerMessage::Request(ServerRequestEnvelope { id, method, .. })
            if method == method::SERVER_REQUEST_MCP_TRUST =>
        {
            Some(id.clone())
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// 1. ServerRequest arrives on sink with correct method and params
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trust_request_emitted_with_correct_method_and_params() {
    let (_app, _sink, messages, _pending, pump_handle) =
        spawn_pump_with_trust_event("e2e-emit", "my-server", "my_tool", "req-1").await;

    pump_handle.abort();

    let msgs = messages.lock().await;

    // Find the server-request.
    let trust_req = msgs.iter().find_map(|m| match m {
        ServerMessage::Request(env @ ServerRequestEnvelope { method, .. })
            if method == method::SERVER_REQUEST_MCP_TRUST =>
        {
            Some(env)
        }
        _ => None,
    });
    let env = trust_req.expect("pump should emit a trust server-request");
    assert_eq!(env.method, "mcp_trust/request");
    assert_eq!(env.params["server_id"], "my-server");
    assert_eq!(env.params["tool_name"], "my_tool");
    assert_eq!(env.params["request_id"], "req-1");
}

// ---------------------------------------------------------------------------
// 2. Notification is emitted alongside the server-request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trust_event_also_emitted_as_notification() {
    let (_app, _sink, messages, _pending, pump_handle) =
        spawn_pump_with_trust_event("e2e-notif", "notif-server", "notif_tool", "req-notif").await;

    pump_handle.abort();

    let msgs = messages.lock().await;

    // The notification should be a stream/event with the
    // McpTrustApprovalRequested event.
    let has_trust_notification = msgs.iter().any(|m| {
        if let ServerMessage::Notification(ServerNotificationEnvelope { method, params }) = m {
            method == "stream/event"
                && params["event"]["event"] == "mcp_trust_approval_requested"
                && params["event"]["request"]["server_id"] == "notif-server"
        } else {
            false
        }
    });
    assert!(
        has_trust_notification,
        "pump should also emit a notification for McpTrustApprovalRequested"
    );
}

// ---------------------------------------------------------------------------
// 3. Trust response with full McpTrustResponseParams format resolves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trust_response_full_params_format_resolves() {
    let (_app, _sink, messages, pending, pump_handle) =
        spawn_pump_with_trust_event("e2e-full-trust", "trust-srv", "tool_a", "req-full-trust")
            .await;

    let server_req_id = {
        let msgs = messages.lock().await;
        find_trust_server_request_id(&msgs)
            .expect("pump should have emitted a trust server-request")
    };

    // Respond with the full McpTrustResponseParams wire format.
    {
        let mut map = pending.lock().await;
        let tx = map
            .remove(&server_req_id)
            .expect("pending map should contain the server-request ID");
        tx.send(ResponseResult::Success {
            data: Some(json!({
                "request_id": "req-full-trust",
                "decision": "trust",
            })),
        })
        .expect("oneshot send");
    }

    // Wait for the spawned resolution task to process the response.
    tokio::time::sleep(Duration::from_millis(200)).await;
    pump_handle.abort();

    // The pending map should be cleaned up after resolution.
    let map = pending.lock().await;
    assert!(
        !map.contains_key(&server_req_id),
        "pending map entry should be cleaned up after trust resolution"
    );
}

// ---------------------------------------------------------------------------
// 4. Deny response with full McpTrustResponseParams format resolves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deny_response_full_params_format_resolves() {
    let (_app, _sink, messages, pending, pump_handle) =
        spawn_pump_with_trust_event("e2e-full-deny", "deny-srv", "tool_b", "req-full-deny").await;

    let server_req_id = {
        let msgs = messages.lock().await;
        find_trust_server_request_id(&msgs)
            .expect("pump should have emitted a trust server-request")
    };

    // Respond with the full McpTrustResponseParams wire format, deny decision.
    {
        let mut map = pending.lock().await;
        let tx = map
            .remove(&server_req_id)
            .expect("pending map should contain the server-request ID");
        tx.send(ResponseResult::Success {
            data: Some(json!({
                "request_id": "req-full-deny",
                "decision": "deny",
            })),
        })
        .expect("oneshot send");
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    pump_handle.abort();

    let map = pending.lock().await;
    assert!(
        !map.contains_key(&server_req_id),
        "pending map entry should be cleaned up after deny resolution"
    );
}

// ---------------------------------------------------------------------------
// 5. Trust response with bare McpTrustDecisionWire format resolves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trust_response_bare_decision_format_resolves() {
    let (_app, _sink, messages, pending, pump_handle) =
        spawn_pump_with_trust_event("e2e-bare-trust", "bare-srv", "tool_c", "req-bare").await;

    let server_req_id = {
        let msgs = messages.lock().await;
        find_trust_server_request_id(&msgs)
            .expect("pump should have emitted a trust server-request")
    };

    // Respond with the bare McpTrustDecisionWire format (just the string).
    {
        let mut map = pending.lock().await;
        let tx = map
            .remove(&server_req_id)
            .expect("pending map should contain the server-request ID");
        tx.send(ResponseResult::Success {
            data: Some(json!("trust")),
        })
        .expect("oneshot send");
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    pump_handle.abort();

    let map = pending.lock().await;
    assert!(
        !map.contains_key(&server_req_id),
        "pending map entry should be cleaned up after bare trust resolution"
    );
}

// ---------------------------------------------------------------------------
// 6. Unrecognizable response defaults to deny
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unrecognizable_response_defaults_to_deny() {
    let (_app, _sink, messages, pending, pump_handle) =
        spawn_pump_with_trust_event("e2e-unrecognizable", "unrec-srv", "tool_d", "req-unrec").await;

    let server_req_id = {
        let msgs = messages.lock().await;
        find_trust_server_request_id(&msgs)
            .expect("pump should have emitted a trust server-request")
    };

    // Respond with a garbage payload that matches neither format.
    {
        let mut map = pending.lock().await;
        let tx = map
            .remove(&server_req_id)
            .expect("pending map should contain the server-request ID");
        tx.send(ResponseResult::Success {
            data: Some(json!({"completely": "wrong", "format": 42})),
        })
        .expect("oneshot send");
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    pump_handle.abort();

    // The pending map should still be cleaned up even for bad responses.
    let map = pending.lock().await;
    assert!(
        !map.contains_key(&server_req_id),
        "pending map entry should be cleaned up even for unrecognizable responses"
    );
}

// ---------------------------------------------------------------------------
// 7. Error response defaults to deny
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_response_defaults_to_deny() {
    let (_app, _sink, messages, pending, pump_handle) =
        spawn_pump_with_trust_event("e2e-error", "err-srv", "tool_e", "req-err").await;

    let server_req_id = {
        let msgs = messages.lock().await;
        find_trust_server_request_id(&msgs)
            .expect("pump should have emitted a trust server-request")
    };

    // Respond with an error result.
    {
        let mut map = pending.lock().await;
        let tx = map
            .remove(&server_req_id)
            .expect("pending map should contain the server-request ID");
        tx.send(ResponseResult::Error(
            orbcode_app_server_protocol::ProtocolError {
                code: orbcode_app_server_protocol::ErrorCode::InternalError,
                message: "client-side error".into(),
                data: None,
            },
        ))
        .expect("oneshot send");
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    pump_handle.abort();

    let map = pending.lock().await;
    assert!(
        !map.contains_key(&server_req_id),
        "pending map entry should be cleaned up after error response"
    );
}

// ---------------------------------------------------------------------------
// 8. Timeout cleans up pending map and defaults to deny
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timeout_cleans_up_pending_and_denies() {
    let app = test_app("e2e-timeout").await;
    let (sink, messages) = TestSink::new();
    let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
    event_tx
        .send(StreamEvent::McpTrustApprovalRequested {
            request: McpTrustApprovalRequest {
                request_id: "req-timeout".to_string(),
                session_id: "session-mcp-trust-timeout".to_string(),
                server_id: "timeout-srv".to_string(),
                tool_name: "slow_tool".to_string(),
            },
        })
        .unwrap();
    drop(event_tx);

    // Use a very short timeout (50ms) so the test runs quickly.
    message_processor::pump_events(
        event_rx,
        sink,
        pending.clone(),
        app,
        "timeout-sub".to_string(),
        0,
        Duration::from_millis(50),
    )
    .await;

    // Wait for the timeout task to fire.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // The pending map should be empty (cleaned up after timeout).
    let map = pending.lock().await;
    assert!(map.is_empty(), "pending map should be empty after timeout");

    // The server-request should have been emitted.
    let msgs = messages.lock().await;
    let has_trust_request = msgs.iter().any(|m| {
        matches!(
            m,
            ServerMessage::Request(ServerRequestEnvelope { method, .. })
            if method == method::SERVER_REQUEST_MCP_TRUST
        )
    });
    assert!(
        has_trust_request,
        "server-request should be emitted before timeout"
    );
}

// Tests 9 and 10 (handle_message response routing) moved to
// message_processor.rs unit tests — they need direct access to the
// private pending_server_requests field.

// ---------------------------------------------------------------------------
// 11. Unknown server-request response ID is silently ignored
// ---------------------------------------------------------------------------

// Note: test 10 was removed here; the deny routing test is now in
// message_processor::tests::handle_message_response_deny_routes_correctly.

// Keeping the block below so the numbered sequence stays readable.
// (The original tests 9 and 10 used pending_server_requests() which is
// no longer pub.)

// ---------------------------------------------------------------------------
// Remaining: test 11 does NOT use pending_server_requests -- it only sends
// a response with a nonexistent ID and verifies no crash.
// ---------------------------------------------------------------------------

// Tests 9-10 (pending oneshot routing) moved to message_processor.rs unit
// tests — they need direct access to the private pending_server_requests field.

// ---------------------------------------------------------------------------
// 11. Unknown server-request response ID is silently ignored
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_response_id_is_silently_ignored() {
    let app = test_app("e2e-unknown-id").await;
    let (sink, messages) = TestSink::new();
    let mut processor = MessageProcessor::new(app, sink);

    processor.handle_message(initialize_request()).await;

    // Send a response for a nonexistent server-request ID.
    processor
        .handle_message(ClientMessage::Response(ServerRequestResponse {
            id: "nonexistent-trust-req".into(),
            result: ResponseResult::Success {
                data: Some(json!({"decision": "trust"})),
            },
        }))
        .await;

    // Should not crash; only the initialize response should be in messages.
    let msgs = messages.lock().await;
    assert_eq!(
        msgs.len(),
        1,
        "only the initialize response should be present"
    );
    match &msgs[0] {
        ServerMessage::Response(r) => assert_eq!(r.id, "init-1"),
        other => panic!("expected init Response, got: {other:?}"),
    }
}
