use std::fs;
use std::process::Command;

use serde_json::{Value, json};

/// Drops a single transcript exercising every high-value message variant
/// (plus a crash-truncated tail) into an isolated `CLAUDE_CONFIG_DIR`, runs
/// the real `orbcode sessions --json` binary, and asserts the session loads as
/// `available` with exactly the messages the loader should surface. Guards the
/// CLI -> AppServer -> SessionManager -> SessionStore wiring end-to-end: the
/// `transcript.rs` unit tests prove each variant decodes in isolation, but
/// only this exercises the real binary classifying them on disk without
/// panicking or marking the session corrupt.
#[test]
fn sessions_json_loads_all_message_variants_as_available() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home dir");
    fs::create_dir_all(&cwd).expect("create cwd dir");

    // Orb Code sanitizes the realpath into the project dir name, so the on-disk
    // layout must match what session discovery scans (macOS maps /var ->
    // /private/var, /tmp -> /private/tmp).
    let cwd_real = fs::canonicalize(&cwd).expect("canonicalize cwd");
    let project_name = sanitize_path(&cwd_real.display().to_string());
    let project_dir = home.join("projects").join(&project_name);
    fs::create_dir_all(&project_dir).expect("create project dir");

    fs::write(project_dir.join("variety.jsonl"), variety_transcript())
        .expect("write variety transcript");

    let binary = env!("CARGO_BIN_EXE_orbcode");
    let output = Command::new(binary)
        .current_dir(&cwd_real)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .arg("sessions")
        .arg("--json")
        .output()
        .expect("run orbcode sessions --json");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sessions --json should exit 0\nstatus: {:?}\nstderr:\n{}\nstdout:\n{}",
        output.status.code(),
        stderr,
        stdout,
    );

    // One JSON object per line; find ours.
    let summary = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find(|value| value.get("session_id").and_then(Value::as_str) == Some("variety"))
        .unwrap_or_else(|| panic!("variety session missing from sessions --json:\n{stdout}"));

    // Loaded cleanly despite the crash-truncated trailing record and the
    // unknown / content-less variants — not flagged corrupt.
    assert_eq!(
        summary.get("status").and_then(|s| s.get("kind")),
        Some(&Value::String("available".into())),
        "session must load as available, got: {summary}"
    );

    // Exactly the seven surfaced records (raw has 12 records + 1 truncated):
    //   surfaced: user, system/local_command, assistant/redacted_thinking,
    //             system/api_error, assistant/tool_use, user/tool_result,
    //             system/snip_boundary
    //   dropped : system/init, attachment, unknown type, stop_hook_summary
    //             (content-less or unknown), the crash-truncated tail
    //   merged  : hook-style progress -> folded into the tool_result metadata
    assert_eq!(
        summary.get("message_count").and_then(Value::as_u64),
        Some(7),
        "expected 7 surfaced messages, got: {summary}"
    );

    // Metadata is recovered from the records that do surface.
    assert_eq!(
        summary.get("title").and_then(Value::as_str),
        Some("build the feature"),
        "title derives from first user message"
    );
    assert_eq!(
        summary.get("model").and_then(Value::as_str),
        Some("claude-opus-4-7"),
    );
    assert_eq!(
        summary.get("git_branch").and_then(Value::as_str),
        Some("main"),
    );
}

/// Build the variety transcript: every high-value variant in one file, with a
/// crash-truncated final line (no trailing newline) standing in for a process
/// that died mid-append.
fn variety_transcript() -> String {
    let records = [
        json!({
            "type": "system", "subtype": "init", "uuid": "r-init",
            "timestamp": "2026-05-29T00:00:00Z", "cwd": "/repo",
            "model": "claude-opus-4-7"
        }),
        json!({
            "type": "user", "uuid": "r-user", "parentUuid": Value::Null,
            "timestamp": "2026-05-29T00:00:01Z", "sessionId": "variety",
            "cwd": "/repo", "gitBranch": "main",
            "message": { "role": "user", "content": "build the feature" }
        }),
        json!({
            "type": "system", "subtype": "local_command", "uuid": "r-localcmd",
            "timestamp": "2026-05-29T00:00:02Z",
            "content": "<command-name>/status</command-name>"
        }),
        json!({
            "type": "attachment", "uuid": "r-att",
            "timestamp": "2026-05-29T00:00:03Z",
            "attachment": { "type": "selected_lines", "filename": "x.rs" }
        }),
        json!({
            "type": "assistant", "uuid": "r-redacted", "parentUuid": "r-user",
            "timestamp": "2026-05-29T00:00:04Z",
            "message": {
                "role": "assistant", "model": "claude-opus-4-7",
                "content": [
                    { "type": "redacted_thinking", "data": "Zz==" },
                    { "type": "text", "text": "Starting now." }
                ]
            }
        }),
        json!({
            "type": "system", "subtype": "api_error", "uuid": "r-apierr",
            "timestamp": "2026-05-29T00:00:05Z",
            "error": { "message": "overloaded_error" },
            "retryAttempt": 1, "maxRetries": 3
        }),
        json!({
            "type": "assistant", "uuid": "r-tool", "parentUuid": "r-redacted",
            "timestamp": "2026-05-29T00:00:06Z",
            "message": {
                "role": "assistant", "model": "claude-opus-4-7",
                "content": [{
                    "type": "tool_use", "id": "toolu_mix", "name": "Bash",
                    "input": { "command": "make" }
                }]
            }
        }),
        json!({
            "type": "progress", "uuid": "r-prog",
            "parentToolUseID": "toolu_mix",
            "timestamp": "2026-05-29T00:00:07Z",
            "data": { "type": "hook_progress", "hookEvent": "PostToolUse" }
        }),
        json!({
            "type": "user", "uuid": "r-result", "parentUuid": "r-tool",
            "timestamp": "2026-05-29T00:00:08Z",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result", "tool_use_id": "toolu_mix",
                    "content": "build ok", "is_error": false
                }]
            }
        }),
        json!({
            "type": "system", "subtype": "snip_boundary", "uuid": "r-snip",
            "timestamp": "2026-05-29T00:00:09Z",
            "snipMetadata": { "removedUuids": ["r-user"] }
        }),
        json!({
            "type": "team-only-future-thing", "uuid": "r-unknown",
            "timestamp": "2026-05-29T00:00:10Z", "payload": { "x": 1 }
        }),
        json!({
            "type": "system", "subtype": "stop_hook_summary", "uuid": "r-stop",
            "timestamp": "2026-05-29T00:00:11Z"
        }),
    ];

    let mut body = records
        .iter()
        .map(|record| serde_json::to_string(record).expect("serialize record"))
        .collect::<Vec<_>>()
        .join("\n");
    body.push('\n');
    // Crash-truncated final record: no trailing newline, invalid JSON.
    body.push_str("{\"type\":\"assistant\",\"uuid\":\"r-trunc\",\"timesta");
    body
}

/// Mirror of `orbcode_config::claude_home::sanitize_path` for paths short enough
/// to skip the hash suffix. The integration test never produces a > 200 char
/// path so the simple form suffices.
fn sanitize_path(name: &str) -> String {
    const MAX: usize = 200;
    let sanitized: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    assert!(
        sanitized.len() <= MAX,
        "test fixture path exceeds sanitize length cap ({} > {MAX}); shorten the tempdir prefix",
        sanitized.len(),
    );
    sanitized
}
