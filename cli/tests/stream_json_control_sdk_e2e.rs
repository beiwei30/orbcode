//! End-to-end coverage for the typed SDK control extensions.
//!
//! Each test spawns the real `orbcode` binary with `--input-format stream-json`
//! and feeds NDJSON control frames on stdin to verify the bidirectional contract.

use std::io::{BufRead, BufReader, Read, Write};
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

    fn command(&self) -> Command {
        let mut command = Command::new(ORBCODE_BIN);
        command
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
            .stderr(Stdio::piped());
        command
    }

    fn run_eof(&self, stdin: &str) -> (i32, String, String) {
        let mut child = self.command().spawn().expect("spawn orbcode");
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

    fn write_mcp_config_with_secrets(&self) {
        std::fs::write(
            self.cwd.path().join(".mcp.json"),
            r#"{
                "mcpServers": {
                    "secret-demo": {
                        "type": "stdio",
                        "command": "orbcode-definitely-missing-mcp-command",
                        "args": ["ARG_SECRET_CANARY_9281"],
                        "env": {"TOKEN": "ENV_SECRET_CANARY_1742"},
                        "headers": {"Authorization": "HEADER_SECRET_CANARY_6630"}
                    }
                }
            }"#,
        )
        .expect("write mcp config");
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
fn initialize_is_correlated_idempotent_and_does_not_repeat_system_init() {
    let harness = Harness::new();
    let first = control_frame("init-1", serde_json::json!({"subtype": "initialize"}));
    let second = control_frame("init-2", serde_json::json!({"subtype": "initialize"}));
    let stdin = format!("{first}\n{second}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let first = control_response(&records, "init-1").expect("first initialize response");
    let second = control_response(&records, "init-2").expect("second initialize response");
    assert_eq!(first["response"]["subtype"], "success");
    assert_eq!(second["response"]["subtype"], "success");
    assert_eq!(
        first["response"]["response"]["session_id"],
        second["response"]["response"]["session_id"]
    );
    assert!(
        first["response"]["response"]["supported_controls"]
            .as_array()
            .expect("supported controls")
            .iter()
            .any(|value| value == "can_use_tool")
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["type"] == "system" && record["subtype"] == "init")
            .count(),
        1,
        "initialize controls must not synthesize another session init"
    );
}

#[test]
fn mcp_status_and_initialize_redact_mutation_secrets() {
    let harness = Harness::new();
    harness.write_mcp_config_with_secrets();
    let initialize = control_frame("init-secret", serde_json::json!({"subtype": "initialize"}));
    let status = control_frame("mcp-secret", serde_json::json!({"subtype": "mcp_status"}));
    let stdin = format!("{initialize}\n{status}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    for secret in [
        "ARG_SECRET_CANARY_9281",
        "ENV_SECRET_CANARY_1742",
        "HEADER_SECRET_CANARY_6630",
    ] {
        assert!(!stdout.contains(secret), "stdout leaked {secret}: {stdout}");
    }
    let records = parse_lines(&stdout);
    for request_id in ["init-secret", "mcp-secret"] {
        let response = control_response(&records, request_id).expect("control response");
        assert_eq!(response["response"]["subtype"], "success");
        let servers = response["response"]["response"]["mcpServers"]
            .as_array()
            .expect("mcpServers");
        assert!(
            servers.iter().any(|server| server["name"] == "secret-demo"),
            "{response:?}"
        );
    }
}

#[test]
fn set_model_changes_authoritative_session_state() {
    let harness = Harness::new();
    let set = control_frame(
        "model-1",
        serde_json::json!({"subtype": "set_model", "model": "claude-haiku-4-5-20251001"}),
    );
    let state = control_frame(
        "model-state",
        serde_json::json!({"subtype": "get_session_state"}),
    );
    let stdin = format!("{set}\n{state}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    let set = control_response(&records, "model-1").expect("model response");
    assert_eq!(set["response"]["subtype"], "success");
    assert_eq!(
        set["response"]["response"]["model"],
        "claude-haiku-4-5-20251001"
    );
    let state = control_response(&records, "model-state").expect("state response");
    assert_eq!(
        state["response"]["response"]["model_name"],
        "claude-haiku-4-5-20251001"
    );
}

#[test]
fn rejected_model_change_preserves_the_previous_effective_model() {
    let harness = Harness::new();
    let valid_model = "claude-haiku-4-5-20251001";
    let set = control_frame(
        "model-valid",
        serde_json::json!({"subtype": "set_model", "model": valid_model}),
    );
    let invalid = control_frame(
        "model-invalid",
        serde_json::json!({"subtype": "set_model", "model": "bad\nmodel"}),
    );
    let state = control_frame(
        "model-after-invalid",
        serde_json::json!({"subtype": "get_session_state"}),
    );
    let stdin = format!("{set}\n{invalid}\n{state}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    assert_eq!(
        control_response(&records, "model-invalid").expect("invalid response")["response"]["subtype"],
        "error"
    );
    assert_eq!(
        control_response(&records, "model-after-invalid").expect("state response")["response"]["response"]
            ["model_name"],
        valid_model
    );
}

#[test]
fn set_model_is_safe_during_a_turn_and_applies_to_next_request() {
    let harness = Harness::new();
    let mode = control_frame(
        "model-mode",
        serde_json::json!({"subtype": "set_permission_mode", "mode": "bypassPermissions"}),
    );
    let first_prompt = user_frame(r#"#tool:bash {"command":"sleep 0.1"}"#);
    let set = control_frame(
        "model-mid",
        serde_json::json!({"subtype": "set_model", "model": "claude-haiku-4-5-20251001"}),
    );
    let second_prompt = user_frame("second turn");
    let stdin = format!("{mode}\n{first_prompt}\n{set}\n{second_prompt}\n");
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    let response = control_response(&records, "model-mid").expect("mid-turn model response");
    assert_eq!(response["response"]["subtype"], "success");
    assert_eq!(
        response["response"]["response"]["model"],
        "claude-haiku-4-5-20251001"
    );
    assert!(
        records
            .iter()
            .any(|record| record["type"] == "result" && record["num_turns"] == 2),
        "both turns must complete: {stdout}"
    );
}

#[test]
fn control_output_order_is_locked_between_assistant_records_and_at_terminal() {
    let harness = Harness::new();
    let mut child = harness.command().spawn().expect("spawn orbcode");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut stderr = child.stderr.take().expect("stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut output = String::new();
        stderr.read_to_string(&mut output).expect("read stderr");
        output
    });

    writeln!(
        stdin,
        "{}",
        control_frame(
            "order-mode",
            serde_json::json!({"subtype": "set_permission_mode", "mode": "bypassPermissions"}),
        )
    )
    .expect("write mode");
    writeln!(
        stdin,
        "{}",
        user_frame(r#"#tool:bash {"command":"sleep 0.2"}"#)
    )
    .expect("write prompt");
    stdin.flush().expect("flush prompt");

    let mut captured = String::new();
    let mut assistant_count = 0;
    while assistant_count < 1 {
        let mut line = String::new();
        assert_ne!(stdout.read_line(&mut line).expect("read stdout"), 0);
        let record: Value = serde_json::from_str(line.trim()).expect("record JSON");
        assistant_count += usize::from(record["type"] == "assistant");
        captured.push_str(&line);
    }
    writeln!(
        stdin,
        "{}",
        control_frame(
            "order-mid",
            serde_json::json!({"subtype": "get_session_state"}),
        )
    )
    .expect("write mid-turn control");
    stdin.flush().expect("flush mid-turn control");

    while assistant_count < 2 {
        let mut line = String::new();
        assert_ne!(stdout.read_line(&mut line).expect("read stdout"), 0);
        let record: Value = serde_json::from_str(line.trim()).expect("record JSON");
        assistant_count += usize::from(record["type"] == "assistant");
        captured.push_str(&line);
    }
    writeln!(
        stdin,
        "{}",
        control_frame(
            "order-terminal",
            serde_json::json!({"subtype": "get_session_state"}),
        )
    )
    .expect("write terminal-adjacent control");
    stdin.flush().expect("flush terminal-adjacent control");
    drop(stdin);
    stdout
        .read_to_string(&mut captured)
        .expect("remaining stdout");
    let status = child.wait().expect("wait");
    let stderr = stderr_reader.join().expect("stderr reader");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr: {stderr}\nstdout:\n{captured}"
    );

    let records = parse_lines(&captured);
    let assistant_positions = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| (record["type"] == "assistant").then_some(index))
        .collect::<Vec<_>>();
    let mid = records
        .iter()
        .position(|record| {
            record["type"] == "control_response" && record["response"]["request_id"] == "order-mid"
        })
        .expect("mid-turn response");
    assert!(
        assistant_positions[0] < mid && mid < assistant_positions[1],
        "mid-turn response must stay between assistant records: {captured}"
    );
    let terminal = records
        .iter()
        .position(|record| {
            record["type"] == "control_response"
                && record["response"]["request_id"] == "order-terminal"
        })
        .expect("terminal-adjacent response");
    let result = records
        .iter()
        .position(|record| record["type"] == "result")
        .expect("result");
    assert_eq!(
        terminal + 1,
        result,
        "terminal-adjacent response must immediately precede result: {captured}"
    );
}

#[test]
fn seed_read_state_validates_the_current_file_identity() {
    let harness = Harness::new();
    let file = harness.cwd.path().join("seeded.txt");
    std::fs::write(&file, "alpha\n").expect("seed file");
    let mtime = std::fs::metadata(&file)
        .expect("metadata")
        .modified()
        .expect("mtime")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after epoch")
        .as_millis() as u64;
    let seed = control_frame(
        "seed-1",
        serde_json::json!({"subtype": "seed_read_state", "path": "seeded.txt", "mtime": mtime}),
    );
    let mode = control_frame(
        "seed-mode",
        serde_json::json!({"subtype": "set_permission_mode", "mode": "bypassPermissions"}),
    );
    let edit = user_frame(
        r#"#tool:file-edit {"file_path":"seeded.txt","old_string":"alpha","new_string":"omega"}"#,
    );
    let stdin = format!("{mode}\n{seed}\n{edit}\n");
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    let response = control_response(&records, "seed-1").expect("seed response");
    assert_eq!(response["response"]["subtype"], "success");
    assert_eq!(response["response"]["response"]["mtime"], mtime);
    assert_eq!(response["response"]["response"]["seeded"], true);
    assert_eq!(
        std::fs::read_to_string(file).expect("edited file"),
        "omega\n"
    );
}

#[test]
fn seed_read_state_rejects_mismatch_missing_and_outside_workspace() {
    let harness = Harness::new();
    let inside = harness.cwd.path().join("inside.txt");
    let outside = harness.home_path.join("outside.txt");
    std::fs::write(&inside, "inside").expect("inside");
    std::fs::write(&outside, "outside").expect("outside");
    let mtime = |path: &std::path::Path| {
        std::fs::metadata(path)
            .expect("metadata")
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after epoch")
            .as_millis() as u64
    };
    let mismatch = control_frame(
        "seed-mismatch",
        serde_json::json!({"subtype": "seed_read_state", "path": "inside.txt", "mtime": mtime(&inside) + 1}),
    );
    let missing = control_frame(
        "seed-missing",
        serde_json::json!({"subtype": "seed_read_state", "path": "missing.txt", "mtime": 0}),
    );
    let outside_frame = control_frame(
        "seed-outside",
        serde_json::json!({"subtype": "seed_read_state", "path": outside.display().to_string(), "mtime": mtime(&outside)}),
    );
    let stdin = format!(
        "{mismatch}\n{missing}\n{outside_frame}\n{}\n",
        user_frame("say hi")
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    for request_id in ["seed-mismatch", "seed-missing", "seed-outside"] {
        let response = control_response(&records, request_id).expect("seed error response");
        assert_eq!(response["response"]["subtype"], "error", "{response:?}");
    }
}

#[test]
fn rewind_files_is_explicitly_not_transcript_rewind() {
    let harness = Harness::new();
    let rewind = control_frame(
        "rewind-1",
        serde_json::json!({"subtype": "rewind_files", "user_message_id": "msg-1", "dry_run": false}),
    );
    let stdin = format!("{rewind}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    let response = control_response(&records, "rewind-1").expect("rewind response");
    assert_eq!(response["response"]["subtype"], "error");
    let error = response["response"]["error"].as_str().expect("error");
    assert!(error.contains("no file checkpoint contract"), "{error}");
    assert!(error.contains("transcript rewind"), "{error}");
}

#[test]
fn cancel_async_message_reports_not_found_without_interrupting_turns() {
    let harness = Harness::new();
    let cancel = control_frame(
        "cancel-1",
        serde_json::json!({"subtype": "cancel_async_message", "message_uuid": "missing-task"}),
    );
    let stdin = format!("{cancel}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    let response = control_response(&records, "cancel-1").expect("cancel response");
    assert_eq!(response["response"]["subtype"], "success");
    assert_eq!(response["response"]["response"]["outcome"], "not_found");
    assert_eq!(response["response"]["response"]["cancelled"], false);
}

#[test]
fn cancel_async_message_signals_one_owned_background_job() {
    let harness = Harness::new();
    let mut child = Command::new(ORBCODE_BIN)
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .current_dir(harness.cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", &harness.home_path)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &harness.home_path)
        .env("ANTHROPIC_BASE_URL", "stub://test")
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn orbcode");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut stderr = child.stderr.take().expect("stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut output = String::new();
        stderr.read_to_string(&mut output).expect("read stderr");
        output
    });
    let mut captured = String::new();
    let mut init_line = String::new();
    stdout.read_line(&mut init_line).expect("read init");
    captured.push_str(&init_line);
    let init: Value = serde_json::from_str(init_line.trim()).expect("init JSON");
    let session_id = init["session_id"].as_str().expect("session id");

    let task_id = "owned-background-job";
    let jobs = harness.home_path.join("background/jobs");
    let logs = harness.home_path.join("background/logs");
    std::fs::create_dir_all(&jobs).expect("jobs dir");
    std::fs::create_dir_all(&logs).expect("logs dir");
    let log_path = logs.join(format!("{task_id}.log"));
    std::fs::write(&log_path, "").expect("log");
    let record_path = jobs.join(format!("{task_id}.json"));
    std::fs::write(
        &record_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "job_id": task_id,
            "session_id": session_id,
            "prompt": "background work",
            "cwd": harness.cwd.path().display().to_string(),
            "provider": "anthropic",
            "fallback_provider": null,
            "model": "claude-sonnet-4-6",
            "permission_mode": null,
            "status": "queued",
            "created_at": "2026-08-05T00:00:00Z",
            "updated_at": "2026-08-05T00:00:00Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "log_path": log_path.display().to_string(),
            "error": null,
            "exit_code": null,
            "signal": null,
            "last_log_offset": 0,
            "cancellation_reason": null
        }))
        .expect("record JSON"),
    )
    .expect("write record");

    let cancel = control_frame(
        "cancel-owned",
        serde_json::json!({"subtype": "cancel_async_message", "message_uuid": task_id}),
    );
    writeln!(stdin, "{cancel}").expect("write cancel");
    stdin.flush().expect("flush cancel");
    drop(stdin);
    stdout
        .read_to_string(&mut captured)
        .expect("remaining stdout");
    let status = child.wait().expect("wait");
    let stderr = stderr_reader.join().expect("stderr reader");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr: {stderr}\nstdout:\n{captured}"
    );
    let records = parse_lines(&captured);
    let response = control_response(&records, "cancel-owned").expect("cancel response");
    assert_eq!(response["response"]["subtype"], "success");
    assert_eq!(response["response"]["response"]["outcome"], "signalled");
    assert_eq!(response["response"]["response"]["cancelled"], true);
    let persisted: Value = serde_json::from_slice(&std::fs::read(record_path).expect("record"))
        .expect("persisted JSON");
    assert_eq!(persisted["status"], "cancelled");
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
fn set_max_thinking_tokens_number_is_applied() {
    let harness = Harness::new();
    let ctrl = control_frame(
        "mt-1",
        serde_json::json!({"subtype": "set_max_thinking_tokens", "max_thinking_tokens": 4096}),
    );
    let context = control_frame(
        "mt-context",
        serde_json::json!({"subtype": "get_context_usage"}),
    );
    let stdin = format!("{ctrl}\n{context}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let response = control_response(&records, "mt-1").expect("set_max_thinking_tokens response");
    assert_eq!(response["response"]["subtype"], "success");
    assert_eq!(
        response["response"]["response"]["max_thinking_tokens"],
        4096
    );
    let context = control_response(&records, "mt-context").expect("context response");
    assert_eq!(context["response"]["response"]["max_thinking_tokens"], 4096);
}

#[test]
fn set_max_thinking_tokens_null_clears_override() {
    let harness = Harness::new();
    let set = control_frame(
        "mt-set",
        serde_json::json!({"subtype": "set_max_thinking_tokens", "max_thinking_tokens": 4096}),
    );
    let clear = control_frame(
        "mt-2",
        serde_json::json!({"subtype": "set_max_thinking_tokens", "max_thinking_tokens": null}),
    );
    let state = control_frame(
        "mt-state",
        serde_json::json!({"subtype": "get_session_state"}),
    );
    let stdin = format!("{set}\n{clear}\n{state}\n{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let response = control_response(&records, "mt-2").expect("set_max_thinking_tokens response");
    assert_eq!(response["response"]["subtype"], "success");
    assert!(
        response["response"]["response"]["max_thinking_tokens"].is_null(),
        "{response:?}"
    );
    let state = control_response(&records, "mt-state").expect("session state response");
    assert!(state["response"]["response"]["max_thinking_tokens"].is_null());
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
