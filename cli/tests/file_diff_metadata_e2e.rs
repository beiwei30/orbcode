use std::fmt::Write as _;
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

fn find_transcript(home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let filename = format!("{session_id}.jsonl");
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().map(|n| n.to_string_lossy())
                == Some(filename.as_str().into())
            {
                return path;
            }
        }
    }
    panic!("transcript {filename} not found under {}", home.display());
}

fn extract_tool_result_metadata(transcript_path: &std::path::Path) -> Vec<Value> {
    let content = std::fs::read_to_string(transcript_path).expect("read transcript");
    content
        .lines()
        .filter_map(|line| {
            let record: Value = serde_json::from_str(line).ok()?;
            record.get("toolUseResult").cloned()
        })
        .collect()
}

fn session_id_from_stdout(stdout: &str) -> String {
    let records = parse_lines(stdout);
    records[0]["session_id"]
        .as_str()
        .expect("session_id in init record")
        .to_string()
}

// ---------------------------------------------------------------------------
// file-edit: basic diff metadata
// ---------------------------------------------------------------------------

#[test]
fn file_edit_produces_diff_metadata_in_transcript() {
    let harness = Harness::new();
    std::fs::write(
        harness.cwd.path().join("target.txt"),
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\n",
    )
    .expect("write fixture");

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-read {"file_path":"target.txt"} #then:file-edit {"file_path":"target.txt","old_string":"line4","new_string":"REPLACED"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let session_id = session_id_from_stdout(&stdout);
    let transcript = find_transcript(&harness.home_path, &session_id);
    let metadatas = extract_tool_result_metadata(&transcript);

    let edit_meta = metadatas
        .iter()
        .find(|m| m.get("diff").is_some())
        .expect("must have a toolUseResult with diff field");

    let diff = edit_meta["diff"].as_str().expect("diff is a string");
    assert!(diff.contains("@@"), "diff must contain hunk header");
    assert!(diff.contains("-line4"), "diff must show removed line");
    assert!(diff.contains("+REPLACED"), "diff must show added line");
    assert!(diff.contains(" line3"), "diff must include context before");
    assert!(diff.contains(" line5"), "diff must include context after");

    assert_eq!(edit_meta["lineRange"]["start"], 4);
    assert_eq!(edit_meta["lineRange"]["end"], 4);
    assert_eq!(edit_meta["linesAdded"], 1);
    assert_eq!(edit_meta["linesRemoved"], 1);
    assert!(
        edit_meta.get("diffTruncated").is_none(),
        "diffTruncated must be absent for small diff"
    );
}

// ---------------------------------------------------------------------------
// file-edit: large diff is truncated
// ---------------------------------------------------------------------------

#[test]
fn file_edit_large_diff_is_truncated() {
    let harness = Harness::new();

    let mut content = String::new();
    for i in 1..=50 {
        writeln!(content, "line{i}").expect("writing to String cannot fail");
    }
    std::fs::write(harness.cwd.path().join("big.txt"), &content).expect("write fixture");

    let old_string: String = (10..=35).map(|i| format!("line{i}\\n")).collect::<String>();
    let old_string = old_string.trim_end_matches("\\n");
    let new_string: String = (10..=35)
        .map(|i| format!("CHANGED{i}\\n"))
        .collect::<String>();
    let new_string = new_string.trim_end_matches("\\n");

    let prompt = format!(
        r#"#tool:file-read {{"file_path":"big.txt"}} #then:file-edit {{"file_path":"big.txt","old_string":"{old_string}","new_string":"{new_string}"}}"#,
    );

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        &prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let session_id = session_id_from_stdout(&stdout);
    let transcript = find_transcript(&harness.home_path, &session_id);
    let metadatas = extract_tool_result_metadata(&transcript);

    let edit_meta = metadatas
        .iter()
        .find(|m| m.get("diff").is_some())
        .expect("must have diff metadata");

    assert_eq!(
        edit_meta["diffTruncated"], true,
        "diffTruncated must be true for large edit"
    );
    let diff = edit_meta["diff"].as_str().unwrap();
    let line_count = diff.lines().count();
    assert!(
        line_count <= 20,
        "diff must be capped at 20 lines, got {line_count}"
    );
}

// ---------------------------------------------------------------------------
// file-edit: replace_all diff shows first + last with summary
// ---------------------------------------------------------------------------

#[test]
fn file_edit_replace_all_diff_summarizes_middle() {
    let harness = Harness::new();
    std::fs::write(
        harness.cwd.path().join("multi.txt"),
        "aaa\nFOO\nbbb\nFOO\nccc\nFOO\nddd\nFOO\neee\n",
    )
    .expect("write fixture");

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-read {"file_path":"multi.txt"} #then:file-edit {"file_path":"multi.txt","old_string":"FOO","new_string":"BAR","replace_all":true}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let session_id = session_id_from_stdout(&stdout);
    let transcript = find_transcript(&harness.home_path, &session_id);
    let metadatas = extract_tool_result_metadata(&transcript);

    let edit_meta = metadatas
        .iter()
        .find(|m| m.get("diff").is_some())
        .expect("must have diff metadata");

    assert_eq!(edit_meta["linesAdded"], 4);
    assert_eq!(edit_meta["linesRemoved"], 4);

    let diff = edit_meta["diff"].as_str().unwrap();
    assert!(
        diff.contains("more replacement(s)"),
        "diff must summarize middle occurrences, got:\n{diff}"
    );
    assert!(diff.contains("-FOO"), "diff must show removed FOO");
    assert!(diff.contains("+BAR"), "diff must show added BAR");

    let on_disk = std::fs::read_to_string(harness.cwd.path().join("multi.txt")).unwrap();
    assert!(!on_disk.contains("FOO"), "all FOO must be replaced");
    assert_eq!(
        on_disk.matches("BAR").count(),
        4,
        "all 4 occurrences must become BAR"
    );
}

// ---------------------------------------------------------------------------
// file-write: linesWritten metadata
// ---------------------------------------------------------------------------

#[test]
fn file_write_reports_lines_written_in_transcript() {
    let harness = Harness::new();

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-write {"file_path":"output.txt","content":"alpha\nbeta\ngamma\n"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let session_id = session_id_from_stdout(&stdout);
    let transcript = find_transcript(&harness.home_path, &session_id);
    let metadatas = extract_tool_result_metadata(&transcript);

    let write_meta = metadatas
        .iter()
        .find(|m| m.get("linesWritten").is_some())
        .expect("must have linesWritten metadata");

    assert_eq!(
        write_meta["linesWritten"], 3,
        "linesWritten must be 3 for a 3-line file"
    );

    let on_disk = std::fs::read_to_string(harness.cwd.path().join("output.txt")).unwrap();
    assert_eq!(on_disk, "alpha\nbeta\ngamma\n");
}
