//! Process-level E2E tests for `orbcode serve --stdio`.
//!
//! These tests spawn the actual binary and communicate over stdin/stdout pipes,
//! verifying the NDJSON transport protocol works end-to-end.

use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

struct ServeProcess {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    _home: TempDir,
    _cwd: TempDir,
}

impl ServeProcess {
    async fn spawn() -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");

        // Write minimal settings so config doesn't fail looking for auth.
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
        let reader = BufReader::new(stdout);

        Self {
            child,
            stdin,
            reader,
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
        let result =
            tokio::time::timeout(Duration::from_secs(10), self.reader.read_line(&mut line)).await;
        match result {
            Ok(Ok(0)) => None,
            Ok(Ok(_)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(serde_json::from_str(trimmed).expect("valid JSON response"))
            }
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => panic!("timeout waiting for response"),
        }
    }

    async fn close_stdin(self) -> tokio::process::Child {
        drop(self.stdin);
        self.child
    }
}

fn initialize_request(id: &str) -> Value {
    json!({
        "type": "request",
        "id": id,
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "e2e-test", "version": "0.1" }
        }
    })
}

fn session_list_request(id: &str) -> Value {
    json!({
        "type": "request",
        "id": id,
        "method": "session/list"
    })
}

// -------------------------------------------------------------------
// 1. Binary starts and responds to initialize
// -------------------------------------------------------------------
#[tokio::test]
async fn binary_starts_and_responds_to_initialize() {
    let mut proc = ServeProcess::spawn().await;

    proc.send(&initialize_request("init-1")).await;
    let resp = proc.recv().await.expect("should get response");

    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "init-1");
    // Should have server_info in the result
    assert!(
        resp["result"]["data"]["server_info"]["name"]
            .as_str()
            .is_some(),
        "expected server_info.name in response: {resp:?}"
    );

    let mut child = proc.close_stdin().await;
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("child should exit")
        .expect("wait");
    assert!(status.success(), "exit status: {status}");
}

// -------------------------------------------------------------------
// 2. Session list after initialize
// -------------------------------------------------------------------
#[tokio::test]
async fn session_list_after_initialize() {
    let mut proc = ServeProcess::spawn().await;

    proc.send(&initialize_request("init-1")).await;
    let _init_resp = proc.recv().await.expect("init response");

    proc.send(&session_list_request("sl-1")).await;
    let resp = proc.recv().await.expect("session list response");

    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "sl-1");
    // Fresh home dir should have an empty session list (array).
    let data = &resp["result"]["data"];
    assert!(data.is_array(), "expected array, got: {data:?}");

    let mut child = proc.close_stdin().await;
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("child should exit")
        .expect("wait");
    assert!(status.success());
}

// -------------------------------------------------------------------
// 3. Unknown method returns MethodNotFound error
// -------------------------------------------------------------------
#[tokio::test]
async fn unknown_method_returns_error() {
    let mut proc = ServeProcess::spawn().await;

    proc.send(&initialize_request("init-1")).await;
    let _init_resp = proc.recv().await.expect("init response");

    let bogus = json!({
        "type": "request",
        "id": "bogus-1",
        "method": "totally/unknown"
    });
    proc.send(&bogus).await;
    let resp = proc.recv().await.expect("error response");

    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "bogus-1");
    // Should be an error response with method_not_found code
    let result = &resp["result"];
    assert_eq!(
        result["status"], "error",
        "expected error status in result: {resp:?}"
    );
    assert_eq!(result["code"], "method_not_found");

    let mut child = proc.close_stdin().await;
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("child should exit")
        .expect("wait");
    assert!(status.success());
}

// -------------------------------------------------------------------
// 4. EOF exits cleanly
// -------------------------------------------------------------------
#[tokio::test]
async fn eof_exits_cleanly() {
    let mut proc = ServeProcess::spawn().await;

    proc.send(&initialize_request("init-1")).await;
    let _init_resp = proc.recv().await.expect("init response");

    // Close stdin (EOF signal)
    let mut child = proc.close_stdin().await;

    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("child should exit within timeout")
        .expect("wait");
    assert!(
        status.success(),
        "process should exit with code 0: {status}"
    );
}

// -------------------------------------------------------------------
// 5. Malformed JSON is skipped, valid requests still work
// -------------------------------------------------------------------
#[tokio::test]
async fn malformed_json_skipped() {
    let mut proc = ServeProcess::spawn().await;

    // Send garbage first
    proc.stdin
        .write_all(b"this is not json at all\n")
        .await
        .unwrap();
    proc.stdin.flush().await.unwrap();

    // Then send a valid initialize
    proc.send(&initialize_request("init-1")).await;
    let resp = proc
        .recv()
        .await
        .expect("should get response after garbage");

    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "init-1");

    let mut child = proc.close_stdin().await;
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("child should exit")
        .expect("wait");
    assert!(status.success());
}
