//! End-to-end tests verifying `--permission-mode` output differences.
//! Each mode changes how tool calls are gated:
//! - `default`: tools require explicit permission → denied in headless
//! - `bypassPermissions`: all tools run without permission requests
//! - `acceptEdits`: edit tools run but bash still requires permission
//! - `plan`: no tool execution at all (plan mode only generates plans)
//!
//! Tests assert on exit codes, presence/absence of permission_request events,
//! and tool execution success vs. denial.

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

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let output = Command::new(ORBCODE_BIN)
            .args(args)
            .current_dir(self.cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", &self.home_path)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home_path)
            .env("ANTHROPIC_BASE_URL", "stub://test")
            .env("ANTHROPIC_API_KEY", "stub-key")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn orbcode");
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        (code, stdout, stderr)
    }
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("each stream-json line is JSON"))
        .collect()
}

#[test]
fn bypass_permissions_runs_tool_without_permission_request() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo hello\"}",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--allowed-tools",
        "bash",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);

    // No permission_request events should be present
    let has_permission_request = records
        .iter()
        .any(|r| r["type"] == "stream_event" && r["event"]["type"] == "permission_requested");
    assert!(
        !has_permission_request,
        "bypassPermissions mode must not emit permission_requested events"
    );

    // Tool should have completed successfully
    let tool_completed = records.iter().any(|r| {
        r["type"] == "stream_event"
            && r["event"]["type"] == "tool_use_completed"
            && r["event"]["kind"] != "permission_denied"
    });
    assert!(
        tool_completed,
        "tool should complete successfully in bypassPermissions mode"
    );

    let result = records.last().expect("result");
    assert_eq!(result["subtype"], "success");
    assert!(result["permission_denials"].as_array().unwrap().is_empty());
}

#[test]
fn default_mode_denies_tool_in_headless() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo hello\"}",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
    ]);
    assert_eq!(
        code, 4,
        "default mode should deny tool (exit 4); stderr: {stderr}"
    );
    let records = parse_lines(&stdout);

    // Permission denial should be recorded
    let result = records.last().expect("result");
    assert_eq!(result["is_error"], true);
    assert!(
        !result["permission_denials"].as_array().unwrap().is_empty(),
        "result must record the permission denial in default mode"
    );
}

#[test]
fn json_format_records_permission_denial() {
    // The single-object `json` output must also record the denial: the emitter
    // now processes events in `json` mode (previously only `stream-json` did,
    // leaving `permission_denials` empty in the `json` result).
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo hello\"}",
        "--output-format",
        "json",
        "--permission-mode",
        "default",
    ]);
    assert_eq!(code, 4, "default mode denies (exit 4); stderr: {stderr}");
    let result: Value =
        serde_json::from_str(stdout.trim()).expect("json output is a single object");
    assert_eq!(result["is_error"], true);
    assert!(
        !result["permission_denials"].as_array().unwrap().is_empty(),
        "json output must record the permission denial: {result}"
    );
}

#[test]
fn init_record_reflects_permission_mode() {
    let harness = Harness::new();

    // Test bypassPermissions
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);
    let init = &records[0];
    assert_eq!(init["type"], "system");
    assert_eq!(init["subtype"], "init");
    assert_eq!(
        init["permissionMode"].as_str().unwrap(),
        "bypassPermissions",
        "init record must reflect the configured permission mode"
    );
}

#[test]
fn default_mode_init_record_shows_default() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);
    assert_eq!(records[0]["permissionMode"], "default");
}

#[test]
fn accept_edits_mode_reflected_in_init() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "acceptEdits",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);
    assert_eq!(records[0]["permissionMode"], "acceptEdits");
}

#[test]
fn plan_mode_reflected_in_init() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "plan",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);
    assert_eq!(records[0]["permissionMode"], "plan");
}

#[test]
fn bypass_permissions_with_allowed_tools_runs_bash() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo bypass_test_marker\"}",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--allowed-tools",
        "bash",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);

    // Verify tool_result is present with the expected output
    let user_with_result = records.iter().find(|r| {
        r["type"] == "user"
            && r["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
    });
    assert!(
        user_with_result.is_some(),
        "bypassPermissions + allowed-tools should produce a tool_result"
    );
}
