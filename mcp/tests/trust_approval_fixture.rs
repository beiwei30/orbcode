//! Real MCP trust approval round-trip fixture tests.
//!
//! These tests verify the trust approval flow end-to-end at the `McpRegistry`
//! layer: a fake stdio MCP server is started, registered with `Unknown` trust,
//! and a `TrustApprovalHandler` is installed so that `invoke_tool` can exercise
//! the full trust-request / trust-response round-trip without going through the
//! app-server or TUI layers.
//!
//! Scenarios covered:
//! 1. Handler responds `Trusted` -- tool call proceeds and succeeds.
//! 2. Handler responds `Denied`  -- tool call is rejected with `ServerUntrusted`.
//! 3. No handler installed       -- tool call is rejected with `ServerUntrusted`.
//! 4. Handler returns `None`     -- tool call is rejected with `ServerUntrusted`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use orbcode_mcp::{
    McpError, McpRegistry, McpServerTrust, SharedTrustApprovalHandler, TrustApprovalHandler,
    TrustApprovalRequest, TrustApprovalResponse,
};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Trust approval handler fixtures
// ---------------------------------------------------------------------------

/// A handler that always responds with a fixed decision and records how many
/// times it was called.
struct FixedTrustHandler {
    decision: TrustApprovalResponse,
    call_count: AtomicUsize,
}

impl FixedTrustHandler {
    fn new(decision: TrustApprovalResponse) -> Arc<Self> {
        Arc::new(Self {
            decision,
            call_count: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.call_count.load(Ordering::Relaxed)
    }
}

impl TrustApprovalHandler for FixedTrustHandler {
    async fn request_trust_approval(
        &self,
        _request: TrustApprovalRequest,
    ) -> Option<TrustApprovalResponse> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        Some(self.decision)
    }
}

/// A handler that always returns `None` (no decision available).
struct NullTrustHandler;

impl TrustApprovalHandler for NullTrustHandler {
    async fn request_trust_approval(
        &self,
        _request: TrustApprovalRequest,
    ) -> Option<TrustApprovalResponse> {
        None
    }
}

/// A handler that records the request details for assertions.
struct RecordingTrustHandler {
    decision: TrustApprovalResponse,
    requests: tokio::sync::Mutex<Vec<TrustApprovalRequest>>,
}

impl RecordingTrustHandler {
    fn new(decision: TrustApprovalResponse) -> Arc<Self> {
        Arc::new(Self {
            decision,
            requests: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    async fn recorded_requests(&self) -> Vec<TrustApprovalRequest> {
        self.requests.lock().await.clone()
    }
}

impl TrustApprovalHandler for RecordingTrustHandler {
    async fn request_trust_approval(
        &self,
        request: TrustApprovalRequest,
    ) -> Option<TrustApprovalResponse> {
        self.requests.lock().await.push(request);
        Some(self.decision)
    }
}

// ---------------------------------------------------------------------------
// Test helpers (shared with stdio_fake_server.rs via duplication -- each
// integration test file is a separate compilation unit in Rust)
// ---------------------------------------------------------------------------

fn temp_paths(label: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("orbcode-mcp-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let cwd = root.join("cwd");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    (home, cwd)
}

fn write_mcp_json(cwd: &Path, value: Value) {
    std::fs::write(
        cwd.join(".mcp.json"),
        serde_json::to_string_pretty(&value).expect("serialize mcp json"),
    )
    .expect("write .mcp.json");
}

async fn trust_registry(label: &str) -> McpRegistry {
    let (home, cwd) = temp_paths(label);
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "args": [],
                }
            }
        }),
    );
    // The server starts with Unknown trust (the default from .mcp.json loading).
    McpRegistry::load(&home, &cwd).await.expect("load registry")
}

// ---------------------------------------------------------------------------
// Fake stdio server binary (compiled once per process)
// ---------------------------------------------------------------------------

fn fake_stdio_server_binary() -> &'static Path {
    static SERVER_BINARY: OnceLock<PathBuf> = OnceLock::new();
    SERVER_BINARY
        .get_or_init(compile_fake_stdio_server)
        .as_path()
}

fn compile_fake_stdio_server() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orbcode-mcp-fake-stdio-trust-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create fake MCP server temp dir");

    let source = dir.join("fake_stdio_server.rs");
    let binary = dir.join(if cfg!(windows) {
        "fake_stdio_server.exe"
    } else {
        "fake_stdio_server"
    });
    std::fs::write(&source, FAKE_STDIO_SERVER_SOURCE).expect("write fake MCP server source");

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(rustc)
        .arg("--edition=2021")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("compile fake MCP stdio server");

    assert!(
        output.status.success(),
        "compile fake MCP stdio server\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    binary
}

// Minimal fake stdio server -- same as in stdio_fake_server.rs.
const FAKE_STDIO_SERVER_SOURCE: &str = r##"
use std::io::{self, BufRead, Write};
use std::time::Duration;

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() { continue; }
        let response = fake_stdio_response(&line);
        writeln!(stdout, "{response}").expect("write");
        stdout.flush().expect("flush");
    }
}

fn fake_stdio_response(request: &str) -> String {
    let id = extract_id(request);

    if request.contains(r#""method":"initialize""#) {
        return success_response(
            &id,
            r#"{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"orbcode-fake-trust","version":"0.1.0"}}"#,
        );
    }

    if request.contains(r#""method":"tools/list""#) {
        return success_response(
            &id,
            r#"{"tools":[{"name":"echo","description":"Echo test input.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}"#,
        );
    }

    if request.contains(r#""method":"tools/call""#) && request.contains(r#""name":"echo""#) {
        let text = extract_text_argument(request);
        let echoed = escape_json_string(&format!("echo: {text}"));
        return success_response(
            &id,
            &format!(r#"{{"content":[{{"type":"text","text":"{echoed}"}}],"isError":false}}"#),
        );
    }

    if request.contains(r#""method":"tools/call""#) {
        return error_response(&id, -32602, "unknown fake tool");
    }

    error_response(&id, -32601, "unknown fake method")
}

fn extract_id(request: &str) -> String {
    let Some(index) = request.find(r#""id":"#) else { return "null".to_string() };
    let rest = request[index + r#""id":"#.len()..].trim_start();
    if rest.starts_with("null") { return "null".to_string(); }
    if rest.starts_with('"') {
        return read_json_string_literal(rest).unwrap_or_else(|| "null".to_string());
    }
    let id: String = rest.chars().take_while(|ch| ch.is_ascii_digit() || *ch == '-').collect();
    if id.is_empty() { "null".to_string() } else { id }
}

fn extract_text_argument(request: &str) -> String {
    let pattern = "\"text\":\"";
    let Some(index) = request.find(pattern) else { return String::new() };
    let rest = &request[index + pattern.len()..];
    read_json_string_value(rest).unwrap_or_default()
}

fn read_json_string_literal(input: &str) -> Option<String> {
    let mut escaped = false;
    for (index, ch) in input.char_indices().skip(1) {
        if escaped { escaped = false; continue; }
        if ch == '\\' { escaped = true; continue; }
        if ch == '"' { return Some(input[..=index].to_string()); }
    }
    None
}

fn read_json_string_value(input: &str) -> Option<String> {
    let mut value = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                '/' => value.push('/'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            },
            other => value.push(other),
        }
    }
    None
}

fn success_response(id: &str, result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#)
}

fn error_response(id: &str, code: i64, message: &str) -> String {
    let message = escape_json_string(message);
    format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":"{message}"}}}}"#)
}

fn escape_json_string(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '"' => output.push_str(r#"\""#),
            '\\' => output.push_str(r#"\\"#),
            '\n' => output.push_str(r#"\n"#),
            '\r' => output.push_str(r#"\r"#),
            '\t' => output.push_str(r#"\t"#),
            other => output.push(other),
        }
    }
    output
}
"##;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// When the handler approves with `Trusted`, the tool call should succeed and
/// the server's persisted trust should be upgraded to `Trusted`.
#[tokio::test]
async fn trust_approval_handler_trusted_allows_tool_call() {
    let registry = trust_registry("trust-approved").await;

    // Verify the server starts as Unknown.
    assert_eq!(
        registry.server_trust("fake").await,
        McpServerTrust::Unknown,
        "server should start with Unknown trust"
    );

    // Install a handler that always approves.
    let handler = FixedTrustHandler::new(TrustApprovalResponse::Trusted);
    let handler_ref = Arc::clone(&handler);
    registry
        .set_trust_approval_handler(handler_ref as SharedTrustApprovalHandler)
        .await;

    // Invoke the tool -- should succeed because the handler approves.
    let result = registry
        .invoke_tool("fake", "echo", r#"{"text":"trust-test"}"#)
        .await
        .expect("tool call should succeed after trust approval");

    assert_eq!(result.output, "echo: trust-test");
    assert!(!result.is_error);

    // The handler should have been called exactly once.
    assert_eq!(handler.calls(), 1, "handler should be called once");

    // Trust should now be persisted as Trusted.
    assert_eq!(
        registry.server_trust("fake").await,
        McpServerTrust::Trusted,
        "trust should be upgraded to Trusted after approval"
    );
}

/// When the handler denies, the tool call should fail with `ServerUntrusted`
/// and the server's trust should be set to `Denied`.
#[tokio::test]
async fn trust_approval_handler_denied_blocks_tool_call() {
    let registry = trust_registry("trust-denied").await;

    assert_eq!(registry.server_trust("fake").await, McpServerTrust::Unknown,);

    let handler = FixedTrustHandler::new(TrustApprovalResponse::Denied);
    let handler_ref = Arc::clone(&handler);
    registry
        .set_trust_approval_handler(handler_ref as SharedTrustApprovalHandler)
        .await;

    let error = registry
        .invoke_tool("fake", "echo", r#"{"text":"deny-test"}"#)
        .await
        .expect_err("tool call should fail after trust denial");

    assert!(
        matches!(error, McpError::ServerUntrusted { ref server, status: "denied" } if server == "fake"),
        "expected ServerUntrusted with denied status, got: {error}"
    );
    assert_eq!(handler.calls(), 1);

    assert_eq!(
        registry.server_trust("fake").await,
        McpServerTrust::Denied,
        "trust should be set to Denied after handler denies"
    );
}

/// When no handler is installed and trust is Unknown, the tool call should fail
/// with `ServerUntrusted`.
#[tokio::test]
async fn no_handler_installed_blocks_tool_call() {
    let registry = trust_registry("trust-no-handler").await;

    assert_eq!(registry.server_trust("fake").await, McpServerTrust::Unknown,);

    // No handler installed -- the default is None.

    let error = registry
        .invoke_tool("fake", "echo", r#"{"text":"no-handler"}"#)
        .await
        .expect_err("tool call should fail without trust handler");

    assert!(
        matches!(error, McpError::ServerUntrusted { ref server, .. } if server == "fake"),
        "expected ServerUntrusted, got: {error}"
    );

    // Trust should remain Unknown (not changed to Denied) since there was no
    // handler to produce a decision.
    assert_eq!(
        registry.server_trust("fake").await,
        McpServerTrust::Unknown,
        "trust should remain Unknown when no handler is installed"
    );
}

/// When the handler returns `None` (no decision), the tool call should fail
/// with `ServerUntrusted` and trust should remain Unknown.
#[tokio::test]
async fn handler_returns_none_blocks_tool_call() {
    let registry = trust_registry("trust-handler-none").await;

    registry
        .set_trust_approval_handler(Arc::new(NullTrustHandler) as SharedTrustApprovalHandler)
        .await;

    let error = registry
        .invoke_tool("fake", "echo", r#"{"text":"none"}"#)
        .await
        .expect_err("tool call should fail when handler returns None");

    assert!(
        matches!(error, McpError::ServerUntrusted { ref server, .. } if server == "fake"),
        "expected ServerUntrusted, got: {error}"
    );

    assert_eq!(
        registry.server_trust("fake").await,
        McpServerTrust::Unknown,
        "trust should remain Unknown when handler returns None"
    );
}

/// After trust is approved once, subsequent calls should not trigger the
/// handler again.
#[tokio::test]
async fn trust_approval_is_sticky_across_calls() {
    let registry = trust_registry("trust-sticky").await;

    let handler = FixedTrustHandler::new(TrustApprovalResponse::Trusted);
    let handler_ref = Arc::clone(&handler);
    registry
        .set_trust_approval_handler(handler_ref as SharedTrustApprovalHandler)
        .await;

    // First call -- handler gets invoked.
    let result1 = registry
        .invoke_tool("fake", "echo", r#"{"text":"first"}"#)
        .await
        .expect("first call should succeed");
    assert_eq!(result1.output, "echo: first");
    assert_eq!(handler.calls(), 1);

    // Second call -- server is now Trusted, handler should NOT be called again.
    let result2 = registry
        .invoke_tool("fake", "echo", r#"{"text":"second"}"#)
        .await
        .expect("second call should succeed without handler");
    assert_eq!(result2.output, "echo: second");
    assert_eq!(
        handler.calls(),
        1,
        "handler should not be called again after trust is persisted"
    );
}

/// The handler receives the correct server_id and tool_name in the request.
#[tokio::test]
async fn trust_approval_request_contains_correct_details() {
    let registry = trust_registry("trust-details").await;

    let handler = RecordingTrustHandler::new(TrustApprovalResponse::Trusted);
    let handler_ref = Arc::clone(&handler);
    registry
        .set_trust_approval_handler(handler_ref as SharedTrustApprovalHandler)
        .await;

    registry
        .invoke_tool("fake", "echo", r#"{"text":"details"}"#)
        .await
        .expect("tool call should succeed");

    let requests = handler.recorded_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].server_id, "fake");
    assert_eq!(requests[0].tool_name, "echo");
    assert!(
        !requests[0].request_id.is_empty(),
        "request_id should be non-empty"
    );
}

/// After denial, the server trust is set to Denied. Even if the handler would
/// now approve, the tool call is rejected because trust is already Denied (not
/// Unknown). The handler is NOT called for Denied servers.
#[tokio::test]
async fn denied_trust_is_not_re_prompted() {
    let registry = trust_registry("trust-denied-sticky").await;

    // First: deny the server.
    let deny_handler = FixedTrustHandler::new(TrustApprovalResponse::Denied);
    let deny_ref = Arc::clone(&deny_handler);
    registry
        .set_trust_approval_handler(deny_ref as SharedTrustApprovalHandler)
        .await;

    let _ = registry
        .invoke_tool("fake", "echo", r#"{"text":"deny"}"#)
        .await;
    assert_eq!(deny_handler.calls(), 1);
    assert_eq!(registry.server_trust("fake").await, McpServerTrust::Denied,);

    // Now install a handler that would approve -- it should NOT be called.
    let approve_handler = FixedTrustHandler::new(TrustApprovalResponse::Trusted);
    let approve_ref = Arc::clone(&approve_handler);
    registry
        .set_trust_approval_handler(approve_ref as SharedTrustApprovalHandler)
        .await;

    let error = registry
        .invoke_tool("fake", "echo", r#"{"text":"retry"}"#)
        .await
        .expect_err("tool call should still fail for Denied server");

    assert!(
        matches!(error, McpError::ServerUntrusted { ref server, status: "denied" } if server == "fake"),
        "expected ServerUntrusted/denied, got: {error}"
    );
    assert_eq!(
        approve_handler.calls(),
        0,
        "handler should NOT be called for Denied servers"
    );
}
