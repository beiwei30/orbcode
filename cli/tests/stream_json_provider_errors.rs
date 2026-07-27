//! End-to-end coverage for headless provider-error handling, extending the
//! single `fatal` case in `stream_json_e2e.rs` to the `ratelimit`,
//! `retry_then_success`, and `auth` mock scenarios.
//!
//! Every scenario is driven through a `mock://` Anthropic base URL (see
//! `orbcode-model-provider`'s `mock-provider` feature) rather than by injecting
//! markers into the prompt, so the CLI exercises the real config -> retry ->
//! diagnostics path. Assertions focus on the headless exit code, the
//! stream-json error classification (`category` + `suggestion`), and
//! provider/attempt attribution.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tempfile::TempDir;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

struct Harness {
    _home: TempDir,
    cwd: TempDir,
    home_path: std::path::PathBuf,
}

struct ScenarioRun {
    code: i32,
    stdout: String,
    stderr: String,
    elapsed: Duration,
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

    /// Run a headless stream-json turn against `base_url`, layering any extra env
    /// overrides (e.g. retry budget / backoff knobs) on top of the base config.
    fn run_scenario(
        &self,
        base_url: &str,
        extra_env: &[(&str, &str)],
        args: &[&str],
    ) -> ScenarioRun {
        let mut command = Command::new(ORBCODE_BIN);
        command
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
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let started = Instant::now();
        let output = command.output().expect("spawn orbcode");
        let elapsed = started.elapsed();
        ScenarioRun {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout).expect("stdout utf8"),
            stderr: String::from_utf8(output.stderr).expect("stderr utf8"),
            elapsed,
        }
    }
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("each stream-json line is JSON"))
        .collect()
}

/// Locate the error `stream_event` the headless loop emits before the result.
fn error_stream_event(records: &[Value]) -> &Value {
    records
        .iter()
        .find(|record| record["type"] == "stream_event" && record["event"]["type"] == "error")
        .expect("expected an error stream_event before result")
}

fn result_record(records: &[Value]) -> &Value {
    records.last().expect("at least one record")
}

/// A per-run unique key so the mock's cross-attempt counter for
/// `retry_then_success` can never collide with another run.
fn unique_key(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{}-{nanos}", std::process::id())
}

const STREAM_JSON_ARGS: &[&str] = &["--output-format", "stream-json", "--verbose"];

#[test]
fn stream_json_ratelimit_honors_retry_after_and_classifies_rate_limit() {
    let harness = Harness::new();
    // One retry is allowed, so attempt 1 fails (429 + Retry-After: 1s), the loop
    // waits the server-directed second, attempt 2 fails again, and with no
    // fallback configured the rate-limit error surfaces. Base backoff is
    // collapsed to 0 so the only measurable delay is the honored Retry-After.
    let run = harness.run_scenario(
        "mock://anthropic?scenario=ratelimit",
        &[
            ("ORBCODE_MAX_RETRIES", "1"),
            ("CLAUDE_CODE_RETRY_BASE_DELAY_MS", "0"),
        ],
        &[&["-p", "rate me"], STREAM_JSON_ARGS].concat(),
    );

    assert_eq!(
        run.code, 1,
        "rate-limit exhaustion should exit 1; stderr: {}",
        run.stderr
    );

    let records = parse_lines(&run.stdout);

    let error = error_stream_event(&records);
    assert_eq!(
        error["event"]["category"], "rate_limit",
        "error stream_event must classify as rate_limit: {error}"
    );
    assert_eq!(
        error["event"]["provider"], "anthropic",
        "rate-limit error should attribute to the primary provider: {error}"
    );
    let suggestion = error["event"]["suggestion"]
        .as_str()
        .expect("rate-limit error carries a suggestion");
    assert!(
        suggestion.contains("rate limited") || suggestion.contains("fallback"),
        "rate-limit suggestion should be actionable, got: {suggestion}"
    );

    let result = result_record(&records);
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], true);
    assert_eq!(result["subtype"], "error_during_execution");
    let errors = result["errors"].as_array().expect("errors array");
    let summary = errors
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        summary.contains("[rate_limit]"),
        "result error should label the rate-limit category, got: {summary}"
    );
    assert!(
        summary.contains("anthropic") && summary.contains("2 attempt"),
        "result error should attribute provider + attempt count, got: {summary}"
    );

    // The single honored Retry-After (1s) dominates the run; base backoff is 0,
    // so anything well under a second means the directive was ignored.
    assert!(
        run.elapsed >= Duration::from_millis(900),
        "expected to honor the ~1s Retry-After before retrying; elapsed {:?}",
        run.elapsed
    );
    assert!(
        run.elapsed < Duration::from_secs(30),
        "rate-limit run took far longer than the single Retry-After window: {:?}",
        run.elapsed
    );
}

#[test]
fn stream_json_retry_then_success_recovers_after_retries() {
    let harness = Harness::new();
    // Mock fails retryably on attempts 1 and 2, then succeeds on attempt 3.
    let base_url = format!(
        "mock://anthropic?scenario=retry_then_success&attempts=2&key={}",
        unique_key("cli-e2e")
    );

    // With a budget of 2 retries (3 total attempts) the turn recovers and the
    // final result is a success.
    let run = harness.run_scenario(
        &base_url,
        &[
            ("ORBCODE_MAX_RETRIES", "2"),
            ("CLAUDE_CODE_RETRY_BASE_DELAY_MS", "0"),
        ],
        &[&["-p", "retry me"], STREAM_JSON_ARGS].concat(),
    );
    assert_eq!(
        run.code, 0,
        "retry-then-success should exit 0; stderr: {}",
        run.stderr
    );
    let records = parse_lines(&run.stdout);
    assert!(
        !records
            .iter()
            .any(|record| record["type"] == "stream_event" && record["event"]["type"] == "error"),
        "a recovered turn must not surface an error stream_event"
    );
    let result = result_record(&records);
    assert_eq!(result["subtype"], "success");
    assert_eq!(result["is_error"], false);
    assert!(
        result["result"]
            .as_str()
            .unwrap_or_default()
            .contains("mock provider response"),
        "successful result should carry the recovered assistant text: {result}"
    );
    assert!(
        records.iter().any(|record| record["type"] == "assistant"),
        "a recovered turn must emit an assistant record"
    );

    // Contrast: an insufficient budget (1 retry => 2 attempts) cannot reach the
    // success on attempt 3, proving the retries above are real work and not the
    // mock simply succeeding immediately. A fresh key keeps the mock's
    // cross-attempt counter independent of the run above.
    let starved_url = format!(
        "mock://anthropic?scenario=retry_then_success&attempts=2&key={}",
        unique_key("cli-e2e-starved")
    );
    let starved = harness.run_scenario(
        &starved_url,
        &[
            ("ORBCODE_MAX_RETRIES", "1"),
            ("CLAUDE_CODE_RETRY_BASE_DELAY_MS", "0"),
        ],
        &[&["-p", "retry me"], STREAM_JSON_ARGS].concat(),
    );
    assert_eq!(
        starved.code, 1,
        "an insufficient retry budget should exhaust and exit 1; stderr: {}",
        starved.stderr
    );
    let starved_records = parse_lines(&starved.stdout);
    let starved_error = error_stream_event(&starved_records);
    assert_eq!(
        starved_error["event"]["category"], "server_error",
        "the retryable failures are server errors: {starved_error}"
    );
    let starved_summary = result_record(&starved_records)["errors"]
        .as_array()
        .expect("errors array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        starved_summary.contains("2 attempt"),
        "starved run should report the two attempts it made, got: {starved_summary}"
    );
}

#[test]
fn stream_json_auth_failure_classifies_auth_with_actionable_suggestion() {
    let harness = Harness::new();
    // Auth failures are fatal: no retries, no fallback, surfaced immediately.
    let run = harness.run_scenario(
        "mock://anthropic?scenario=auth",
        &[],
        &[&["-p", "who am i"], STREAM_JSON_ARGS].concat(),
    );
    assert_eq!(
        run.code, 3,
        "auth failure should exit with the dedicated AuthFailure code (3); stderr: {}",
        run.stderr
    );

    let records = parse_lines(&run.stdout);
    let error = error_stream_event(&records);
    assert_eq!(
        error["event"]["category"], "auth",
        "auth failure must classify as auth: {error}"
    );
    assert_eq!(error["event"]["provider"], "anthropic");
    let suggestion = error["event"]["suggestion"]
        .as_str()
        .expect("auth error carries a suggestion");
    assert!(
        suggestion.contains("ANTHROPIC_API_KEY") && suggestion.contains("orbcode auth status"),
        "auth suggestion should point at the documented credential remedies, got: {suggestion}"
    );

    let result = result_record(&records);
    assert_eq!(result["is_error"], true);
    assert_eq!(result["subtype"], "error_during_execution");
    let summary = result["errors"]
        .as_array()
        .expect("errors array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        summary.contains("[auth]") && summary.contains("ANTHROPIC_API_KEY"),
        "result error should carry the auth label and actionable suggestion, got: {summary}"
    );

    // No assistant output should escape a fatal auth failure.
    assert!(
        !records.iter().any(|record| record["type"] == "assistant"),
        "no assistant record should be emitted for a fatal auth error"
    );
}

#[test]
fn stream_json_disabled_provider_gemini_returns_unsupported_error() {
    let harness = Harness::new();
    let run = harness.run_scenario(
        "https://unused.invalid",
        &[("ORBCODE_PROVIDER", "gemini")],
        &[&["-p", "hello"], STREAM_JSON_ARGS].concat(),
    );

    assert_eq!(
        run.code, 1,
        "disabled provider should exit 1; stderr: {}",
        run.stderr
    );

    let records = parse_lines(&run.stdout);
    let error = error_stream_event(&records);
    assert_eq!(
        error["event"]["category"], "unsupported_provider",
        "unsupported provider must surface as 'unsupported_provider' category: {error}"
    );
    assert_eq!(
        error["event"]["provider"], "gemini",
        "error should attribute to the gemini provider: {error}"
    );
    let message = error["event"]["message"]
        .as_str()
        .expect("error carries a message");
    assert!(
        message.contains("not supported") && message.contains("gemini"),
        "error message should name the provider and say it's unsupported, got: {message}"
    );
    let suggestion = error["event"]["suggestion"]
        .as_str()
        .expect("unsupported-provider error carries a suggestion");
    assert!(
        suggestion.contains("anthropic") && suggestion.contains("openai"),
        "suggestion should point to active providers, got: {suggestion}"
    );

    let result = result_record(&records);
    assert_eq!(result["is_error"], true);
    assert_eq!(result["subtype"], "error_during_execution");

    assert!(
        !records.iter().any(|r| r["type"] == "assistant"),
        "no assistant record should be emitted for a disabled provider"
    );
}

#[test]
fn stream_json_disabled_provider_grok_returns_unsupported_error() {
    let harness = Harness::new();
    let run = harness.run_scenario(
        "https://unused.invalid",
        &[("ORBCODE_PROVIDER", "grok")],
        &[&["-p", "hello"], STREAM_JSON_ARGS].concat(),
    );

    assert_eq!(
        run.code, 1,
        "disabled provider should exit 1; stderr: {}",
        run.stderr
    );

    let records = parse_lines(&run.stdout);
    let error = error_stream_event(&records);
    assert_eq!(error["event"]["category"], "unsupported_provider");
    assert_eq!(error["event"]["provider"], "grok");
    assert!(
        error["event"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("grok"),
        "error message should mention grok: {error}"
    );

    let result = result_record(&records);
    assert_eq!(result["is_error"], true);
    assert_eq!(result["subtype"], "error_during_execution");
}

#[test]
fn orbcode_binary_exists_at_expected_path() {
    assert!(
        Path::new(ORBCODE_BIN).exists(),
        "orbcode binary missing at {ORBCODE_BIN}"
    );
}
