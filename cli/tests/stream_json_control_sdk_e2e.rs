//! End-to-end coverage for the SDK control extensions: `get_session_state`,
//! `get_context_usage`, and `set_max_thinking_tokens` control requests.
//!
//! Each test spawns the real `orbcode` binary with `--input-format stream-json`
//! and feeds NDJSON control frames on stdin to verify the bidirectional contract.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

struct Harness {
    _home: TempDir,
    cwd: TempDir,
    home_path: std::path::PathBuf,
}

impl Harness {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_BASE_URL":"stub://test","ANTHROPIC_API_KEY":"stub-key"}}"#,
        )
        .expect("write settings");
        let home_path = home.path().to_path_buf();
        Self {
            _home: home,
            cwd,
            home_path,
        }
    }

    fn run_eof(&self, stdin: &str) -> (i32, String, String) {
        let mut child = Command::new(ORBCODE_BIN)
            .args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
            ])
            .current_dir(self.cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", &self.home_path)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home_path)
            .env("ANTHROPIC_BASE_URL", "stub://test")
            .env("ANTHROPIC_API_KEY", "stub-key")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn orbcode");
        {
            let mut pipe = child.stdin.take().expect("stdin pipe");
            pipe.write_all(stdin.as_bytes()).expect("write stdin");
            pipe.flush().expect("flush stdin");
        }
        let output = child.wait_with_output().expect("wait orbcode");
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        (code, stdout, stderr)
    }
}

fn user_frame(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": text},
    })
    .to_string()
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("each line is JSON"))
        .collect()
}

fn control_response<'a>(records: &'a [Value], request_id: &str) -> Option<&'a Value> {
    records.iter().find(|record| {
        record["type"] == "control_response"
            && record["response"]["request_id"].as_str() == Some(request_id)
    })
}

fn control_frame(request_id: &str, request: Value) -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": request,
    })
    .to_string()
}

#[test]
fn get_session_state_returns_data() {
    let harness = Harness::new();
    let ctrl = control_frame("ss-1", serde_json::json!({"subtype": "get_session_state"}));
    let stdin = format!("{ctrl}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let response = control_response(&records, "ss-1").expect("session_state response");
    assert_eq!(response["response"]["subtype"], "success");
    let data = &response["response"]["response"];
    assert!(
        data["session_id"].is_string(),
        "session_id must be a string"
    );
    assert!(
        data["model_name"].is_string(),
        "model_name must be a string"
    );
    assert!(data["cwd"].is_string(), "cwd must be a string");
    assert!(
        data["available_tool_count"].is_number(),
        "available_tool_count must be a number"
    );
}

#[test]
fn get_context_usage_returns_data() {
    let harness = Harness::new();
    let ctrl = control_frame("cu-1", serde_json::json!({"subtype": "get_context_usage"}));
    let stdin = format!("{ctrl}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let response = control_response(&records, "cu-1").expect("context_usage response");
    assert_eq!(response["response"]["subtype"], "success");
    let data = &response["response"]["response"];
    assert!(data["model"].is_string(), "model must be a string");
    assert!(
        data["context_window"].is_number(),
        "context_window must be a number"
    );
    assert!(
        data["estimated_tokens"].is_number(),
        "estimated_tokens must be a number"
    );
    assert!(
        data["categories"].is_object(),
        "categories must be an object"
    );
}

#[test]
fn set_max_thinking_tokens_number_returns_unsupported_error() {
    let harness = Harness::new();
    let ctrl = control_frame(
        "mt-1",
        serde_json::json!({"subtype": "set_max_thinking_tokens", "max_thinking_tokens": 4096}),
    );
    let stdin = format!("{ctrl}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let response = control_response(&records, "mt-1").expect("set_max_thinking_tokens response");
    assert_eq!(response["response"]["subtype"], "error");
    assert!(
        response["response"]["error"]
            .as_str()
            .expect("error string")
            .contains("set_max_thinking_tokens"),
        "{response:?}"
    );
}

#[test]
fn set_max_thinking_tokens_null_returns_unsupported_error() {
    let harness = Harness::new();
    let ctrl = control_frame(
        "mt-2",
        serde_json::json!({"subtype": "set_max_thinking_tokens", "max_thinking_tokens": null}),
    );
    let stdin = format!("{ctrl}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let response = control_response(&records, "mt-2").expect("set_max_thinking_tokens response");
    assert_eq!(response["response"]["subtype"], "error");
    assert!(
        response["response"]["error"]
            .as_str()
            .expect("error string")
            .contains("no runtime thinking-token override"),
        "{response:?}"
    );
}

#[test]
fn set_max_thinking_tokens_string_returns_validation_error() {
    let harness = Harness::new();
    let ctrl = control_frame(
        "mt-bad",
        serde_json::json!({"subtype": "set_max_thinking_tokens", "max_thinking_tokens": "lots"}),
    );
    let stdin = format!("{ctrl}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let response = control_response(&records, "mt-bad").expect("set_max_thinking_tokens response");
    assert_eq!(response["response"]["subtype"], "error");
    assert!(
        response["response"]["error"]
            .as_str()
            .expect("error string")
            .contains("invalid control_request"),
        "{response:?}"
    );
}

/// Verify that `get_session_state` works mid-turn: a bash tool holds the turn
/// active while the control frame is processed via `handle_mid_turn_frame`.
#[test]
fn get_session_state_mid_turn() {
    let harness = Harness::new();
    let mode = control_frame(
        "mode-1",
        serde_json::json!({"subtype": "set_permission_mode", "mode": "bypassPermissions"}),
    );
    let prompt = user_frame(r#"#tool:bash {"command":"sleep 0.1"}"#);
    let query = control_frame(
        "mid-ss",
        serde_json::json!({"subtype": "get_session_state"}),
    );
    let stdin = format!("{mode}\n{prompt}\n{query}\n");
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);

    let mode_ack = control_response(&records, "mode-1").expect("mode ack");
    assert_eq!(mode_ack["response"]["subtype"], "success");

    let mid = control_response(&records, "mid-ss").expect("mid-turn session_state response");
    assert_eq!(mid["response"]["subtype"], "success");
    let data = &mid["response"]["response"];
    assert!(
        data["session_id"].is_string(),
        "mid-turn session_state must return session_id"
    );
    assert!(
        data["model_name"].is_string(),
        "mid-turn session_state must return model_name"
    );
}

/// Verify that `get_context_usage` works mid-turn.
#[test]
fn get_context_usage_mid_turn() {
    let harness = Harness::new();
    let mode = control_frame(
        "mode-2",
        serde_json::json!({"subtype": "set_permission_mode", "mode": "bypassPermissions"}),
    );
    let prompt = user_frame(r#"#tool:bash {"command":"sleep 0.1"}"#);
    let query = control_frame(
        "mid-cu",
        serde_json::json!({"subtype": "get_context_usage"}),
    );
    let stdin = format!("{mode}\n{prompt}\n{query}\n");
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let mid = control_response(&records, "mid-cu").expect("mid-turn context_usage response");
    assert_eq!(mid["response"]["subtype"], "success");
    let data = &mid["response"]["response"];
    assert!(
        data["context_window"].is_number(),
        "mid-turn context_usage must return context_window"
    );
}
