//! End-to-end tests for stream-json result cost fields.
//! Verifies that the result envelope carries `total_cost_usd`, `pricing_known`,
//! and `modelUsage` with per-model cost breakdowns.

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
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("each stream-json line is JSON"))
        .collect()
}

#[test]
fn result_carries_total_cost_usd_field() {
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
    let result = records.last().expect("result record");

    assert_eq!(result["type"], "result");
    assert!(
        result["total_cost_usd"].is_number(),
        "result must carry total_cost_usd as a number; got: {}",
        result["total_cost_usd"]
    );
    assert!(
        result["total_cost_usd"].as_f64().unwrap() >= 0.0,
        "total_cost_usd must be non-negative"
    );
}

#[test]
fn result_carries_pricing_known_boolean() {
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
    let result = records.last().expect("result record");

    assert!(
        result["pricing_known"].is_boolean(),
        "result must carry pricing_known as a boolean; got: {}",
        result["pricing_known"]
    );
}

#[test]
fn result_model_usage_contains_cost_usd_per_model() {
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
    let result = records.last().expect("result record");

    let model_usage = result["modelUsage"]
        .as_object()
        .expect("modelUsage must be an object");
    assert!(
        !model_usage.is_empty(),
        "modelUsage must have at least one model entry"
    );

    for (model_name, entry) in model_usage {
        assert!(
            entry["costUSD"].is_number(),
            "modelUsage[{model_name}] must have costUSD as number"
        );
        assert!(
            entry["inputTokens"].as_u64().is_some(),
            "modelUsage[{model_name}] must have inputTokens"
        );
        assert!(
            entry["outputTokens"].as_u64().is_some(),
            "modelUsage[{model_name}] must have outputTokens"
        );
    }
}

#[test]
fn result_usage_carries_token_counts() {
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
    let result = records.last().expect("result record");

    let usage = &result["usage"];
    assert!(
        usage["input_tokens"].as_u64().is_some(),
        "usage.input_tokens must be present"
    );
    assert!(
        usage["output_tokens"].as_u64().is_some(),
        "usage.output_tokens must be present"
    );
    assert!(
        usage["input_tokens"].as_u64().unwrap() > 0,
        "input_tokens must be > 0 for a completed turn"
    );
    assert!(
        usage["output_tokens"].as_u64().unwrap() > 0,
        "output_tokens must be > 0 for a completed turn"
    );
}

#[test]
fn result_cost_fields_present_on_error_exit() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run_with_base_url(
        "mock://anthropic?scenario=fatal",
        &["-p", "boom", "--output-format", "stream-json", "--verbose"],
    );
    assert_eq!(code, 1, "stderr: {stderr}");
    let records = parse_lines(&stdout);
    let result = records.last().expect("result record");

    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], true);
    assert!(
        result["total_cost_usd"].is_number(),
        "total_cost_usd must be present even on error"
    );
    assert!(
        result["pricing_known"].is_boolean(),
        "pricing_known must be present even on error"
    );
}

#[test]
fn result_carries_duration_fields() {
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
    let result = records.last().expect("result record");

    assert!(
        result["duration_ms"].is_number(),
        "result must carry duration_ms"
    );
    assert!(
        result["duration_ms"].as_u64().unwrap() > 0,
        "duration_ms must be positive for a completed turn"
    );
    assert!(
        result["duration_api_ms"].is_number(),
        "result must carry duration_api_ms"
    );
}

#[test]
fn multi_turn_accumulates_cost() {
    let harness = Harness::new();

    // First turn
    let (code1, stdout1, stderr1) = harness.run(&[
        "-p",
        "first message",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code1, 0, "stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    let result1 = records1.last().expect("result1");
    let cost1 = result1["total_cost_usd"].as_f64().unwrap();
    let session_id = records1[0]["session_id"].as_str().unwrap().to_string();

    // Second turn (resume)
    let (code2, stdout2, stderr2) = harness.run(&[
        "-p",
        "second message",
        "--resume",
        &session_id,
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    assert_eq!(code2, 0, "stderr: {stderr2}");
    let records2 = parse_lines(&stdout2);
    let result2 = records2.last().expect("result2");
    let cost2 = result2["total_cost_usd"].as_f64().unwrap();

    assert!(
        cost2 >= cost1,
        "accumulated cost after second turn ({cost2}) should be >= first turn cost ({cost1})"
    );
}
