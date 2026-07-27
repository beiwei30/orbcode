//! End-to-end tests for the control channel `set_permission_mode` request
//! followed by tool execution. Verifies the complete flow:
//! 1. Default mode denies tool (baseline)
//! 2. `set_permission_mode` to `bypassPermissions` via control frame
//! 3. Subsequent tool call succeeds without permission denial
//!
//! This covers the SDK use case where a client dynamically escalates
//! permissions mid-session via the bidirectional control channel.

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

    fn base_command(&self) -> Command {
        let mut cmd = Command::new(ORBCODE_BIN);
        cmd.args([
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
        .stderr(Stdio::piped());
        cmd
    }

    fn command(&self) -> Command {
        let mut cmd = self.base_command();
        cmd.args(["--allowed-tools", "bash"]);
        cmd
    }

    fn run_eof(&self, stdin: &str) -> (i32, String, String) {
        self.run_eof_cmd(self.command(), stdin)
    }

    fn run_eof_no_allowed_tools(&self, stdin: &str) -> (i32, String, String) {
        self.run_eof_cmd(self.base_command(), stdin)
    }

    fn run_eof_cmd(&self, mut cmd: Command, stdin: &str) -> (i32, String, String) {
        let mut child = cmd.spawn().expect("spawn orbcode");
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

fn set_permission_mode_frame(request_id: &str, mode: &str) -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "set_permission_mode",
            "mode": mode,
        }
    })
    .to_string()
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("each stream-json line is JSON"))
        .collect()
}

fn control_response<'a>(records: &'a [Value], request_id: &str) -> Option<&'a Value> {
    records.iter().find(|record| {
        record["type"] == "control_response"
            && record["response"]["request_id"].as_str() == Some(request_id)
    })
}

const BASH_PROMPT: &str = "#tool:bash {\"command\":\"echo permission_test\"}";

#[test]
fn set_permission_mode_bypass_then_tool_succeeds() {
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        set_permission_mode_frame("perm-1", "bypassPermissions"),
        user_frame(BASH_PROMPT),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(
        code, 0,
        "set_permission_mode + tool call should succeed; stderr: {stderr}\nstdout:\n{stdout}"
    );

    let records = parse_lines(&stdout);

    // Control response acknowledged
    let ack =
        control_response(&records, "perm-1").expect("control_response for set_permission_mode");
    assert_eq!(ack["response"]["subtype"], "success");

    // Tool should have run (user record with tool_result)
    let tool_ran = records.iter().any(|r| {
        r["type"] == "user"
            && r["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
    });
    assert!(
        tool_ran,
        "after set_permission_mode bypassPermissions, the tool must execute"
    );

    // No permission denials
    let result = records.last().expect("result record");
    assert_eq!(result["subtype"], "success");
    assert!(result["permission_denials"].as_array().unwrap().is_empty());
}

#[test]
fn without_set_permission_mode_tool_is_denied() {
    let harness = Harness::new();
    // Same prompt but no permission escalation
    let stdin = format!("{}\n", user_frame(BASH_PROMPT));
    let (code, stdout, stderr) = harness.run_eof_no_allowed_tools(&stdin);
    assert_eq!(
        code, 4,
        "without permission mode change, tool should be denied (exit 4); stderr: {stderr}\nstdout:\n{stdout}"
    );
    let records = parse_lines(&stdout);
    let result = records.last().expect("result record");
    assert!(
        !result["permission_denials"].as_array().unwrap().is_empty(),
        "tool denial must be recorded"
    );
}

#[test]
fn set_permission_mode_accept_edits_still_allows_bash_with_allowed_tools() {
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        set_permission_mode_frame("perm-2", "acceptEdits"),
        user_frame(BASH_PROMPT),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    // acceptEdits + --allowed-tools bash should allow the bash tool
    assert_eq!(
        code, 0,
        "acceptEdits + allowed-tools bash should succeed; stderr: {stderr}\nstdout:\n{stdout}"
    );
    let records = parse_lines(&stdout);
    let ack =
        control_response(&records, "perm-2").expect("control_response for set_permission_mode");
    assert_eq!(ack["response"]["subtype"], "success");
}

#[test]
fn set_permission_mode_invalid_mode_returns_error() {
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        set_permission_mode_frame("perm-bad", "invalidMode"),
        user_frame("say hi"),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(
        code, 0,
        "invalid mode should not crash the process; stderr: {stderr}"
    );
    let records = parse_lines(&stdout);
    let err = control_response(&records, "perm-bad").expect("control_response for invalid mode");
    assert_eq!(
        err["response"]["subtype"], "error",
        "invalid permission mode must return error response"
    );
}

#[test]
fn set_permission_mode_acknowledged_before_turn_starts() {
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        set_permission_mode_frame("perm-first", "bypassPermissions"),
        user_frame("say hi"),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);

    // Find positions of control_response and first assistant record
    let ack_pos = records
        .iter()
        .position(|r| {
            r["type"] == "control_response"
                && r["response"]["request_id"].as_str() == Some("perm-first")
        })
        .expect("control_response must be present");

    let assistant_pos = records
        .iter()
        .position(|r| r["type"] == "assistant")
        .expect("assistant record must be present");

    assert!(
        ack_pos < assistant_pos,
        "control_response (pos {ack_pos}) must appear before first assistant record (pos {assistant_pos})"
    );
}
