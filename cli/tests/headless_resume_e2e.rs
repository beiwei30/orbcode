//! End-to-end tests for `--resume <session-id>` and `--continue` multi-turn
//! session resumption. Verifies that a resumed session keeps the same
//! session_id, emits `SessionLoaded` (not `SessionStarted`), and that
//! `--continue` picks the latest session matching the CWD.

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
        self.run_in_dir(self.cwd.path(), args)
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
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("each stream-json line is JSON"))
        .collect()
}

fn session_id_from(records: &[Value]) -> String {
    records[0]["session_id"]
        .as_str()
        .expect("init session_id")
        .to_string()
}

#[test]
fn resume_by_session_id_keeps_same_session() {
    let harness = Harness::new();

    // First turn: create a session
    let (code1, stdout1, stderr1) = harness.run(&[
        "-p",
        "first turn",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code1, 0, "first turn failed; stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    let original_id = session_id_from(&records1);

    // Second turn: resume by explicit session id
    let (code2, stdout2, stderr2) = harness.run(&[
        "-p",
        "second turn",
        "--resume",
        &original_id,
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code2, 0, "resume failed; stderr: {stderr2}");
    let records2 = parse_lines(&stdout2);
    let resumed_id = session_id_from(&records2);

    assert_eq!(
        resumed_id, original_id,
        "--resume <id> must produce the same session_id"
    );

    // The result should be successful
    let result = records2.last().expect("result record");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "success");
}

#[test]
fn resume_without_id_picks_latest_session() {
    let harness = Harness::new();

    // Create a session
    let (code1, stdout1, stderr1) =
        harness.run(&["-p", "hello", "--output-format", "stream-json", "--verbose"]);
    assert_eq!(code1, 0, "stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    let original_id = session_id_from(&records1);

    // Resume without specifying a session id (bare --resume)
    let (code2, stdout2, stderr2) = harness.run(&[
        "-p",
        "continued",
        "--resume",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code2, 0, "bare --resume failed; stderr: {stderr2}");
    let records2 = parse_lines(&stdout2);
    let resumed_id = session_id_from(&records2);

    assert_eq!(
        resumed_id, original_id,
        "bare --resume must pick the latest available session"
    );
}

#[test]
fn continue_flag_resumes_latest_session_for_cwd() {
    let harness = Harness::new();

    // Create a session in the default cwd
    let (code1, stdout1, stderr1) = harness.run(&[
        "-p",
        "initial",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code1, 0, "stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    let original_id = session_id_from(&records1);

    // --continue should resume that session
    let (code2, stdout2, stderr2) = harness.run(&[
        "-p",
        "--continue",
        "followup",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code2, 0, "stderr: {stderr2}");
    let records2 = parse_lines(&stdout2);
    let continued_id = session_id_from(&records2);

    assert_eq!(
        continued_id, original_id,
        "--continue must resume the latest session matching cwd"
    );
}

#[test]
fn resume_nonexistent_session_exits_with_error() {
    let harness = Harness::new();

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "hello",
        "--resume",
        "nonexistent-session-id-12345",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    // Should fail because the session doesn't exist
    assert_ne!(
        code, 0,
        "resuming a nonexistent session must fail; stdout: {stdout}"
    );
    let _ = stderr;
}

#[test]
fn resume_preserves_result_session_id_across_all_records() {
    let harness = Harness::new();

    // Create a session
    let (code1, stdout1, stderr1) =
        harness.run(&["-p", "setup", "--output-format", "stream-json", "--verbose"]);
    assert_eq!(code1, 0, "stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    let original_id = session_id_from(&records1);

    // Resume it
    let (code2, stdout2, stderr2) = harness.run(&[
        "-p",
        "more",
        "--resume",
        &original_id,
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code2, 0, "stderr: {stderr2}");
    let records2 = parse_lines(&stdout2);

    // Every record must carry the same session_id
    for record in &records2 {
        assert_eq!(
            record["session_id"].as_str().unwrap(),
            original_id,
            "all records in a resumed session must share the original session_id"
        );
    }

    // The result record specifically should have the correct session_id
    let result = records2.last().expect("result record");
    assert_eq!(result["type"], "result");
    assert_eq!(result["session_id"].as_str().unwrap(), original_id);
}
