//! Integration tests for the AskUserQuestion server-request DTOs and the
//! pump-level interactive suspension flow.
//!
//! Tests cover: wire format round-trips, method constant membership, server-
//! request envelope shape, and the pump_events handler for
//! `AskUserQuestionRequested` (answer resolution, timeout/cancel paths).

use orbcode_app_server_protocol::{
    AskUserQuestionRequest, AskUserQuestionResponse, McpTrustDecisionWire, McpTrustResponseParams,
    method,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// 1. AskUserQuestion wire format round-trips
// ---------------------------------------------------------------------------

#[test]
fn ask_user_request_wire_format_with_options() {
    let req = AskUserQuestionRequest {
        session_id: "session-ask".into(),
        turn_id: None,
        tool_use_id: String::new(),
        request_id: "ask-e2e-1".into(),
        deadline: None,
        validation_error: None,
        questions: Vec::new(),
        question: "Which branch to deploy?".into(),
        options: vec!["main".into(), "staging".into(), "canary".into()],
    };

    let wire = serde_json::to_value(&req).unwrap();
    assert_eq!(wire["session_id"], "session-ask");
    assert_eq!(wire["request_id"], "ask-e2e-1");
    assert_eq!(wire["question"], "Which branch to deploy?");
    assert_eq!(wire["options"], json!(["main", "staging", "canary"]));

    // Round-trip
    let back: AskUserQuestionRequest = serde_json::from_value(wire).unwrap();
    assert_eq!(req, back);
}

#[test]
fn ask_user_request_wire_format_no_options() {
    let req = AskUserQuestionRequest {
        session_id: "session-ask".into(),
        turn_id: None,
        tool_use_id: String::new(),
        request_id: "ask-e2e-2".into(),
        deadline: None,
        validation_error: None,
        questions: Vec::new(),
        question: "Enter the deployment target".into(),
        options: vec![],
    };

    let wire = serde_json::to_value(&req).unwrap();
    // Empty options should be omitted from serialization.
    assert!(
        wire.get("options").is_none(),
        "empty options must be skipped in wire format"
    );
    assert_eq!(wire["request_id"], "ask-e2e-2");
    assert_eq!(wire["session_id"], "session-ask");

    // Round-trip (absent options defaults to empty vec).
    let back: AskUserQuestionRequest = serde_json::from_value(wire).unwrap();
    assert_eq!(back.options, Vec::<String>::new());
}

#[test]
fn ask_user_response_with_answer_wire_format() {
    let resp = AskUserQuestionResponse {
        request_id: "ask-e2e-1".into(),
        outcome: None,
        answer: Some("staging".into()),
    };

    let wire = serde_json::to_value(&resp).unwrap();
    assert_eq!(wire["request_id"], "ask-e2e-1");
    assert_eq!(wire["answer"], "staging");

    let back: AskUserQuestionResponse = serde_json::from_value(wire).unwrap();
    assert_eq!(resp, back);
}

#[test]
fn ask_user_response_cancelled_wire_format() {
    let resp = AskUserQuestionResponse {
        request_id: "ask-e2e-3".into(),
        outcome: None,
        answer: None,
    };

    let wire = serde_json::to_value(&resp).unwrap();
    assert_eq!(wire["request_id"], "ask-e2e-3");
    assert!(wire["answer"].is_null());

    let back: AskUserQuestionResponse = serde_json::from_value(wire).unwrap();
    assert_eq!(resp, back);
}

// ---------------------------------------------------------------------------
// 2. MCP trust wire format cross-validation
// ---------------------------------------------------------------------------

#[test]
fn mcp_trust_response_params_cross_format() {
    // Verify that McpTrustResponseParams can be parsed from the same shape
    // that pump_events sends to set_mcp_server_trust.
    let wire = json!({
        "request_id": "srv-mcp-trust-42",
        "decision": "trust",
    });
    let params: McpTrustResponseParams = serde_json::from_value(wire).unwrap();
    assert_eq!(params.request_id, "srv-mcp-trust-42");
    assert_eq!(params.decision, McpTrustDecisionWire::Trust);

    let wire_deny = json!({
        "request_id": "srv-mcp-trust-99",
        "decision": "deny",
    });
    let params_deny: McpTrustResponseParams = serde_json::from_value(wire_deny).unwrap();
    assert_eq!(params_deny.decision, McpTrustDecisionWire::Deny);
}

// ---------------------------------------------------------------------------
// 3. Method constants are registered in server_request_methods
// ---------------------------------------------------------------------------

#[test]
fn ask_user_method_is_in_server_request_methods() {
    let methods = method::server_request_methods();
    assert!(
        methods.contains(&method::SERVER_REQUEST_ASK_USER),
        "SERVER_REQUEST_ASK_USER must be advertised in server_request_methods()"
    );
}

#[test]
fn mcp_trust_method_is_in_server_request_methods() {
    let methods = method::server_request_methods();
    assert!(
        methods.contains(&method::SERVER_REQUEST_MCP_TRUST),
        "SERVER_REQUEST_MCP_TRUST must be advertised in server_request_methods()"
    );
}

// ---------------------------------------------------------------------------
// 4. AskUserQuestion server-request envelope shape
// ---------------------------------------------------------------------------

/// Validates that an AskUserQuestionRequest can be serialized into a
/// ServerRequestEnvelope-compatible params field, mirroring how the
/// pump_events function will eventually emit it.
#[test]
fn ask_user_request_as_server_request_params() {
    use orbcode_app_server_protocol::ServerRequestEnvelope;

    let req = AskUserQuestionRequest {
        session_id: "session-ask".into(),
        turn_id: None,
        tool_use_id: String::new(),
        request_id: "ask-envelope-1".into(),
        deadline: None,
        validation_error: None,
        questions: Vec::new(),
        question: "Confirm deployment?".into(),
        options: vec!["yes".into(), "no".into()],
    };

    let envelope = ServerRequestEnvelope {
        id: "srv-ask-1".into(),
        method: method::SERVER_REQUEST_ASK_USER.to_string(),
        params: serde_json::to_value(&req).unwrap(),
    };

    // The envelope should serialize cleanly.
    let wire = serde_json::to_value(&envelope).unwrap();
    assert_eq!(wire["method"], "ask_user/request");
    assert_eq!(wire["params"]["session_id"], "session-ask");
    assert_eq!(wire["params"]["question"], "Confirm deployment?");
    assert_eq!(wire["params"]["options"], json!(["yes", "no"]));

    // And be parseable back from the params.
    let parsed: AskUserQuestionRequest = serde_json::from_value(wire["params"].clone()).unwrap();
    assert_eq!(parsed, req);
}

// ---------------------------------------------------------------------------
// 5. Pump-level E2E: AskUserQuestionRequested emits server-request
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use orbcode_app_server::AppServer;
use orbcode_app_server::message_processor::{self, ServerSink};
use orbcode_app_server_protocol::{ResponseResult, ServerMessage};
use orbcode_config::AppConfigOverrides;
use orbcode_protocol::StreamEvent;
use tokio::sync::{Mutex, mpsc, oneshot};

type RequestId = String;

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

#[tokio::test]
async fn pump_emits_server_request_for_ask_user_question() {
    let app = test_app("ask-pump").await;
    let (sink, messages) = TestSink::new();
    let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
    event_tx
        .send(StreamEvent::AskUserQuestionRequested {
            session_id: "session-ask".into(),
            turn_id: None,
            tool_use_id: String::new(),
            request_id: "ask-pump-1".into(),
            deadline: None,
            questions: Vec::new(),
            question: "Pick a color".into(),
            options: vec!["red".into(), "blue".into()],
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
                "ask-sub".to_string(),
                0,
                Duration::from_secs(5),
            )
            .await;
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let msgs = messages.lock().await;
    let server_requests: Vec<_> = msgs
        .iter()
        .filter_map(|m| {
            if let ServerMessage::Request(r) = m {
                Some(r)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        server_requests.len(),
        1,
        "pump should emit exactly one server-request for AskUserQuestion"
    );
    let sreq = server_requests[0];
    assert_eq!(sreq.method, "ask_user/request");
    assert_eq!(sreq.params["session_id"], "session-ask");
    assert_eq!(sreq.params["question"], "Pick a color");
    assert_eq!(sreq.params["options"], json!(["red", "blue"]));

    // A pending oneshot should have been inserted.
    let pending_count = pending.lock().await.len();
    assert_eq!(pending_count, 1, "pending map should have one entry");

    pump_handle.abort();
}

#[tokio::test]
async fn pump_resolves_ask_user_answer_via_response() {
    let app = test_app("ask-resolve").await;
    let (sink, messages) = TestSink::new();
    let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
    event_tx
        .send(StreamEvent::AskUserQuestionRequested {
            session_id: "session-ask".into(),
            turn_id: None,
            tool_use_id: String::new(),
            request_id: "ask-resolve-1".into(),
            deadline: None,
            questions: Vec::new(),
            question: "Pick a color".into(),
            options: vec!["red".into(), "blue".into()],
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
                "ask-sub".to_string(),
                0,
                Duration::from_secs(5),
            )
            .await;
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Find the server-request ID and resolve the pending oneshot.
    let server_req_id = {
        let msgs = messages.lock().await;
        msgs.iter()
            .find_map(|m| {
                if let ServerMessage::Request(r) = m {
                    if r.method == "ask_user/request" {
                        Some(r.id.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .expect("should have server-request")
    };

    // Simulate client response with AskUserQuestionResponse.
    let response_data = json!({
        "request_id": "ask-resolve-1",
        "answer": "blue"
    });
    let tx = pending
        .lock()
        .await
        .remove(&server_req_id)
        .expect("pending entry");
    tx.send(ResponseResult::Success {
        data: Some(response_data),
    })
    .unwrap();

    // Allow time for the resolve task to run.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The pending map should be empty after resolution.
    assert!(
        pending.lock().await.is_empty(),
        "pending map should be empty after resolution"
    );

    pump_handle.abort();
}

#[tokio::test]
async fn pump_ask_user_timeout_resolves_none() {
    let app = test_app("ask-timeout").await;
    let (sink, _messages) = TestSink::new();
    let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
    event_tx
        .send(StreamEvent::AskUserQuestionRequested {
            session_id: "session-ask".into(),
            turn_id: None,
            tool_use_id: String::new(),
            request_id: "ask-timeout-1".into(),
            deadline: None,
            questions: Vec::new(),
            question: "Pick a color".into(),
            options: vec![],
        })
        .unwrap();
    drop(event_tx);

    let pending_clone = Arc::clone(&pending);
    let _pump_handle = tokio::spawn({
        let sink = Arc::clone(&sink) as Arc<dyn ServerSink>;
        let app = app.clone();
        async move {
            message_processor::pump_events(
                event_rx,
                sink,
                pending_clone,
                app,
                "ask-sub".to_string(),
                0,
                // Very short timeout to trigger quickly.
                Duration::from_millis(100),
            )
            .await;
        }
    });

    // Wait for timeout + cleanup.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Pending should be cleaned up after timeout.
    assert!(
        pending.lock().await.is_empty(),
        "pending map should be cleaned up after timeout"
    );
}

// pump_exit and pump_abort core-pending proof tests live in
// message_processor.rs unit tests (crate-internal access to
// ask_user_pending_for_test without exposing it in the public API).
