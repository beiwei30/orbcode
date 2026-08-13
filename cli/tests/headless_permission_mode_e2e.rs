//! End-to-end tests verifying `--permission-mode` output differences.
//! Each mode changes how tool calls are gated:
//! - `default`: workspace-safe tools run; boundary requests are denied in headless
//! - `bypassPermissions`: all tools run without permission requests
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
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
        let mut command = Command::new(ORBCODE_BIN);
        command
            .args(args)
            .current_dir(self.cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", &self.home_path)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home_path)
            .env("ANTHROPIC_BASE_URL", "stub://test")
            .env("ANTHROPIC_API_KEY", "stub-key")
            .env("RUST_LOG", "warn")
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().expect("spawn orbcode");
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
fn default_mode_denies_boundary_request_in_headless() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo hello\",\"sandbox_permissions\":\"require_escalated\"}",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
    ]);
    assert_eq!(
        code, 4,
        "default mode should deny a boundary request (exit 4); stderr: {stderr}"
    );
    let records = parse_lines(&stdout);

    // Permission denial should be recorded
    let result = records.last().expect("result");
    assert_eq!(result["is_error"], true);
    assert!(
        !result["permission_denials"].as_array().unwrap().is_empty(),
        "result must record the boundary denial in default mode"
    );
}

#[test]
fn default_mode_runs_workspace_safe_bash_without_permission_request() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo workspace-safe\"}",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    assert!(
        !records.iter().any(|record| record["type"] == "stream_event"
            && record["event"]["type"] == "permission_requested")
    );
    assert!(records.iter().any(|record| {
        record["type"] == "stream_event"
            && record["event"]["type"] == "tool_use_completed"
            && record["event"]["kind"] == "success"
    }));
}

#[test]
fn implicit_default_preserves_explicit_read_only_sandbox() {
    let harness = Harness::new();
    let target = harness.cwd.path().join("must-not-be-written.txt");
    let prompt = format!(
        "#tool:Write {}",
        serde_json::json!({"file_path": target, "content": "blocked"})
    );
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        &prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--sandbox-mode",
        "read-only",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    assert!(!target.exists(), "read-only sandbox must block the write");
    let records = parse_lines(&stdout);
    assert!(records.iter().any(|record| {
        record["type"] == "stream_event"
            && record["event"]["type"] == "tool_use_completed"
            && record["event"]["kind"] == "execution_failed"
    }));
}

#[test]
fn implicit_default_preserves_explicit_allow_tools_false() {
    let harness = Harness::new();
    let target = harness.cwd.path().join("tools-disabled.txt");
    let prompt = format!(
        "#tool:Write {}",
        serde_json::json!({"file_path": target, "content": "blocked"})
    );
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        &prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--allow-tools",
        "false",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    assert!(!target.exists(), "allow-tools=false must block the write");
    let records = parse_lines(&stdout);
    assert!(
        records.iter().any(|record| {
            record["type"] == "stream_event"
                && record["event"]["type"] == "tool_use_completed"
                && record["event"]["kind"] == "execution_failed"
        }),
        "a simulated tool call must still fail at the execution boundary"
    );
}

#[test]
fn implicit_default_preserves_environment_read_only_sandbox() {
    let harness = Harness::new();
    let target = harness.cwd.path().join("env-must-not-be-written.txt");
    let prompt = format!(
        "#tool:Write {}",
        serde_json::json!({"file_path": target, "content": "blocked"})
    );
    let (code, stdout, stderr) = harness.run_with_env(
        &["-p", &prompt, "--output-format", "stream-json", "--verbose"],
        &[("ORBCODE_SANDBOX_MODE", "read-only")],
    );
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    assert!(!target.exists(), "environment read-only must block writes");
}

#[test]
fn implicit_default_preserves_environment_allow_tools_false() {
    let harness = Harness::new();
    let target = harness.cwd.path().join("env-tools-disabled.txt");
    let prompt = format!(
        "#tool:Write {}",
        serde_json::json!({"file_path": target, "content": "blocked"})
    );
    let (code, stdout, stderr) = harness.run_with_env(
        &["-p", &prompt, "--output-format", "stream-json", "--verbose"],
        &[("ORBCODE_ALLOW_TOOLS", "false")],
    );
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    assert!(
        !target.exists(),
        "environment allow-tools=false must block writes"
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
        "#tool:bash {\"command\":\"echo hello\",\"sandbox_permissions\":\"require_escalated\"}",
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
fn legacy_permission_modes_map_to_current_policies() {
    for (mode, expected) in [
        ("acceptEdits", "default"),
        ("accept-edits", "default"),
        ("dontAsk", "bypassPermissions"),
        ("dont-ask", "bypassPermissions"),
    ] {
        let harness = Harness::new();
        let (code, stdout, stderr) = harness.run(&[
            "-p",
            "say hi",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            mode,
        ]);
        assert_eq!(code, 0, "mode {mode} should remain compatible: {stderr}");
        let records = parse_lines(&stdout);
        assert_eq!(records[0]["permissionMode"], expected, "mode {mode}");
    }
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
