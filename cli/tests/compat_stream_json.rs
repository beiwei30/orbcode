//! Byte-level headless stream-json compatibility goldens.
//!
//! Each test drives the real `orbcode` binary in headless `--output-format
//! stream-json` mode against a deterministic provider, normalizes the NDJSON
//! stdout through the shared `orbcode-compat-fixtures` normalizer
//! (`normalize_stream_json`), and asserts it matches a checked-in golden
//! record-for-record. This locks the SDK stream-json wire format against
//! regression the same way `session-store/tests/compat_transcripts.rs` locks the
//! on-disk transcript format.
//!
//! Determinism comes from two always-reproducible provider paths:
//!   * `mock://anthropic?scenario=...` (model-provider `mock-provider` feature,
//!     unified into this binary via the cli dev-dependency) for the plain
//!     success and fatal-error streams, whose content is a fixed string with no
//!     cwd/date/session text embedded; and
//!   * the always-compiled `stub://` provider driven by a `#tool:` directive for
//!     the tool round-trip and permission-deny streams, whose tool ids are
//!     `toolu-<session-uuid>` (folded to `<UUID>`).
//!
//! Volatile fields are folded by the normalizer: per-record `uuid`,
//! `session_id`, `request_id`, and the `toolu-<uuid>` tool ids -> `<UUID>`; ISO
//! `timestamp` -> `<TS>`; the init `cwd` -> `<CWD>`; `duration_ms` /
//! `duration_api_ms` -> `<DUR>`; `claude_code_version` -> `<VERSION>`; Windows
//! path separators -> `/`. Asynchronous `tool_progress` events are dropped.
//!
//! To (re)generate the goldens after an intentional wire-format change, run:
//!   `ORBCODE_UPDATE_STREAM_JSON_GOLDENS=1 cargo test -p orbcode \
//!        --test compat_stream_json`
//! then inspect and commit the rewritten `compat-fixtures/fixtures/stream_json/`
//! files. The goldens are caching-independent: no prompt caching is exercised
//! and `total_cost_usd` / `costUSD` are fixed at `0.0`.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use orbcode_compat_fixtures::{FixtureCategory, category_dir, normalize_stream_json};
use tempfile::TempDir;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");
const UPDATE_ENV: &str = "ORBCODE_UPDATE_STREAM_JSON_GOLDENS";

struct Harness {
    _home: TempDir,
    home_path: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        // Only an API key is layered through settings; the base URL is supplied
        // per run so each scenario can pick the mock or stub provider. A
        // hermetic temp home keeps the run independent of user state.
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
        )
        .expect("write settings");
        let home_path = home.path().to_path_buf();
        Self {
            _home: home,
            home_path,
        }
    }

    /// Run one headless stream-json turn against `base_url` from a throwaway cwd.
    fn run(&self, base_url: &str, args: &[&str]) -> (i32, String, String) {
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        self.run_in_cwd(base_url, args, cwd.path())
    }

    /// Run one headless stream-json turn against `base_url` from a specific cwd.
    fn run_in_cwd(
        &self,
        base_url: &str,
        args: &[&str],
        cwd: &std::path::Path,
    ) -> (i32, String, String) {
        let output = Command::new(ORBCODE_BIN)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("ORBCODE_HOME", &self.home_path)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home_path)
            .env("ANTHROPIC_BASE_URL", base_url)
            .env("ANTHROPIC_API_KEY", "stub-key")
            .env("RUST_LOG", "error")
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

fn golden_path(name: &str) -> PathBuf {
    category_dir(FixtureCategory::StreamJson).join(format!("{name}.jsonl"))
}

/// Compare `normalized` against the golden `name`, or rewrite the golden when
/// the update env var is set. Comparison is record-for-record so a mismatch
/// points at the exact diverging line.
fn assert_or_update_golden(name: &str, normalized: &str) {
    let path = golden_path(name);

    if std::env::var(UPDATE_ENV).is_ok() {
        std::fs::write(&path, format!("{normalized}\n"))
            .unwrap_or_else(|error| panic!("write golden {}: {error}", path.display()));
        return;
    }

    let golden = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing stream-json golden {} ({error}); regenerate with {UPDATE_ENV}=1",
            path.display()
        )
    });

    let actual_lines: Vec<&str> = normalized.lines().filter(|l| !l.is_empty()).collect();
    let golden_lines: Vec<&str> = golden.lines().filter(|l| !l.is_empty()).collect();

    for (index, (actual, expected)) in actual_lines.iter().zip(golden_lines.iter()).enumerate() {
        assert_eq!(
            actual, expected,
            "{name}: stream-json record {index} diverged from golden\n  actual:   {actual}\n  expected: {expected}\n(regenerate with {UPDATE_ENV}=1 after an intentional change)"
        );
    }
    assert_eq!(
        actual_lines.len(),
        golden_lines.len(),
        "{name}: record count diverged (actual {}, golden {}); regenerate with {UPDATE_ENV}=1",
        actual_lines.len(),
        golden_lines.len()
    );
}

const STREAM_ARGS: &[&str] = &["--output-format", "stream-json", "--verbose"];

#[test]
fn compat_stream_json_simple_text_golden() {
    // Plain text turn via the mock success stream: a single content delta, one
    // assistant record, and a success result.
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(
        "mock://anthropic?scenario=success",
        &[&["-p", "say hi"], STREAM_ARGS].concat(),
    );
    assert_eq!(code, 0, "simple text should exit 0; stderr: {stderr}");
    assert_or_update_golden("simple_text", &normalize_stream_json(&stdout));
}

#[test]
fn compat_stream_json_tool_round_trip_golden() {
    // assistant tool_use -> user tool_result -> follow-up assistant text, via the
    // stub `#tool:` directive with the tool pre-allowed so it executes.
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(
        "stub://test",
        &[
            &[
                "-p",
                "#tool:bash {\"command\":\"echo hi\"}",
                "--permission-mode",
                "default",
                "--allowed-tools",
                "bash",
            ],
            STREAM_ARGS,
        ]
        .concat(),
    );
    assert_eq!(code, 0, "tool round-trip should exit 0; stderr: {stderr}");
    assert_or_update_golden("tool_round_trip", &normalize_stream_json(&stdout));
}

#[test]
fn compat_stream_json_permission_deny_golden() {
    // A sandbox escalation in the default Ask preset is denied headlessly:
    // permission_request -> permission_resolved(denied) -> errored tool_result
    // -> tool_use_completed(permission_denied) -> error result.
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(
        "stub://test",
        &[
            &[
                "-p",
                "#tool:bash {\"command\":\"echo hi\",\"sandbox_permissions\":\"require_escalated\"}",
            ],
            STREAM_ARGS,
        ]
        .concat(),
    );
    assert_eq!(
        code, 4,
        "permission deny should exit with the PermissionDenied code (4); stderr: {stderr}"
    );
    assert_or_update_golden("permission_deny", &normalize_stream_json(&stdout));
}

#[test]
fn compat_stream_json_model_error_golden() {
    // Fatal provider error via the mock: an error stream_event, no assistant
    // record, and an error_during_execution result carrying the failure.
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(
        "mock://anthropic?scenario=fatal",
        &[&["-p", "boom"], STREAM_ARGS].concat(),
    );
    assert_eq!(code, 1, "model error should exit 1; stderr: {stderr}");
    assert_or_update_golden("model_error", &normalize_stream_json(&stdout));
}

#[test]
fn compat_stream_json_agent_round_trip_golden() {
    // The agent round-trip golden is hand-authored because the Agent tool's
    // child session inherits the mock base URL, making a generated golden
    // infeasible without recursive-scenario support. The golden documents the
    // expected wire shape: init → user → assistant(tool_use Agent) →
    // tool_use_started → user(tool_result) → tool_use_completed →
    // content_block_delta → assistant → result.
    let path = golden_path("agent_round_trip");
    let contents = std::fs::read_to_string(&path).expect("agent_round_trip golden should exist");
    let normalized = normalize_stream_json(&contents);
    assert_eq!(
        normalize_stream_json(&normalized),
        normalized,
        "normalization should be idempotent on the agent golden"
    );
    let lines: Vec<serde_json::Value> = normalized
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON"))
        .collect();
    assert!(
        lines.len() >= 7,
        "agent golden should have at least 7 records"
    );

    let has_agent_tool_use = lines.iter().any(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("assistant")
            && v.pointer("/message/content")
                .and_then(|c| c.as_array())
                .is_some_and(|a| {
                    a.iter().any(|b| {
                        b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                            && b.get("name").and_then(|n| n.as_str()) == Some("Agent")
                    })
                })
    });
    assert!(has_agent_tool_use, "golden must contain an Agent tool_use");

    let has_tool_use_started = lines.iter().any(|v| {
        v.pointer("/event/type").and_then(|t| t.as_str()) == Some("tool_use_started")
            && v.pointer("/event/tool_name").and_then(|n| n.as_str()) == Some("Agent")
    });
    assert!(
        has_tool_use_started,
        "golden must contain a tool_use_started for Agent"
    );

    let has_tool_use_completed = lines.iter().any(|v| {
        v.pointer("/event/type").and_then(|t| t.as_str()) == Some("tool_use_completed")
            && v.pointer("/event/tool_name").and_then(|n| n.as_str()) == Some("Agent")
    });
    assert!(
        has_tool_use_completed,
        "golden must contain a tool_use_completed for Agent"
    );

    let has_tool_result = lines.iter().any(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("user")
            && v.pointer("/message/content")
                .and_then(|c| c.as_array())
                .is_some_and(|a| {
                    a.iter()
                        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                })
    });
    assert!(
        has_tool_result,
        "golden must contain a tool_result for Agent"
    );
}

#[test]
fn compat_stream_json_compaction_boundary_golden() {
    let path = golden_path("compaction_boundary");
    let contents = std::fs::read_to_string(&path).expect("compaction_boundary golden should exist");
    let normalized = normalize_stream_json(&contents);
    assert_eq!(
        normalize_stream_json(&normalized),
        normalized,
        "normalization should be idempotent on the compaction golden"
    );
    let lines: Vec<serde_json::Value> = normalized
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON"))
        .collect();
    assert!(
        lines
            .iter()
            .any(|v| v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary")),
        "golden must contain a compact_boundary record"
    );
    let compact_record = lines
        .iter()
        .find(|v| v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary"))
        .expect("compact_boundary record");
    let metadata = compact_record
        .get("compact_metadata")
        .expect("compact_metadata field");
    assert_eq!(
        metadata.get("trigger").and_then(|v| v.as_str()),
        Some("auto")
    );
    assert!(metadata.get("pre_messages").is_some());
    assert!(metadata.get("post_messages").is_some());
    assert!(metadata.get("provider_generated").is_some());
}

#[test]
fn compat_stream_json_multi_turn_resume_golden() {
    let harness = Harness::new();
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let (_code1, _stdout1, _stderr1) = harness.run_in_cwd(
        "mock://anthropic?scenario=success",
        &[&["-p", "hello"], STREAM_ARGS].concat(),
        cwd.path(),
    );
    let (code2, stdout2, stderr2) = harness.run_in_cwd(
        "mock://anthropic?scenario=success",
        &[&["-p", "continue from here", "--continue"], STREAM_ARGS].concat(),
        cwd.path(),
    );
    assert_eq!(code2, 0, "resume should exit 0; stderr: {stderr2}");
    assert_or_update_golden("multi_turn_resume", &normalize_stream_json(&stdout2));
}

#[test]
fn compat_stream_json_thinking_delta_golden() {
    let harness = Harness::new();
    let (code, stdout, stderr) = harness.run(
        "mock://anthropic?scenario=thinking",
        &[&["-p", "think about this"], STREAM_ARGS].concat(),
    );
    assert_eq!(code, 0, "thinking delta should exit 0; stderr: {stderr}");
    assert_or_update_golden("thinking_delta", &normalize_stream_json(&stdout));
}

#[test]
fn orbcode_binary_exists_at_expected_path() {
    assert!(
        std::path::Path::new(ORBCODE_BIN).exists(),
        "orbcode binary missing at {ORBCODE_BIN}"
    );
}
