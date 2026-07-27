use std::path::Path;
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
        self.run_with_base_url("stub://test", args)
    }

    /// Like [`run`], but points the Anthropic provider at an explicit base URL.
    /// Tests use a `mock://` URL to drive provider failure scenarios from
    /// config instead of injecting markers into the prompt.
    fn run_with_base_url(&self, base_url: &str, args: &[&str]) -> (i32, String, String) {
        let output = Command::new(ORBCODE_BIN)
            .args(args)
            .current_dir(self.cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", &self.home_path)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home_path)
            .env("ANTHROPIC_BASE_URL", base_url)
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

    fn run_in_dir(&self, dir: &Path, args: &[&str]) -> (i32, String, String) {
        let output = Command::new(ORBCODE_BIN)
            .args(args)
            .current_dir(dir)
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

    fn run_with_piped_stdin(&self, stdin_data: &[u8], args: &[&str]) -> (i32, String, String) {
        use std::io::Write;
        let mut child = Command::new(ORBCODE_BIN)
            .args(args)
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
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin_data)
            .expect("write stdin");
        let output = child.wait_with_output().expect("wait orbcode");
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

fn assert_envelope(record: &Value, expected_type: &str) {
    assert_eq!(
        record.get("type").and_then(Value::as_str),
        Some(expected_type),
        "expected type {expected_type}, got {record}"
    );
    assert!(
        record.get("uuid").and_then(Value::as_str).is_some(),
        "record missing uuid: {record}"
    );
    assert!(
        record.get("session_id").and_then(Value::as_str).is_some(),
        "record missing session_id: {record}"
    );
    if record.get("timestamp").is_some() {
        assert!(
            record.get("timestamp").and_then(Value::as_str).is_some(),
            "timestamp must be string: {record}"
        );
    }
}

fn unique_session_id(records: &[Value]) -> String {
    let mut ids: Vec<&str> = records
        .iter()
        .filter_map(|record| record.get("session_id").and_then(Value::as_str))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        1,
        "every record must share one session_id, found {ids:?}"
    );
    ids[0].to_string()
}

#[test]
fn stream_json_simple_text_turn_emits_sdk_compatible_envelopes() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    assert!(records.len() >= 4, "got only {} records", records.len());

    let init = &records[0];
    assert_envelope(init, "system");
    assert_eq!(init["subtype"], "init");
    assert!(init["tools"].is_array(), "tools must be array");
    assert!(init["mcp_servers"].is_array());
    assert!(init["model"].is_string());
    assert_eq!(init["permissionMode"], "default");
    assert_eq!(init["apiKeySource"], "user");
    assert!(
        init["claude_code_version"].is_string(),
        "init must carry claude_code_version"
    );

    let result = records.last().expect("last record");
    assert_envelope(result, "result");
    assert_eq!(result["subtype"], "success");
    assert_eq!(result["is_error"], false);
    assert!(!result["result"].as_str().unwrap_or("").is_empty());
    assert_eq!(result["stop_reason"], "end_turn");
    let usage = &result["usage"];
    assert!(usage["input_tokens"].as_u64().is_some());
    assert!(usage["output_tokens"].as_u64().is_some());
    let model_usage = result["modelUsage"].as_object().expect("modelUsage object");
    assert!(!model_usage.is_empty(), "modelUsage must be populated");
    for (_model, entry) in model_usage {
        assert!(entry["inputTokens"].as_u64().is_some());
        assert!(entry["outputTokens"].as_u64().is_some());
        assert!(
            entry["costUSD"].is_number(),
            "modelUsage entry must have costUSD"
        );
    }
    assert!(result["permission_denials"].is_array());
    assert!(
        result["pricing_known"].is_boolean(),
        "result must carry pricing_known boolean"
    );
    assert!(
        result["total_cost_usd"].is_number(),
        "result must carry total_cost_usd number"
    );

    let assistant = records
        .iter()
        .find(|record| record["type"] == "assistant")
        .expect("must emit at least one assistant record");
    assert_envelope(assistant, "assistant");
    let message = &assistant["message"];
    assert_eq!(message["role"], "assistant");
    assert_eq!(message["type"], "message");
    assert!(message["content"].is_array());
    assert!(message["usage"].is_object());

    let session_id = unique_session_id(&records);
    assert_eq!(init["session_id"].as_str().unwrap(), session_id);

    let stream_events = records
        .iter()
        .filter(|record| record["type"] == "stream_event")
        .count();
    assert!(
        stream_events > 0,
        "must emit content_block_delta stream_events"
    );
}

#[test]
fn stream_json_model_error_produces_error_result_and_exit_1() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run_with_base_url(
        "mock://anthropic?scenario=fatal",
        &["-p", "boom", "--output-format", "stream-json", "--verbose"],
    );
    assert_eq!(code, 1, "expected exit 1 for model error; stderr: {stderr}");
    let records = parse_lines(&stdout);

    let result = records.last().expect("last record");
    assert_envelope(result, "result");
    assert_eq!(result["is_error"], true);
    assert_eq!(result["subtype"], "error_during_execution");
    assert!(result["errors"].is_array());
    assert!(
        !result["errors"].as_array().unwrap().is_empty(),
        "errors should include the failure message"
    );

    let has_error_stream_event = records
        .iter()
        .any(|record| record["type"] == "stream_event" && record["event"]["type"] == "error");
    assert!(
        has_error_stream_event,
        "expected an error stream_event before result"
    );

    assert!(
        !records.iter().any(|record| record["type"] == "assistant"),
        "no assistant record should be emitted for an interrupted model error"
    );
}

#[test]
fn stream_json_tool_use_round_trip_emits_assistant_tool_use_and_user_tool_result() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo hi\"}",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "acceptEdits",
        "--allowed-tools",
        "bash",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);

    let assistant_with_tool_use = records.iter().any(|record| {
        record["type"] == "assistant"
            && record["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_use"))
    });
    assert!(
        assistant_with_tool_use,
        "expected an assistant record carrying a tool_use content block"
    );

    let user_with_tool_result = records.iter().any(|record| {
        record["type"] == "user"
            && record["message"]["content"]
                .as_array()
                .is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "tool_result" && block["tool_use_id"].is_string()
                    })
                })
    });
    assert!(
        user_with_tool_result,
        "expected a user record carrying tool_result for the bash invocation"
    );

    let result = records.last().expect("result");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "success");
    assert!(result["permission_denials"].as_array().unwrap().is_empty());
}

#[test]
fn stream_json_first_line_is_system_init_and_session_id_is_stable() {
    let harness = Harness::new();
    let (code, stdout, stderr) =
        harness.run(&["-p", "hi", "--output-format", "stream-json", "--verbose"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);
    assert_eq!(records[0]["type"], "system");
    assert_eq!(records[0]["subtype"], "init");
    let init_session = records[0]["session_id"].as_str().expect("init session_id");
    let last = records.last().unwrap();
    assert_eq!(last["type"], "result");
    assert_eq!(last["session_id"].as_str().unwrap(), init_session);
    for record in &records {
        assert_eq!(
            record["session_id"].as_str().unwrap(),
            init_session,
            "every record must share the same session_id"
        );
    }
}

#[test]
fn stream_json_diagnostics_go_to_stderr_not_stdout() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code, 0);
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|e| panic!("stdout line must be JSON: {e}: {line}"));
    }
    let _ = stderr;
}

#[test]
fn stream_json_requires_verbose_for_stream_output() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&["-p", "hi", "--output-format", "stream-json"]);
    // Invalid argument combinations are a pre-flight failure with the dedicated
    // InvalidCliInput exit code (2); the diagnostic goes to stderr and stdout
    // stays empty so machine consumers never see a malformed record.
    assert_eq!(
        code, 2,
        "expected the InvalidCliInput exit code (2) when --verbose missing; stderr: {stderr}"
    );
    assert!(
        stderr.contains("requires --verbose"),
        "stderr should mention --verbose requirement, got: {stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "invalid CLI input must not emit JSON on stdout, got: {stdout}"
    );
}

#[test]
fn stream_json_permission_denied_exits_4_with_error_subtype() {
    let harness = Harness::new();
    // Drive a tool the headless loop will deny: bash requires permission and is
    // not on the allow-list (default permission mode), so the request is denied,
    // the turn still completes, and the run is classified PermissionDenied.
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "#tool:bash {\"command\":\"echo hi\"}",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(
        code, 4,
        "a denied tool call should exit with the PermissionDenied code (4); stderr: {stderr}\nstdout:\n{stdout}"
    );
    let records = parse_lines(&stdout);

    let denied = records.iter().any(|record| {
        record["type"] == "stream_event"
            && record["event"]["type"] == "tool_use_completed"
            && record["event"]["kind"] == "permission_denied"
    });
    assert!(
        denied,
        "expected a tool_use_completed stream_event classified permission_denied"
    );

    let result = records.last().expect("result record");
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], true);
    assert_eq!(result["subtype"], "error_during_execution");
    assert!(
        !result["permission_denials"].as_array().unwrap().is_empty(),
        "result must record the permission denial"
    );
    assert!(
        !result["errors"].as_array().unwrap().is_empty(),
        "result must carry an error summary for the denial"
    );
}

#[test]
fn orbcode_binary_exists_at_expected_path() {
    assert!(
        Path::new(ORBCODE_BIN).exists(),
        "Cargo did not stage orbcode binary at {ORBCODE_BIN}"
    );
}

#[test]
fn stream_json_stdout_is_pure_json_on_model_error() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run_with_base_url(
        "mock://anthropic?scenario=fatal",
        &["-p", "boom", "--output-format", "stream-json", "--verbose"],
    );
    assert_eq!(code, 1, "stderr: {stderr}");
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|e| panic!("stdout must be pure JSON even on error: {e}: {line}"));
    }
    let records = parse_lines(&stdout);
    let has_error = records
        .iter()
        .any(|r| r["type"] == "stream_event" && r["event"]["type"] == "error");
    assert!(
        has_error,
        "error should be carried as a stream_event record on stdout"
    );
    let _ = stderr;
}

#[test]
fn text_mode_error_diagnostics_go_to_stderr() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run_with_base_url(
        "mock://anthropic?scenario=fatal",
        &["-p", "boom", "--output-format", "text"],
    );
    assert_ne!(code, 0, "fatal error should produce non-zero exit");
    assert!(
        stderr.contains("error"),
        "stderr should contain the error diagnostic, got: {stderr}"
    );
    for line in stdout.lines() {
        assert!(
            !line.starts_with("error:"),
            "error prefix should not leak to stdout in text mode: {line}"
        );
    }
}

#[test]
fn continue_prefers_session_matching_cwd() {
    let harness = Harness::new();
    let dir_a = tempfile::tempdir().expect("dir_a");
    let dir_b = tempfile::tempdir().expect("dir_b");

    let (code_a, stdout_a, stderr_a) = harness.run_in_dir(
        dir_a.path(),
        &[
            "-p",
            "say hello",
            "--output-format",
            "stream-json",
            "--verbose",
        ],
    );
    assert_eq!(code_a, 0, "stderr: {stderr_a}");
    let records_a = parse_lines(&stdout_a);
    let session_id_a = records_a[0]["session_id"]
        .as_str()
        .expect("init session_id")
        .to_string();

    let (code_b, stdout_b, stderr_b) = harness.run_in_dir(
        dir_b.path(),
        &[
            "-p",
            "say goodbye",
            "--output-format",
            "stream-json",
            "--verbose",
        ],
    );
    assert_eq!(code_b, 0, "stderr: {stderr_b}");
    let records_b = parse_lines(&stdout_b);
    let session_id_b = records_b[0]["session_id"]
        .as_str()
        .expect("init session_id")
        .to_string();

    assert_ne!(
        session_id_a, session_id_b,
        "sessions from different cwds must have distinct ids"
    );

    let (code_c, stdout_c, stderr_c) = harness.run_in_dir(
        dir_a.path(),
        &[
            "-p",
            "--continue",
            "continue",
            "--output-format",
            "stream-json",
            "--verbose",
        ],
    );
    assert_eq!(code_c, 0, "stderr: {stderr_c}");
    let records_c = parse_lines(&stdout_c);
    let resumed_id = records_c[0]["session_id"]
        .as_str()
        .expect("resumed session_id");
    assert_eq!(
        resumed_id, session_id_a,
        "--continue from dir_a should resume session_a, not session_b"
    );
}

#[test]
fn add_dir_in_headless_mode_bootstraps_successfully() {
    let harness = Harness::new();
    let extra_dir = tempfile::tempdir().expect("extra dir");
    let extra_path = extra_dir.path().to_str().expect("extra path");
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--add-dir",
        extra_path,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    assert_eq!(records[0]["type"], "system");
    assert_eq!(records[0]["subtype"], "init");
    let result = records.last().expect("result");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "success");
}

#[test]
fn piped_stdin_is_used_as_prompt_in_print_mode() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run_with_piped_stdin(
        b"say hello world",
        &["-p", "--output-format", "stream-json", "--verbose"],
    );
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    assert!(records.len() >= 3, "expected init + events + result");
    assert_eq!(records[0]["type"], "system");
    assert_eq!(records[0]["subtype"], "init");
    let result = records.last().expect("result");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "success");
}

#[test]
fn stream_json_records_carry_monotonic_sequence() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);
    let mut prev: Option<u64> = None;
    for record in &records {
        if let Some(seq) = record.get("sequence").and_then(Value::as_u64) {
            if let Some(p) = prev {
                assert!(
                    seq > p,
                    "sequence must be strictly increasing: {p} -> {seq}"
                );
            }
            prev = Some(seq);
        }
    }
    assert!(
        prev.is_some(),
        "at least one record should carry a sequence number"
    );
    assert_eq!(
        records[0]["sequence"].as_u64(),
        Some(0),
        "first record (system/init) must have sequence 0"
    );
}
