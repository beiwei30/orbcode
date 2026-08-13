//! End-to-end coverage for the headless SDK control channel:
//! `orbcode -p --input-format stream-json`. Each test spawns the real binary and
//! feeds NDJSON control frames on stdin (`user` prompts, `interrupt`,
//! `set_permission_mode`, unsupported subtypes, malformed lines) to verify the
//! bidirectional contract end-to-end:
//!   - incremental `user` prompts run as turns,
//!   - `set_permission_mode` escalates so a tool runs (exit 0) where the same
//!     prompt is otherwise denied (exit 4),
//!   - `interrupt` cancels the active turn (exit 5),
//!   - unsupported subtypes / schema errors yield a structured `control_response`
//!     error and never crash the process.
//!
//! The provider is the deterministic `stub://test` backend; `#tool:bash {...}`
//! markers drive a tool round-trip just as in `stream_json_e2e.rs`.

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

    /// Feed `stdin` then close it (EOF). EOF ends the run once buffered frames
    /// are drained — the path most control flows take.
    fn run_eof(&self, stdin: &str) -> (i32, String, String) {
        let mut child = self.command().spawn().expect("spawn orbcode");
        {
            let mut pipe = child.stdin.take().expect("stdin pipe");
            pipe.write_all(stdin.as_bytes()).expect("write stdin");
            pipe.flush().expect("flush stdin");
            // pipe dropped here -> stdin EOF
        }
        let output = child.wait_with_output().expect("wait orbcode");
        decode(output)
    }

    /// Feed `stdin` but keep the pipe open for the lifetime of the child. With no
    /// EOF, a held permission can only be resolved by an `interrupt`, so the run
    /// ends via cancellation rather than the EOF-deny fallback — making the
    /// interrupt path deterministic.
    fn run_keep_open(&self, stdin: &str) -> (i32, String, String) {
        let mut child = self.command().spawn().expect("spawn orbcode");
        let mut pipe = child.stdin.take().expect("stdin pipe");
        pipe.write_all(stdin.as_bytes()).expect("write stdin");
        pipe.flush().expect("flush stdin");
        // Hold `pipe` open across wait: the child exits on cancellation, its
        // stdout closes, and wait_with_output returns even though stdin is open.
        let output = child.wait_with_output().expect("wait orbcode");
        drop(pipe);
        decode(output)
    }
}

fn decode(output: std::process::Output) -> (i32, String, String) {
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    (code, stdout, stderr)
}

/// Build a stream-json `user` frame carrying `text` as the incremental prompt.
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
        .map(|line| serde_json::from_str::<Value>(line).expect("each stream-json line is JSON"))
        .collect()
}

/// Find the first `control_response` whose nested `request_id` matches.
fn control_response<'a>(records: &'a [Value], request_id: &str) -> Option<&'a Value> {
    records.iter().find(|record| {
        record["type"] == "control_response"
            && record["response"]["request_id"].as_str() == Some(request_id)
    })
}

const BASH_PROMPT: &str =
    "#tool:bash {\"command\":\"echo hi\",\"sandbox_permissions\":\"require_escalated\"}";

#[test]
fn incremental_user_prompt_runs_a_turn_and_exits_0() {
    let harness = Harness::new();
    let stdin = format!("{}\n", user_frame("say hi"));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    assert_eq!(records[0]["type"], "system");
    assert_eq!(records[0]["subtype"], "init");

    let result = records.last().expect("result record");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "success");
    assert_eq!(result["is_error"], false);
    assert!(
        records.iter().any(|record| record["type"] == "assistant"),
        "an incremental user prompt must drive at least one assistant record"
    );
}

#[test]
fn tool_without_grant_is_denied_and_exits_4() {
    // Baseline for the set_permission_mode test below: in the default mode a
    // sandbox escalation is held while stdin is open, then denied on EOF.
    let harness = Harness::new();
    let stdin = format!("{}\n", user_frame(BASH_PROMPT));
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(
        code, 4,
        "a denied tool call exits PermissionDenied (4); stderr: {stderr}\nstdout:\n{stdout}"
    );

    let records = parse_lines(&stdout);
    let result = records.last().expect("result record");
    assert_eq!(result["subtype"], "error_during_execution");
    assert!(
        !result["permission_denials"].as_array().unwrap().is_empty(),
        "result must record the denial"
    );
}

#[test]
fn set_permission_mode_grants_tool_and_exits_0() {
    // Same bash prompt as the baseline, but a leading set_permission_mode escalates
    // to bypassPermissions so the tool runs unprompted: exit 0 instead of 4.
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        r#"{"type":"control_request","request_id":"mode-1","request":{"subtype":"set_permission_mode","mode":"bypassPermissions"}}"#,
        user_frame(BASH_PROMPT),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(
        code, 0,
        "set_permission_mode bypassPermissions should let the tool run; stderr: {stderr}\nstdout:\n{stdout}"
    );

    let records = parse_lines(&stdout);
    let ack =
        control_response(&records, "mode-1").expect("control_response for set_permission_mode");
    assert_eq!(ack["response"]["subtype"], "success");

    let result = records.last().expect("result record");
    assert_eq!(result["subtype"], "success");
    assert!(
        result["permission_denials"].as_array().unwrap().is_empty(),
        "no denial should be recorded once the mode grants the tool"
    );
    let ran_tool = records.iter().any(|record| {
        record["type"] == "user"
            && record["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
    });
    assert!(ran_tool, "expected a tool_result proving the bash call ran");
}

#[test]
fn interrupt_cancels_active_turn_and_exits_5() {
    // The bash call parks the turn at the permission await (held, not answered,
    // because stdin stays open). The buffered interrupt frame cancels it.
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        user_frame(BASH_PROMPT),
        r#"{"type":"control_request","request_id":"int-1","request":{"subtype":"interrupt"}}"#,
    );
    let (code, stdout, stderr) = harness.run_keep_open(&stdin);
    assert_eq!(
        code, 5,
        "interrupt should cancel the turn (Cancelled = 5); stderr: {stderr}\nstdout:\n{stdout}"
    );

    let records = parse_lines(&stdout);
    let ack = control_response(&records, "int-1").expect("control_response for interrupt");
    assert_eq!(ack["response"]["subtype"], "success");

    let result = records.last().expect("result record");
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], true);
    assert_eq!(result["subtype"], "error_during_execution");
}

#[test]
fn unsupported_subtype_yields_control_response_error_and_run_continues() {
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        r#"{"type":"control_request","request_id":"uns-1","request":{"subtype":"frobnicate"}}"#,
        user_frame("say hi"),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(
        code, 0,
        "an unsupported control request must not abort the run; stderr: {stderr}\nstdout:\n{stdout}"
    );

    let records = parse_lines(&stdout);
    let err =
        control_response(&records, "uns-1").expect("control_response for unsupported subtype");
    assert_eq!(err["response"]["subtype"], "error");
    assert!(
        err["response"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("unsupported"),
        "error message should name the failure: {err}"
    );

    let result = records.last().expect("result record");
    assert_eq!(result["subtype"], "success");
}

#[test]
fn invalid_permission_mode_yields_error_correlated_by_request_id() {
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        r#"{"type":"control_request","request_id":"bad-1","request":{"subtype":"set_permission_mode","mode":"nonsense"}}"#,
        user_frame("say hi"),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let err = control_response(&records, "bad-1").expect("control_response for invalid mode");
    assert_eq!(err["response"]["subtype"], "error");
}

#[test]
fn malformed_json_does_not_crash_and_run_continues() {
    // A truncated line has no recoverable request_id, so the diagnostic goes to
    // stderr; the process must keep going and run the following valid prompt.
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        r#"{"type":"control_request","request_id":"#, // truncated, invalid JSON
        user_frame("say hi"),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(
        code, 0,
        "malformed input must not crash the process; stderr: {stderr}\nstdout:\n{stdout}"
    );

    let records = parse_lines(&stdout);
    let result = records.last().expect("result record");
    assert_eq!(result["subtype"], "success");
    assert!(
        records.iter().any(|record| record["type"] == "assistant"),
        "the valid prompt after the malformed line must still run"
    );
    assert!(
        stderr.contains("warning"),
        "an uncorrelated parse error should be reported on stderr: {stderr}"
    );
}

#[test]
fn missing_type_with_request_id_yields_control_response_error() {
    // A schema-invalid frame that still carries a request_id is reported as a
    // correlated control_response error rather than a stderr-only warning.
    let harness = Harness::new();
    let stdin = format!(
        "{}\n{}\n",
        r#"{"request_id":"miss-1"}"#,
        user_frame("say hi"),
    );
    let (code, stdout, stderr) = harness.run_eof(&stdin);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let err = control_response(&records, "miss-1").expect("control_response for missing type");
    assert_eq!(err["response"]["subtype"], "error");
}

#[test]
fn ask_user_duplex_rejects_invalid_then_accepts_valid_response_exactly_once() {
    let harness = Harness::new();
    let mut child = harness.command().spawn().expect("spawn orbcode");
    let mut stdin = Some(child.stdin.take().expect("stdin pipe"));
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut stderr = child.stderr.take().expect("stderr pipe");

    let prompt = r#"#tool:AskUserQuestion {"questions":[{"id":"database","question":"Which database?","header":"Database","options":[{"id":"postgres","label":"PostgreSQL","description":"SQL"}],"allow_free_text":false}]}"#;
    writeln!(stdin.as_mut().unwrap(), "{}", user_frame(prompt)).expect("write user frame");
    stdin.as_mut().unwrap().flush().expect("flush user frame");

    let mut records = Vec::new();
    let mut request_id = None;
    for line in BufReader::new(stdout).lines() {
        let line = line.expect("stdout line");
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(&line).expect("stream-json output");
        if record["type"] == "stream_event"
            && record["event"]["type"] == "server_request"
            && record["event"]["method"] == "ask_user/request"
        {
            let id = record["event"]["request_id"]
                .as_str()
                .expect("ask request id")
                .to_string();
            assert_eq!(record["event"]["params"]["questions"][0]["id"], "database");
            request_id = Some(id.clone());

            let invalid = serde_json::json!({
                "type": "server_response",
                "request_id": id,
                "response": {
                    "outcome": "answered",
                    "answers": {
                        "database": {"kind": "selected", "option_id": "unknown"}
                    },
                    "annotations": {}
                }
            });
            let valid = serde_json::json!({
                "type": "server_response",
                "request_id": record["event"]["request_id"],
                "response": {
                    "outcome": "answered",
                    "answers": {
                        "database": {"kind": "selected", "option_id": "postgres"}
                    },
                    "annotations": {}
                }
            });
            let pipe = stdin.as_mut().expect("stdin still open");
            writeln!(pipe, "{invalid}").expect("write invalid response");
            writeln!(pipe, "{valid}").expect("write valid response");
            writeln!(pipe, "{valid}").expect("write duplicate response");
            pipe.flush().expect("flush responses");
            drop(stdin.take());
        }
        let terminal = record["type"] == "result";
        records.push(record);
        if terminal {
            break;
        }
    }

    let status = child.wait().expect("wait orbcode");
    let mut stderr_text = String::new();
    stderr
        .read_to_string(&mut stderr_text)
        .expect("read stderr");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr: {stderr_text}\nrecords: {records:#?}"
    );

    let request_id = request_id.expect("server request lifecycle frame");
    let correlated: Vec<&Value> = records
        .iter()
        .filter(|record| {
            record["type"] == "control_response"
                && record["response"]["request_id"].as_str() == Some(request_id.as_str())
        })
        .collect();
    assert_eq!(
        correlated.len(),
        3,
        "invalid, valid, and duplicate acknowledgements"
    );
    assert_eq!(correlated[0]["response"]["subtype"], "error");
    assert!(
        correlated[0]["response"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown option")
    );
    assert_eq!(correlated[1]["response"]["subtype"], "success");
    assert_eq!(correlated[2]["response"]["subtype"], "error");
    assert!(records.iter().any(|record| {
        record["type"] == "stream_event"
            && record["event"]["type"] == "server_request_resolved"
            && record["event"]["request_id"] == request_id
    }));
    assert!(
        records.iter().any(|record| {
            let wire = record.to_string();
            wire.contains("User has answered your questions") && wire.contains("PostgreSQL")
        }),
        "validated labels and the provider-visible summary must return to the model"
    );
}

#[test]
fn ask_user_duplex_eof_cancels_pending_interaction_without_hanging() {
    let harness = Harness::new();
    let prompt = r#"#tool:AskUserQuestion {"questions":[{"id":"name","question":"What name?","header":"Name","options":[]}]}"#;
    let (code, stdout, stderr) = harness.run_eof(&format!("{}\n", user_frame(prompt)));
    assert_eq!(code, 5, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    assert!(records.iter().any(|record| {
        record["type"] == "stream_event"
            && record["event"]["type"] == "server_request"
            && record["event"]["method"] == "ask_user/request"
    }));
    assert!(records.iter().any(|record| {
        record["type"] == "stream_event"
            && record["event"]["type"] == "tool_use_completed"
            && record["event"]["kind"] == "cancelled"
    }));
}
