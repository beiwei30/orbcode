//! End-to-end coverage for background agent orphan detection on process restart.
//!
//! A `local_agent` background task record left in `Running` status after a
//! process crash has no live tokio task behind it. On the next `AppServer`
//! construction (simulating a restart), `reconcile_orphaned_agents` marks it
//! `Orphaned` on disk. This test writes the on-disk record directly and then
//! runs the `orbcode ps` command (which constructs a fresh AppServer internally)
//! to verify the reconciliation path.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run_orbcode(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_orbcode"))
        .current_dir(cwd)
        .env("ORBCODE_HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("run orbcode {args:?}: {err}"))
}

#[test]
fn orphaned_local_agent_detected_after_restart() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let jobs_dir = home.join("background").join("jobs");
    let logs_dir = home.join("background").join("logs");
    fs::create_dir_all(&jobs_dir).expect("create jobs dir");
    fs::create_dir_all(&logs_dir).expect("create logs dir");

    let job_id = "agent-e2e-orphan-local";
    let log_path = logs_dir.join(format!("{job_id}.log"));
    fs::write(&log_path, "partial agent output\n").expect("write agent log");

    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "background agent that got orphaned",
  "cwd": "/tmp",
  "status": "running",
  "created_at": "2026-06-01T00:00:00Z",
  "updated_at": "2026-06-01T00:00:00Z",
  "started_at": "2026-06-01T00:00:00Z",
  "finished_at": null,
  "pid": null,
  "log_path": "{log_path}",
  "error": null,
  "task_kind": "local_agent",
  "tool_use_id": "toolu-e2e",
  "child_session_id": "session-e2e:agent-aaa",
  "agent_type": "general-purpose",
  "model": "claude-sonnet-4-20250514",
  "permission_mode": null,
  "result": null
}}"#,
        log_path = log_path.display(),
    );
    fs::write(jobs_dir.join(format!("{job_id}.json")), &record).expect("write job record");

    // `ps` triggers AppServer construction which calls reconcile_orphaned_agents.
    let output = run_orbcode(&home, scratch.path(), &["ps"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "ps should succeed\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );

    // Verify the on-disk record was updated to orphaned.
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(jobs_dir.join(format!("{job_id}.json"))).expect("reread record"),
    )
    .expect("parse persisted record");
    assert_eq!(
        persisted["status"], "orphaned",
        "local_agent should be marked orphaned: {persisted}"
    );
    assert!(
        persisted["finished_at"].is_string(),
        "finished_at should be stamped: {persisted}"
    );
    assert!(
        persisted["error"]
            .as_str()
            .is_some_and(|msg| msg.contains("orphaned")),
        "error should explain the orphan reason: {persisted}"
    );
}

#[test]
fn completed_local_agent_not_reclassified_on_restart() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let jobs_dir = home.join("background").join("jobs");
    let logs_dir = home.join("background").join("logs");
    fs::create_dir_all(&jobs_dir).expect("create jobs dir");
    fs::create_dir_all(&logs_dir).expect("create logs dir");

    let job_id = "agent-e2e-completed-stable";
    let log_path = logs_dir.join(format!("{job_id}.log"));
    fs::write(&log_path, "agent final output\n").expect("write agent log");

    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "a completed background agent",
  "cwd": "/tmp",
  "status": "completed",
  "created_at": "2026-06-01T00:00:00Z",
  "updated_at": "2026-06-01T00:01:00Z",
  "started_at": "2026-06-01T00:00:00Z",
  "finished_at": "2026-06-01T00:01:00Z",
  "pid": null,
  "log_path": "{log_path}",
  "error": null,
  "task_kind": "local_agent",
  "tool_use_id": "toolu-e2e-2",
  "child_session_id": "session-e2e:agent-bbb",
  "agent_type": "Explore",
  "model": "claude-haiku-4-5",
  "permission_mode": null,
  "result": "agent final output"
}}"#,
        log_path = log_path.display(),
    );
    fs::write(jobs_dir.join(format!("{job_id}.json")), &record).expect("write job record");

    let output = run_orbcode(&home, scratch.path(), &["ps"]);
    assert!(output.status.success());

    // Record should remain completed — not reclassified.
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(jobs_dir.join(format!("{job_id}.json"))).expect("reread record"),
    )
    .expect("parse persisted record");
    assert_eq!(
        persisted["status"], "completed",
        "completed agent must not be reclassified: {persisted}"
    );
}
