//! End-to-end CLI tests for compaction behavior (snip, auto-compact).
//!
//! Each test spawns the real `orbcode` binary against a `stub://` provider
//! (in-process, no network) and drives compaction via low env-var thresholds.
//! Multi-turn sessions use `--resume <id>` to share transcript state across
//! separate CLI invocations.

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
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8(output.stdout).expect("stdout utf8"),
            String::from_utf8(output.stderr).expect("stderr utf8"),
        )
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
        .expect("first record has session_id")
        .to_string()
}

fn compact_boundary_records(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|r| r["type"] == "system" && r["subtype"] == "compact_boundary")
        .collect()
}

const STREAM_JSON_ARGS: &[&str] = &["--output-format", "stream-json", "--verbose"];

/// Snip fires on the second turn when the oversized first-turn prompt becomes
/// history, but does NOT fire on the first turn (snip never touches the live
/// prompt).
#[test]
fn compaction_snip_two_turn_fires_on_oversized_history() {
    let harness = Harness::new();
    let snip_env = [("ORBCODE_SNIP_MESSAGE_TOKEN_THRESHOLD_OVERRIDE", "50")];
    let oversized = "x".repeat(8_000);

    let (code1, stdout1, stderr1) =
        harness.run(&snip_env, &[&["-p", &oversized], STREAM_JSON_ARGS].concat());
    assert_eq!(code1, 0, "turn 1 must succeed; stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    assert!(
        compact_boundary_records(&records1).is_empty(),
        "turn 1 must NOT snip the live prompt"
    );
    let sid = session_id_from(&records1);

    let (code2, stdout2, stderr2) = harness.run(
        &snip_env,
        &[
            &["--resume", &sid, "-p", "second turn after snip"],
            STREAM_JSON_ARGS,
        ]
        .concat(),
    );
    assert_eq!(
        code2, 0,
        "turn 2 must succeed after snip; stderr: {stderr2}"
    );
    let records2 = parse_lines(&stdout2);
    let boundaries = compact_boundary_records(&records2);
    assert_eq!(
        boundaries.len(),
        1,
        "turn 2 must emit exactly one compact_boundary"
    );
    let metadata = &boundaries[0]["compact_metadata"];
    assert_eq!(
        metadata["provider_generated"], false,
        "snip is lightweight — not provider-generated"
    );
}

/// Auto-compact fires on the second turn when accumulated history exceeds the
/// blocking limit. The provider is called for a compaction summary.
///
/// Turn 1 runs without a blocking limit so the oversized prompt succeeds.
/// Turn 2 sets a low blocking limit so the accumulated history (turn-1 prompt +
/// response + turn-2 prompt) exceeds it, triggering auto-compact.
#[test]
fn compaction_auto_compact_two_turn_emits_provider_summary() {
    let harness = Harness::new();
    let oversized = "x".repeat(32_000);

    let (code1, stdout1, stderr1) =
        harness.run(&[], &[&["-p", &oversized], STREAM_JSON_ARGS].concat());
    assert_eq!(code1, 0, "turn 1 must succeed; stderr: {stderr1}");
    let records1 = parse_lines(&stdout1);
    let sid = session_id_from(&records1);

    let compact_env = [("CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE", "5000")];
    let (code2, stdout2, stderr2) = harness.run(
        &compact_env,
        &[
            &["--resume", &sid, "-p", "follow-up after auto-compact"],
            STREAM_JSON_ARGS,
        ]
        .concat(),
    );
    assert_eq!(
        code2, 0,
        "turn 2 must succeed after auto-compact; stderr: {stderr2}"
    );
    let records2 = parse_lines(&stdout2);
    let boundaries = compact_boundary_records(&records2);
    assert!(
        !boundaries.is_empty(),
        "turn 2 must emit a compact_boundary when history exceeds blocking limit"
    );
    let metadata = &boundaries[0]["compact_metadata"];
    assert_eq!(
        metadata["provider_generated"], true,
        "auto-compact must be provider-generated"
    );
    assert!(
        metadata["pre_messages"].as_u64().unwrap_or(0) >= 2,
        "pre_messages must include at least the user+assistant pair from turn 1"
    );
}

/// After auto-compact, a third turn via --resume sees the compacted session and
/// does NOT trigger another compaction.
///
/// Turn 1 runs without a blocking limit; turns 2 and 3 set a low limit. Turn 2
/// triggers compaction; turn 3 should find the history already compact.
#[test]
fn compaction_auto_compact_resume_preserves_session_continuity() {
    let harness = Harness::new();
    let compact_env = [("CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE", "5000")];
    let oversized = "x".repeat(32_000);

    let (code1, stdout1, _) = harness.run(&[], &[&["-p", &oversized], STREAM_JSON_ARGS].concat());
    assert_eq!(code1, 0, "turn 1 must succeed");
    let sid = session_id_from(&parse_lines(&stdout1));

    let (code2, stdout2, _) = harness.run(
        &compact_env,
        &[
            &["--resume", &sid, "-p", "trigger compact"],
            STREAM_JSON_ARGS,
        ]
        .concat(),
    );
    assert_eq!(code2, 0, "turn 2 must succeed");
    let records2 = parse_lines(&stdout2);
    assert!(
        !compact_boundary_records(&records2).is_empty(),
        "turn 2 must emit compact_boundary"
    );

    let (code3, stdout3, stderr3) = harness.run(
        &compact_env,
        &[
            &["--resume", &sid, "-p", "post-compact follow-up"],
            STREAM_JSON_ARGS,
        ]
        .concat(),
    );
    assert_eq!(
        code3, 0,
        "turn 3 must succeed after prior compaction; stderr: {stderr3}"
    );
    let records3 = parse_lines(&stdout3);
    let sid3 = session_id_from(&records3);
    assert_eq!(
        sid, sid3,
        "session_id must stay the same across all 3 turns"
    );
    assert!(
        compact_boundary_records(&records3).is_empty(),
        "turn 3 must NOT compact again — history is already small after prior compaction"
    );
}

/// Snip never fires on the live prompt (the trailing message), even when it
/// exceeds the snip threshold.
#[test]
fn compaction_snip_does_not_emit_compact_boundary_on_live_prompt() {
    let harness = Harness::new();
    let snip_env = [("ORBCODE_SNIP_MESSAGE_TOKEN_THRESHOLD_OVERRIDE", "50")];
    let oversized = "x".repeat(8_000);

    let (code, stdout, stderr) =
        harness.run(&snip_env, &[&["-p", &oversized], STREAM_JSON_ARGS].concat());
    assert_eq!(code, 0, "turn must succeed; stderr: {stderr}");
    let records = parse_lines(&stdout);
    assert!(
        compact_boundary_records(&records).is_empty(),
        "snip must NOT fire on the live prompt — only on history messages"
    );
}
