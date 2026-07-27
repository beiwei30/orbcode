//! End-to-end tests for `--settings '{"model":"..."}'` JSON overlay.
//! Verifies that the settings overlay is applied and reflected in the
//! stream-json init record's `model` field.

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
fn settings_overlay_model_reflected_in_init_record() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--settings",
        r#"{"model":"claude-sonnet-4-6-20250514"}"#,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);

    let init = &records[0];
    assert_eq!(init["type"], "system");
    assert_eq!(init["subtype"], "init");
    assert!(
        init["model"]
            .as_str()
            .unwrap()
            .starts_with("claude-sonnet-4-6-20250514"),
        "init record must reflect the overlaid model; got: {}",
        init["model"]
    );
}

#[test]
fn settings_overlay_model_appears_in_stub_response() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--settings",
        r#"{"model":"claude-sonnet-4-6-20250514"}"#,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let records = parse_lines(&stdout);

    // The stub provider echoes the model in its response text
    let assistant = records
        .iter()
        .find(|r| r["type"] == "assistant")
        .expect("assistant record");
    let content = assistant["message"]["content"]
        .as_array()
        .expect("content array");
    let text = content
        .iter()
        .find_map(|block| {
            if block["type"] == "text" {
                block["text"].as_str()
            } else {
                None
            }
        })
        .expect("text block");
    assert!(
        text.contains("claude-sonnet-4-6-20250514"),
        "stub response should echo the resolved model; got: {text}"
    );
}

#[test]
fn settings_overlay_from_file_path() {
    let harness = Harness::new();

    // Write a settings file in cwd
    let settings_path = harness.cwd.path().join("custom-settings.json");
    std::fs::write(&settings_path, r#"{"model":"claude-haiku-4-5-20251001"}"#)
        .expect("write custom settings file");

    let (code, stdout, stderr) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--settings",
        settings_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");
    let records = parse_lines(&stdout);

    let init = &records[0];
    assert!(
        init["model"]
            .as_str()
            .unwrap()
            .starts_with("claude-haiku-4-5-20251001"),
        "init record must reflect the model from settings file; got: {}",
        init["model"]
    );
}

#[test]
fn settings_overlay_does_not_clobber_base_config() {
    let harness = Harness::new();

    // Run without settings overlay — model should be the default
    let (code1, stdout1, stderr1) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code1, 0, "stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    let default_model = records1[0]["model"].as_str().unwrap().to_string();

    // Run with overlay
    let (code2, stdout2, stderr2) = harness.run(&[
        "-p",
        "say hi",
        "--output-format",
        "stream-json",
        "--verbose",
        "--settings",
        r#"{"model":"claude-sonnet-4-6-20250514"}"#,
    ]);
    assert_eq!(code2, 0, "stderr: {stderr2}");
    let records2 = parse_lines(&stdout2);
    let overlay_model = records2[0]["model"].as_str().unwrap().to_string();

    assert_ne!(
        default_model, overlay_model,
        "overlay must change the model from default"
    );
    assert!(
        overlay_model.starts_with("claude-sonnet-4-6-20250514"),
        "overlay model must start with the specified model; got: {overlay_model}"
    );
}
