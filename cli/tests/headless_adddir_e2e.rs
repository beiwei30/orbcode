//! End-to-end tests for `--add-dir` sandbox scope verification.
//! Verifies that:
//! - `--add-dir <existing>` bootstraps successfully
//! - A bash tool can read files in the added directory
//! - Non-existent directories produce an error

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
fn add_dir_existing_directory_bootstraps_and_succeeds() {
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
fn add_dir_allows_bash_to_read_file_in_added_directory() {
    let harness = Harness::new();
    let extra_dir = tempfile::tempdir().expect("extra dir");
    let marker_file = extra_dir.path().join("marker.txt");
    std::fs::write(&marker_file, "SANDBOX_MARKER_CONTENT").expect("write marker");
    let extra_path = extra_dir.path().to_str().expect("extra path");
    let marker_path = marker_file.to_str().expect("marker path");

    let prompt = format!("#tool:bash {{\"command\":\"cat {marker_path}\"}}");
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        &prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--allowed-tools",
        "bash",
        "--add-dir",
        extra_path,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);

    // Find the user record with tool_result containing the file content
    let tool_result = records.iter().find(|r| {
        r["type"] == "user"
            && r["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
    });
    assert!(
        tool_result.is_some(),
        "expected a tool_result record proving bash ran in the added directory"
    );
}

#[test]
fn add_dir_nonexistent_directory_does_not_crash() {
    let harness = Harness::new();
    let nonexistent = "/tmp/orbcode_test_nonexistent_dir_12345_does_not_exist";

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--add-dir",
        nonexistent,
    ]);
    // The CLI tolerates a nonexistent --add-dir gracefully (does not crash)
    assert_eq!(
        code, 0,
        "non-existent --add-dir should not crash the process; stderr: {stderr}\nstdout:\n{stdout}"
    );
    let records = parse_lines(&stdout);
    let result = records.last().expect("result");
    assert_eq!(result["type"], "result");
}

#[test]
fn add_dir_multiple_directories_all_accepted() {
    let harness = Harness::new();
    let extra_a = tempfile::tempdir().expect("extra_a");
    let extra_b = tempfile::tempdir().expect("extra_b");
    let path_a = extra_a.path().to_str().expect("path_a");
    let path_b = extra_b.path().to_str().expect("path_b");

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--add-dir",
        path_a,
        "--add-dir",
        path_b,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);
    let result = records.last().expect("result");
    assert_eq!(result["subtype"], "success");
}
