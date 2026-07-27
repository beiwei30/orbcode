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
fn file_edit_crlf_round_trip_preserves_line_endings() {
    let harness = Harness::new();
    std::fs::write(
        harness.cwd.path().join("crlf.txt"),
        "alpha\r\nbeta\r\ngamma\r\n",
    )
    .expect("write CRLF fixture");

    let (code, _stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-read {"file_path":"crlf.txt"} #then:file-edit {"file_path":"crlf.txt","old_string":"beta\n","new_string":"BETA\n"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let result = std::fs::read(harness.cwd.path().join("crlf.txt")).expect("read result");
    assert_eq!(
        result, b"alpha\r\nBETA\r\ngamma\r\n",
        "CRLF line endings must be preserved after edit"
    );
}

#[test]
fn file_edit_preserves_bom() {
    let harness = Harness::new();
    std::fs::write(harness.cwd.path().join("bom.txt"), "\u{FEFF}hello\nworld\n")
        .expect("write BOM fixture");

    let (code, _stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-read {"file_path":"bom.txt"} #then:file-edit {"file_path":"bom.txt","old_string":"hello","new_string":"HELLO"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let result = std::fs::read(harness.cwd.path().join("bom.txt")).expect("read result");
    let text = String::from_utf8(result).expect("utf8");
    assert!(
        text.starts_with('\u{FEFF}'),
        "UTF-8 BOM must be preserved after edit"
    );
    assert!(text.contains("HELLO"), "edit must be applied");
    assert!(!text.contains("hello"), "old text must be replaced");
}

#[test]
fn file_write_preserves_bom_on_overwrite() {
    let harness = Harness::new();
    std::fs::write(harness.cwd.path().join("bom.txt"), "\u{FEFF}original\n")
        .expect("write BOM fixture");

    let (code, _stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-write {"file_path":"bom.txt","content":"replaced"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let result = std::fs::read(harness.cwd.path().join("bom.txt")).expect("read result");
    let text = String::from_utf8(result).expect("utf8");
    assert!(
        text.starts_with('\u{FEFF}'),
        "UTF-8 BOM must be preserved on overwrite"
    );
    assert!(text.contains("replaced"));
}

#[test]
fn file_write_preserves_crlf_on_overwrite() {
    let harness = Harness::new();
    std::fs::write(harness.cwd.path().join("crlf.txt"), "line1\r\nline2\r\n")
        .expect("write CRLF fixture");

    let (code, _stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-write {"file_path":"crlf.txt","content":"new1\nnew2\n"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let result = std::fs::read(harness.cwd.path().join("crlf.txt")).expect("read result");
    assert_eq!(
        result, b"new1\r\nnew2\r\n",
        "line endings must be converted to CRLF on overwrite"
    );
}

#[test]
fn file_write_new_file_appends_final_newline() {
    let harness = Harness::new();

    let (code, _stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-write {"file_path":"new.txt","content":"no trailing newline"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let result = std::fs::read(harness.cwd.path().join("new.txt")).expect("read result");
    assert_eq!(
        result, b"no trailing newline\n",
        "new file must get trailing newline"
    );
}

#[test]
fn file_write_preserves_no_final_newline() {
    let harness = Harness::new();
    std::fs::write(harness.cwd.path().join("nonl.txt"), "no newline")
        .expect("write no-newline fixture");

    let (code, _stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-write {"file_path":"nonl.txt","content":"replaced"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let result = std::fs::read(harness.cwd.path().join("nonl.txt")).expect("read result");
    assert_eq!(
        result, b"replaced",
        "must NOT add trailing newline when original had none"
    );
}

#[test]
fn file_write_existing_with_final_newline_appends_newline() {
    let harness = Harness::new();
    std::fs::write(harness.cwd.path().join("has-nl.txt"), "original\n")
        .expect("write newline fixture");

    let (code, _stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-write {"file_path":"has-nl.txt","content":"replaced"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let result = std::fs::read(harness.cwd.path().join("has-nl.txt")).expect("read result");
    assert_eq!(
        result, b"replaced\n",
        "must add trailing newline when original had one"
    );
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

#[test]
fn file_read_metadata_reports_encoding_and_bom() {
    let harness = Harness::new();
    std::fs::write(harness.cwd.path().join("bom.txt"), "\u{FEFF}hello\n")
        .expect("write BOM fixture");

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        r#"#tool:file-read {"file_path":"bom.txt"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let records = parse_lines(&stdout);
    let session_id = records[0]["session_id"]
        .as_str()
        .expect("session_id in init");
    let transcript = find_transcript(&harness.home_path, session_id);
    let metadatas = extract_tool_result_metadata(&transcript);
    assert!(
        !metadatas.is_empty(),
        "transcript must contain a tool_result with metadata"
    );
    let meta = &metadatas[0];
    assert_eq!(meta["encoding"], "utf-8", "encoding must be utf-8");
    assert_eq!(meta["hasBom"], true, "hasBom must be true for BOM file");

    std::fs::write(harness.cwd.path().join("plain.txt"), "hello\n").expect("write plain fixture");

    let (code2, stdout2, stderr2) = harness.run(&[
        "-p",
        r#"#tool:file-read {"file_path":"plain.txt"}"#,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
    ]);
    assert_eq!(code2, 0, "stderr: {stderr2}");

    let records2 = parse_lines(&stdout2);
    let session_id2 = records2[0]["session_id"]
        .as_str()
        .expect("session_id in init");
    let transcript2 = find_transcript(&harness.home_path, session_id2);
    let metadatas2 = extract_tool_result_metadata(&transcript2);
    assert!(
        !metadatas2.is_empty(),
        "transcript must contain a tool_result with metadata"
    );
    let meta2 = &metadatas2[0];
    assert_eq!(meta2["encoding"], "utf-8", "encoding must be utf-8");
    assert_eq!(
        meta2["hasBom"], false,
        "hasBom must be false for plain file"
    );
}
