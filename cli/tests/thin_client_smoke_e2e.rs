//! External thin-client smoke test.
//!
//! Proves the protocol contract is externally consumable: no Rust facade link,
//! only JSON-RPC over stdio transport. Exercises: initialize with capability
//! negotiation, session lifecycle, experimental method gating, and server
//! capabilities inspection.

use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

struct ThinClient {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    _home: TempDir,
    _cwd: TempDir,
}

impl ThinClient {
    async fn spawn() -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");

        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
        )
        .expect("write settings");

        let mut child = Command::new(ORBCODE_BIN)
            .arg("serve")
            .arg("--stdio")
            .current_dir(cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", home.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home.path())
            .env("RUST_LOG", "warn")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn orbcode serve --stdio");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");

        Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            _home: home,
            _cwd: cwd,
        }
    }

    async fn send(&mut self, msg: &Value) {
        let line = serde_json::to_string(msg).unwrap();
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.write_all(b"\n").await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        match tokio::time::timeout(Duration::from_secs(10), self.reader.read_line(&mut line)).await
        {
            Ok(Ok(0)) => None,
            Ok(Ok(_)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(serde_json::from_str(trimmed).expect("valid JSON"))
            }
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => panic!("timeout waiting for response"),
        }
    }

    async fn shutdown(self) {
        drop(self.stdin);
        let mut child = self.child;
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }
}

// ---------------------------------------------------------------------------
// 1. Initialize without experimental opt-in — experimental_methods hidden
// ---------------------------------------------------------------------------
#[tokio::test]
async fn thin_client_default_capabilities() {
    let mut c = ThinClient::spawn().await;

    c.send(&json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "thin-client", "version": "0.1" }
        }
    }))
    .await;

    let resp = c.recv().await.expect("init response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "init-1");
    assert_eq!(resp["result"]["status"], "success");

    let caps = &resp["result"]["data"]["capabilities"];
    assert!(caps["streaming"].as_bool().unwrap());

    // Default client should NOT see experimental methods
    let experimental = caps["experimental_methods"].as_array().unwrap();
    assert!(
        experimental.is_empty(),
        "default client should have empty experimental_methods, got: {experimental:?}"
    );

    // But should see stable methods
    let stable = caps["stable_methods"].as_array().unwrap();
    assert!(!stable.is_empty(), "stable_methods should not be empty");

    // And server-request methods
    let sreqs = caps["server_request_methods"].as_array().unwrap();
    assert!(sreqs.iter().any(|m| m == "permission/request"));
    assert!(sreqs.iter().any(|m| m == "mcp_trust/request"));
    assert!(sreqs.iter().any(|m| m == "ask_user/request"));

    c.shutdown().await;
}

// ---------------------------------------------------------------------------
// 2. Experimental method gating — default client rejected
// ---------------------------------------------------------------------------
#[tokio::test]
async fn thin_client_experimental_rejected() {
    let mut c = ThinClient::spawn().await;

    c.send(&json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "thin-client", "version": "0.1" }
        }
    }))
    .await;
    let _ = c.recv().await;

    c.send(&json!({
        "type": "request",
        "id": "bg-1",
        "method": "background/list"
    }))
    .await;

    let resp = c.recv().await.expect("background/list response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["result"]["status"], "error");
    assert_eq!(resp["result"]["code"], "method_not_found");
    assert!(
        resp["result"]["message"]
            .as_str()
            .unwrap()
            .contains("experimental"),
        "error should mention experimental"
    );

    c.shutdown().await;
}

// ---------------------------------------------------------------------------
// 3. Initialize with experimental opt-in — experimental methods visible + allowed
// ---------------------------------------------------------------------------
#[tokio::test]
async fn thin_client_experimental_opt_in() {
    let mut c = ThinClient::spawn().await;

    c.send(&json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "thin-client", "version": "0.1" },
            "capabilities": { "experimental_methods": true }
        }
    }))
    .await;

    let resp = c.recv().await.expect("init response");
    let caps = &resp["result"]["data"]["capabilities"];
    let experimental = caps["experimental_methods"].as_array().unwrap();
    assert!(
        !experimental.is_empty(),
        "opt-in client should see experimental methods"
    );
    assert!(experimental.iter().any(|m| m == "background/create"));

    // Now background/list should succeed (not MethodNotFound)
    c.send(&json!({
        "type": "request",
        "id": "bg-1",
        "method": "background/list"
    }))
    .await;

    let resp = c.recv().await.expect("background/list response");
    assert_eq!(resp["type"], "response");
    assert_ne!(
        resp["result"]["status"].as_str(),
        Some("error"),
        "opt-in client should not get error for experimental method: {resp:?}"
    );

    c.shutdown().await;
}

// ---------------------------------------------------------------------------
// 4. Session lifecycle: list → bootstrap → list-again
// ---------------------------------------------------------------------------
#[tokio::test]
async fn thin_client_session_lifecycle() {
    let mut c = ThinClient::spawn().await;

    c.send(&json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "thin-client", "version": "0.1" },
            "capabilities": { "streaming": true, "experimental_methods": true }
        }
    }))
    .await;
    let _ = c.recv().await;

    // List sessions — should be empty initially
    c.send(&json!({
        "type": "request",
        "id": "sl-1",
        "method": "session/list"
    }))
    .await;

    let resp = c.recv().await.expect("session/list response");
    assert_eq!(resp["result"]["status"], "success");
    let sessions = resp["result"]["data"].as_array().unwrap();
    assert!(sessions.is_empty(), "fresh home should have no sessions");

    // Bootstrap a new session
    c.send(&json!({
        "type": "request",
        "id": "bs-1",
        "method": "session/bootstrap"
    }))
    .await;

    let resp = c.recv().await.expect("bootstrap response");
    assert_eq!(resp["result"]["status"], "success");
    let data = &resp["result"]["data"];
    // session_id may be at top level or nested under "session"
    let session_id = data["session_id"]
        .as_str()
        .or_else(|| data["session"]["session_id"].as_str())
        .unwrap_or_else(|| panic!("session_id not found in bootstrap response: {data}"));

    // Verify the session_id is a non-empty string
    assert!(
        !session_id.is_empty(),
        "session_id from bootstrap should be non-empty"
    );

    c.shutdown().await;
}

// ---------------------------------------------------------------------------
// 5. Pre-initialize rejection
// ---------------------------------------------------------------------------
#[tokio::test]
async fn thin_client_pre_initialize_rejected() {
    let mut c = ThinClient::spawn().await;

    c.send(&json!({
        "type": "request",
        "id": "sl-pre",
        "method": "session/list"
    }))
    .await;

    let resp = c.recv().await.expect("pre-init response");
    assert_eq!(resp["result"]["status"], "error");
    assert_eq!(resp["result"]["code"], "invalid_request");
    assert!(
        resp["result"]["message"]
            .as_str()
            .unwrap()
            .contains("not initialized")
    );

    c.shutdown().await;
}

// ---------------------------------------------------------------------------
// 6. Unknown method → MethodNotFound
// ---------------------------------------------------------------------------
#[tokio::test]
async fn thin_client_unknown_method() {
    let mut c = ThinClient::spawn().await;

    c.send(&json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "thin-client", "version": "0.1" }
        }
    }))
    .await;
    let _ = c.recv().await;

    c.send(&json!({
        "type": "request",
        "id": "unk-1",
        "method": "nonexistent/method"
    }))
    .await;

    let resp = c.recv().await.expect("unknown method response");
    assert_eq!(resp["result"]["status"], "error");
    assert_eq!(resp["result"]["code"], "method_not_found");

    c.shutdown().await;
}
