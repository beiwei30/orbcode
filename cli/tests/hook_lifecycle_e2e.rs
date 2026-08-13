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

    fn write_user_settings(&self, json: &str) {
        std::fs::write(self.home_path.join("settings.json"), json).expect("write user settings");
    }

    fn write_local_hook(&self, event: &str, command: &str) {
        let dir = self.cwd.path().join(".claude");
        std::fs::create_dir_all(&dir).expect("create .claude");
        let body = serde_json::json!({
            "hooks": {
                event: [
                    {
                        "matcher": "bash",
                        "hooks": [
                            { "type": "command", "command": command, "timeout": 5.0 }
                        ]
                    }
                ]
            }
        });
        std::fs::write(dir.join("settings.local.json"), body.to_string())
            .expect("write settings.local.json");
    }

    fn run_with_env(&self, args: &[&str], extra: &[(&str, &str)]) -> (i32, String, String) {
        let mut cmd = Command::new(ORBCODE_BIN);
        cmd.args(args)
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
            .stderr(Stdio::piped());
        for (key, value) in extra {
            cmd.env(key, value);
        }
        let output = cmd.output().expect("spawn orbcode");
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        (code, stdout, stderr)
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        self.run_with_env(args, &[])
    }
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("each stream-json line is JSON"))
        .collect()
}

fn tool_result_block(records: &[Value]) -> Value {
    for record in records {
        if record["type"] != "user" {
            continue;
        }
        let Some(blocks) = record["message"]["content"].as_array() else {
            continue;
        };
        for block in blocks {
            if block["type"] == "tool_result" {
                return block.clone();
            }
        }
    }
    panic!("no tool_result block found in records: {records:#?}");
}

fn tool_result_text(block: &Value) -> String {
    if let Some(text) = block["content"].as_str() {
        return text.to_string();
    }
    if let Some(items) = block["content"].as_array() {
        return items
            .iter()
            .filter_map(|item| item["text"].as_str().or_else(|| item.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

const BASH_TOOL_PROMPT: &str = "#tool:bash {\"command\":\"echo hi\"}";

const COMMON_ARGS: &[&str] = &[
    "-p",
    BASH_TOOL_PROMPT,
    "--output-format",
    "stream-json",
    "--verbose",
    "--permission-mode",
    "default",
    "--allowed-tools",
    "bash",
];

#[test]
fn pre_tool_local_hook_denial_labels_settings_local_source_in_tool_result() {
    let harness = Harness::new();
    harness.write_local_hook("PreToolUse", "printf '%s' 'local hook crashed' >&2; exit 1");

    let (code, stdout, stderr) = harness.run(COMMON_ARGS);
    // The hook blocks the only tool the model attempts and the turn ends with no
    // recovered output, so the headless run reports the dedicated PermissionDenied
    // exit code (4) rather than a generic success.
    assert_eq!(code, 4, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let tool_result = tool_result_block(&records);
    assert_eq!(
        tool_result["is_error"], true,
        "tool_result must report error: {tool_result}"
    );
    let text = tool_result_text(&tool_result);
    assert!(
        text.contains("[settings.local.json]"),
        "tool_result text missing source label: {text}"
    );
    assert!(
        text.contains("local hook crashed"),
        "tool_result text missing captured stderr: {text}"
    );
    assert!(
        text.contains("exit status: 1"),
        "tool_result text missing exit status: {text}"
    );
}

#[test]
fn pre_tool_user_settings_hook_denial_omits_source_label_in_tool_result() {
    let harness = Harness::new();
    harness.write_user_settings(
        r#"{
            "env": {"ANTHROPIC_BASE_URL":"stub://test","ANTHROPIC_API_KEY":"stub-key"},
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "bash",
                        "hooks": [
                            {"type":"command","command":"printf '%s' 'user hook crashed' >&2; exit 1","timeout":5.0}
                        ]
                    }
                ]
            }
        }"#,
    );

    let (code, stdout, stderr) = harness.run(COMMON_ARGS);
    // Same denial-terminates-the-turn contract as the local-hook case: a blocked
    // tool with no recovered output surfaces the PermissionDenied exit code (4).
    assert_eq!(code, 4, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let tool_result = tool_result_block(&records);
    assert_eq!(tool_result["is_error"], true);
    let text = tool_result_text(&tool_result);
    assert!(
        !text.contains("[settings.local.json]") && !text.contains("[settings.json]"),
        "user-settings hook must not carry a bracketed source label: {text}"
    );
    assert!(
        text.contains("user hook crashed"),
        "tool_result text missing captured stderr: {text}"
    );
    assert!(
        text.contains("exit status: 1"),
        "tool_result text missing exit status: {text}"
    );
}

#[test]
fn untrusted_project_skips_local_settings_pre_tool_hook() {
    let harness = Harness::new();
    harness.write_local_hook("PreToolUse", "printf '%s' 'should be skipped' >&2; exit 1");

    let (code, stdout, stderr) =
        harness.run_with_env(COMMON_ARGS, &[("ORBCODE_TRUSTED_PROJECT", "0")]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let tool_result = tool_result_block(&records);
    let is_error = tool_result["is_error"].as_bool().unwrap_or(false);
    let text = tool_result_text(&tool_result);
    assert!(
        !is_error,
        "tool should run successfully when local hook is skipped: {tool_result}"
    );
    assert!(
        !text.contains("[settings.local.json]"),
        "skipped local hook must not appear in tool_result: {text}"
    );
    assert!(
        !text.contains("should be skipped"),
        "skipped local hook stderr must not leak into tool_result: {text}"
    );
    assert!(
        text.contains("hi"),
        "expected echo output in tool_result: {text}"
    );
}
