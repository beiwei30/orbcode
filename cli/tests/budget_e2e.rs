//! End-to-end coverage that the headless `-p` path enforces `maxBudgetUsd`.
//!
//! A configured cap blocks the next provider request and ends the turn with a
//! `budget` stream event plus an `error_max_budget_usd` result; leaving the cap
//! unset preserves the prior success behavior (zero regression). Both scenarios
//! drive the real `orbcode` binary against the always-compiled `stub://` provider
//! and trigger a Bash tool round so the turn loop re-enters the budget precheck
//! before a second provider request.

use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

/// A Bash tool round: the stub provider replays the directive as a `tool_use`,
/// the real Bash tool runs, and the loop re-enters the budget precheck before
/// the follow-up provider request.
const BASH_TURN: &[&str] = &[
    "-p",
    r#"#tool:bash {"command":"echo hi"}"#,
    "--output-format",
    "stream-json",
    "--verbose",
    "--permission-mode",
    "acceptEdits",
    "--allowed-tools",
    "bash",
];

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

    /// Run a headless turn, layering any extra environment over the stub config.
    fn run(&self, extra_env: &[(&str, &str)], args: &[&str]) -> (i32, String, String) {
        let mut command = Command::new(ORBCODE_BIN);
        command
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
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let output = command.output().expect("spawn orbcode");
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

fn budget_event(records: &[Value]) -> Option<&Value> {
    records
        .iter()
        .find(|record| record["type"] == "stream_event" && record["event"]["type"] == "budget")
}

fn result_envelope(records: &[Value]) -> &Value {
    records
        .iter()
        .find(|record| record["type"] == "result")
        .expect("a result envelope is always emitted")
}

#[test]
fn budget_cap_blocks_turn_and_reports_error_max_budget_usd() {
    let harness = Harness::new();
    // A tiny cap plus strict-unknown-pricing guarantees the second precheck
    // blocks whether or not the stub model has known pricing.
    let (code, stdout, stderr) = harness.run(
        &[
            ("ORBCODE_MAX_BUDGET_USD", "0.000001"),
            ("ORBCODE_MAX_BUDGET_STRICT_UNKNOWN", "1"),
        ],
        BASH_TURN,
    );
    let records = parse_lines(&stdout);

    let budget = budget_event(&records).unwrap_or_else(|| {
        panic!("expected a budget stream event\nstderr:\n{stderr}\nstdout:\n{stdout}")
    });
    assert_eq!(
        budget["event"]["blocked"],
        Value::Bool(true),
        "budget event must signal a hard block: {budget}"
    );
    assert!(
        budget["event"]["max_budget_usd"].is_number(),
        "budget event carries the configured cap: {budget}"
    );
    assert!(
        budget["event"]["total_cost_usd"].is_number(),
        "budget event carries the running total: {budget}"
    );

    let result = result_envelope(&records);
    assert_eq!(
        result["subtype"], "error_max_budget_usd",
        "terminal result is the dedicated budget subtype: {result}"
    );
    assert_eq!(
        result["is_error"],
        Value::Bool(true),
        "budget result is an error: {result}"
    );
    assert_eq!(
        result["stop_reason"], "budget_exceeded",
        "budget result stop reason: {result}"
    );
    assert_eq!(
        result["num_turns"],
        serde_json::json!(1),
        "the turn must stop before issuing a second provider request: {result}"
    );

    // The budget block maps to the dedicated max-budget exit code (7), wired by
    // the merge of `budget-enforcement` with the `headless-exit-codes` outcome
    // classifier (`HeadlessOutcome::MaxBudget`).
    assert_eq!(code, 7, "stderr:\n{stderr}\nstdout:\n{stdout}");
}

#[test]
fn unset_budget_preserves_success_behavior() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(&[], BASH_TURN);
    let records = parse_lines(&stdout);

    assert!(
        budget_event(&records).is_none(),
        "no budget events when the cap is unset\nstdout:\n{stdout}"
    );
    let result = result_envelope(&records);
    assert_eq!(
        result["subtype"], "success",
        "unset cap completes normally: {result}"
    );
    assert_eq!(code, 0, "stderr:\n{stderr}\nstdout:\n{stdout}");
}
