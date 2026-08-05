//! Contract fixture tests — deserialize golden JSON files and verify round-trip.
//!
//! These fixtures are the protocol contract artifact for external clients.
//! If a fixture fails to deserialize or round-trip, the wire shape has drifted
//! and the fixture must be updated intentionally (not silently).

use orbcode_app_server_protocol::{
    AskUserQuestionRequest, AskUserQuestionResponse, BootstrapParams, ClientMessage, ErrorCode,
    InitializeParams, InitializeResult, McpListServersResult, McpServerInput, McpTrustDecisionWire,
    McpTrustResponseParams, PermissionDecisionWire, PermissionResponseParams, ServerMessage,
    method,
};
use serde_json::Value;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn load_fixture(name: &str) -> Value {
    let path = format!("{FIXTURES_DIR}/{name}");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {path}: {e}"));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("failed to parse fixture {path}: {e}"))
}

fn assert_roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(fixture: &str) -> T {
    let value = load_fixture(fixture);
    let parsed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("fixture {fixture} failed to deserialize: {e}"));
    let reserialized = serde_json::to_value(&parsed)
        .unwrap_or_else(|e| panic!("fixture {fixture} failed to reserialize: {e}"));
    assert_eq!(
        value, reserialized,
        "fixture {fixture} round-trip mismatch — wire shape has drifted"
    );
    parsed
}

// ---------------------------------------------------------------------------
// Envelope types (ClientMessage / ServerMessage)
// ---------------------------------------------------------------------------

#[test]
fn contract_client_request() {
    let msg: ClientMessage = assert_roundtrip("client_request.json");
    assert!(matches!(msg, ClientMessage::Request(ref r) if r.method == "session/list"));
}

#[test]
fn contract_client_request_with_params() {
    let msg: ClientMessage = assert_roundtrip("client_request_with_params.json");
    assert!(matches!(msg, ClientMessage::Request(ref r) if r.method == "turn/submit"));
}

#[test]
fn contract_client_response() {
    let msg: ClientMessage = assert_roundtrip("client_response.json");
    assert!(matches!(msg, ClientMessage::Response(_)));
}

#[test]
fn contract_server_response_success() {
    let msg: ServerMessage = assert_roundtrip("server_response_success.json");
    assert!(matches!(msg, ServerMessage::Response(_)));
}

#[test]
fn contract_server_response_error() {
    let msg: ServerMessage = assert_roundtrip("server_response_error.json");
    if let ServerMessage::Response(env) = msg {
        assert!(
            matches!(env.result, orbcode_app_server_protocol::ResponseResult::Error(ref e) if e.code == ErrorCode::MethodNotFound)
        );
    } else {
        panic!("expected response");
    }
}

#[test]
fn contract_server_notification() {
    let msg: ServerMessage = assert_roundtrip("server_notification.json");
    assert!(matches!(msg, ServerMessage::Notification(ref n) if n.method == "stream/event"));
}

#[test]
fn contract_server_request_permission() {
    let msg: ServerMessage = assert_roundtrip("server_request_permission.json");
    assert!(matches!(msg, ServerMessage::Request(ref r) if r.method == "permission/request"));
}

#[test]
fn contract_server_request_mcp_trust() {
    let msg: ServerMessage = assert_roundtrip("server_request_mcp_trust.json");
    assert!(matches!(msg, ServerMessage::Request(ref r) if r.method == "mcp_trust/request"));
}

#[test]
fn contract_server_request_ask_user() {
    let msg: ServerMessage = assert_roundtrip("server_request_ask_user.json");
    assert!(matches!(msg, ServerMessage::Request(ref r) if r.method == "ask_user/request"));
}

#[test]
fn contract_canonical_ask_user_request() {
    let request: AskUserQuestionRequest =
        assert_roundtrip("ask_user_question_request_canonical.json");
    assert_eq!(request.canonical_questions().unwrap().len(), 2);
}

#[test]
fn contract_canonical_ask_user_response() {
    let response: AskUserQuestionResponse =
        assert_roundtrip("ask_user_question_response_canonical.json");
    assert!(response.outcome.is_some());
    assert!(response.answer.is_none());
}

// ---------------------------------------------------------------------------
// Initialize handshake DTOs
// ---------------------------------------------------------------------------

#[test]
fn contract_initialize_params() {
    let params: InitializeParams = assert_roundtrip("initialize_params.json");
    assert_eq!(params.protocol_version, "1.0");
    assert_eq!(params.client_info.name, "test-client");
    assert!(params.capabilities.streaming);
    assert!(!params.capabilities.experimental_methods);
}

#[test]
fn contract_initialize_result() {
    let result: InitializeResult = assert_roundtrip("initialize_result.json");
    assert_eq!(result.server_info.name, "orbcode");
    assert!(result.capabilities.streaming);
    assert!(!result.capabilities.stable_methods.is_empty());
    assert!(!result.capabilities.experimental_methods.is_empty());
    assert!(!result.capabilities.server_notification_methods.is_empty());
    assert!(!result.capabilities.server_request_methods.is_empty());
}

// ---------------------------------------------------------------------------
// Request parameter DTOs
// ---------------------------------------------------------------------------

#[test]
fn contract_bootstrap_params() {
    let params: BootstrapParams = assert_roundtrip("bootstrap_params.json");
    assert_eq!(params.session_id.as_deref(), Some("session-123"));
    assert_eq!(
        params.cwd.as_deref(),
        Some(std::path::Path::new("/tmp/project"))
    );
    assert_eq!(params.additional_directories.len(), 1);
    assert_eq!(params.session_mcp_servers.len(), 1);
    assert_eq!(params.session_mcp_servers[0].id, "docs-server");
}

#[test]
fn contract_mcp_server_input() {
    let input: McpServerInput = assert_roundtrip("mcp_server_input.json");
    assert_eq!(input.id, "docs-server");
    assert_eq!(input.env["DOCS_TOKEN"], "secret");
}

#[test]
fn contract_mcp_list_servers_result() {
    let result: McpListServersResult = assert_roundtrip("mcp_list_servers_result.json");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "docs-server");
    assert_eq!(result[0].env["DOCS_TOKEN"], "<redacted>");
}

// ---------------------------------------------------------------------------
// Permission DTOs
// ---------------------------------------------------------------------------

#[test]
fn contract_permission_decision_approve() {
    let d: PermissionDecisionWire = assert_roundtrip("permission_decision_approve.json");
    assert!(matches!(d, PermissionDecisionWire::Approve));
}

#[test]
fn contract_permission_decision_deny() {
    let d: PermissionDecisionWire = assert_roundtrip("permission_decision_deny.json");
    assert!(matches!(d, PermissionDecisionWire::Deny));
}

#[test]
fn contract_permission_decision_approve_always() {
    let d: PermissionDecisionWire = assert_roundtrip("permission_decision_approve_always.json");
    assert!(matches!(d, PermissionDecisionWire::ApproveAlways { .. }));
}

#[test]
fn contract_permission_response_params() {
    let p: PermissionResponseParams = assert_roundtrip("permission_response_params.json");
    assert_eq!(p.request_id, "perm-1");
    assert!(matches!(p.decision, PermissionDecisionWire::Approve));
}

// ---------------------------------------------------------------------------
// MCP trust DTOs
// ---------------------------------------------------------------------------

#[test]
fn contract_mcp_trust_decision_trust() {
    let d: McpTrustDecisionWire = assert_roundtrip("mcp_trust_decision_trust.json");
    assert!(matches!(d, McpTrustDecisionWire::Trust));
}

#[test]
fn contract_mcp_trust_decision_deny() {
    let d: McpTrustDecisionWire = assert_roundtrip("mcp_trust_decision_deny.json");
    assert!(matches!(d, McpTrustDecisionWire::Deny));
}

#[test]
fn contract_mcp_trust_response_params() {
    let p: McpTrustResponseParams = assert_roundtrip("mcp_trust_response_params.json");
    assert_eq!(p.request_id, "trust-1");
    assert!(matches!(p.decision, McpTrustDecisionWire::Trust));
}

// ---------------------------------------------------------------------------
// AskUserQuestion DTOs
// ---------------------------------------------------------------------------

#[test]
fn contract_ask_user_question_request() {
    let r: AskUserQuestionRequest = assert_roundtrip("ask_user_question_request.json");
    assert_eq!(r.session_id, "session-123");
    assert_eq!(r.question, "Which database?");
    assert_eq!(r.options, vec!["PostgreSQL", "MySQL"]);
}

#[test]
fn contract_ask_user_question_response() {
    let r: AskUserQuestionResponse = assert_roundtrip("ask_user_question_response.json");
    assert_eq!(r.answer.as_deref(), Some("PostgreSQL"));
}

// ---------------------------------------------------------------------------
// Error codes — pinned list prevents silent additions/removals
// ---------------------------------------------------------------------------

#[test]
fn contract_error_codes_pinned() {
    let value = load_fixture("error_codes.json");
    let codes: Vec<String> = serde_json::from_value(value).unwrap();
    for code_str in &codes {
        let _: ErrorCode = serde_json::from_value(Value::String(code_str.clone()))
            .unwrap_or_else(|_| panic!("error code '{code_str}' no longer deserializes"));
    }
    assert_eq!(
        codes.len(),
        14,
        "error code count changed — update the fixture if intentional"
    );
}

// ---------------------------------------------------------------------------
// Method constants — pinned list prevents silent additions/removals
// ---------------------------------------------------------------------------

#[test]
fn contract_method_constants_pinned() {
    let value = load_fixture("method_constants.json");
    let stable_fixture: Vec<String> =
        serde_json::from_value(value["stable_client_request_methods"].clone()).unwrap();
    let experimental_fixture: Vec<String> =
        serde_json::from_value(value["experimental_client_request_methods"].clone()).unwrap();
    let notification_fixture: Vec<String> =
        serde_json::from_value(value["server_notification_methods"].clone()).unwrap();
    let server_request_fixture: Vec<String> =
        serde_json::from_value(value["server_request_methods"].clone()).unwrap();

    let stable_actual: Vec<String> = method::stable_client_request_methods()
        .into_iter()
        .map(String::from)
        .collect();
    let experimental_actual: Vec<String> = method::experimental_client_request_methods()
        .into_iter()
        .map(String::from)
        .collect();
    let notification_actual: Vec<String> = method::server_notification_methods()
        .into_iter()
        .map(String::from)
        .collect();
    let server_request_actual: Vec<String> = method::server_request_methods()
        .into_iter()
        .map(String::from)
        .collect();

    assert_eq!(
        stable_fixture, stable_actual,
        "stable method list drifted — update fixture if intentional"
    );
    assert_eq!(
        experimental_fixture, experimental_actual,
        "experimental method list drifted — update fixture if intentional"
    );
    assert_eq!(
        notification_fixture, notification_actual,
        "notification method list drifted — update fixture if intentional"
    );
    assert_eq!(
        server_request_fixture, server_request_actual,
        "server request method list drifted — update fixture if intentional"
    );
}
