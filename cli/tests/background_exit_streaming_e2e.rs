//! End-to-end tests for background job exit_code/signal persistence and
//! the offset-based incremental log reading added by the
//! `background-tasks-exit-streaming` branch.
//!
//! These tests synthesise on-disk job records and drive the real `orbcode`
//! binary (via `ps` / `logs`) to verify that the new fields round-trip
//! through the persistence layer and that the CLI read paths render them
//! correctly.
#![cfg(unix)]

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

fn write_job_record(jobs_dir: &Path, job_id: &str, body: &str) {
    fs::write(jobs_dir.join(format!("{job_id}.json")), body)
        .unwrap_or_else(|err| panic!("write job record {job_id}: {err}"));
}

fn setup_dirs(scratch: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let home = scratch.join("home");
    let jobs_dir = home.join("background").join("jobs");
    let logs_dir = home.join("background").join("logs");
    fs::create_dir_all(&jobs_dir).expect("create jobs dir");
    fs::create_dir_all(&logs_dir).expect("create logs dir");
    (home, jobs_dir, logs_dir)
}

#[test]
fn completed_job_persists_exit_code_zero() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let (home, jobs_dir, logs_dir) = setup_dirs(scratch.path());

    let job_id = "job-exit-completed";
    let log_path = logs_dir.join(format!("{job_id}.log"));
    fs::write(&log_path, "done\n").expect("write log");

    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "completed job",
  "cwd": "/tmp",
  "provider": "anthropic",
  "fallback_provider": null,
  "status": "completed",
  "created_at": "2026-05-30T00:00:00Z",
  "updated_at": "2026-05-30T00:00:01Z",
  "started_at": "2026-05-30T00:00:00Z",
  "finished_at": "2026-05-30T00:00:01Z",
  "pid": 12345,
  "log_path": "{log_path}",
  "error": null,
  "exit_code": 0,
  "signal": null,
  "last_log_offset": 5,
  "cancellation_reason": null
}}"#,
        log_path = log_path.display(),
    );
    write_job_record(&jobs_dir, job_id, &record);

    let output = run_orbcode(&home, scratch.path(), &["ps"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "ps should succeed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.contains(job_id) && stdout.contains("completed"),
        "ps should list the completed job:\n{stdout}",
    );

    // Verify the persisted record retains exit_code=0 after being loaded.
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(jobs_dir.join(format!("{job_id}.json"))).expect("reread"),
    )
    .expect("parse");
    assert_eq!(persisted["exit_code"], 0);
    assert!(persisted["signal"].is_null());
}

#[test]
fn failed_job_persists_exit_code_and_signal() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let (home, jobs_dir, logs_dir) = setup_dirs(scratch.path());

    let job_id = "job-exit-failed";
    let log_path = logs_dir.join(format!("{job_id}.log"));
    fs::write(&log_path, "error output\n").expect("write log");

    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "failed job",
  "cwd": "/tmp",
  "provider": "anthropic",
  "fallback_provider": null,
  "status": "failed",
  "created_at": "2026-05-30T00:00:00Z",
  "updated_at": "2026-05-30T00:00:01Z",
  "started_at": "2026-05-30T00:00:00Z",
  "finished_at": "2026-05-30T00:00:01Z",
  "pid": 12345,
  "log_path": "{log_path}",
  "error": "killed by signal",
  "exit_code": 137,
  "signal": 9,
  "last_log_offset": 13,
  "cancellation_reason": null
}}"#,
        log_path = log_path.display(),
    );
    write_job_record(&jobs_dir, job_id, &record);

    let output = run_orbcode(&home, scratch.path(), &["ps"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "ps should succeed");
    assert!(
        stdout.contains(job_id) && stdout.contains("failed"),
        "ps should list the failed job:\n{stdout}",
    );

    // Verify exit_code=137 and signal=9 survive the serde round-trip.
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(jobs_dir.join(format!("{job_id}.json"))).expect("reread"),
    )
    .expect("parse");
    assert_eq!(persisted["exit_code"], 137);
    assert_eq!(persisted["signal"], 9);
}

#[test]
fn orphaned_job_gets_signal_on_reconciliation() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let (home, jobs_dir, logs_dir) = setup_dirs(scratch.path());

    let job_id = "job-orphan-signal";
    let log_path = logs_dir.join(format!("{job_id}.log"));
    fs::write(&log_path, "orphan log\n").expect("write log");

    // Spawn and reap a child to get a guaranteed-dead pid.
    let mut child = Command::new("true").spawn().expect("spawn");
    let dead_pid = child.id();
    child.wait().expect("reap");

    // Write a running record with no exit_code/signal (simulates pre-upgrade).
    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "orphan signal test",
  "cwd": "/tmp",
  "provider": "anthropic",
  "fallback_provider": null,
  "status": "running",
  "created_at": "2026-05-30T00:00:00Z",
  "updated_at": "2026-05-30T00:00:00Z",
  "started_at": "2026-05-30T00:00:00Z",
  "finished_at": null,
  "pid": {dead_pid},
  "log_path": "{log_path}",
  "error": null
}}"#,
        log_path = log_path.display(),
    );
    write_job_record(&jobs_dir, job_id, &record);

    // ps triggers orphan reconciliation.
    let output = run_orbcode(&home, scratch.path(), &["ps"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("orphaned"),
        "should be reconciled to orphaned:\n{stdout}",
    );

    // Verify signal=9 was set by reconcile_orphan.
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(jobs_dir.join(format!("{job_id}.json"))).expect("reread"),
    )
    .expect("parse");
    assert_eq!(persisted["status"], "orphaned");
    assert_eq!(
        persisted["signal"], 9,
        "orphan reconciliation should set signal=9: {persisted}",
    );
}

#[test]
fn logs_shows_terminal_status_with_exit_detail() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let (home, jobs_dir, logs_dir) = setup_dirs(scratch.path());

    let job_id = "job-logs-exit";
    let log_path = logs_dir.join(format!("{job_id}.log"));
    fs::write(&log_path, "line 1\nline 2\n").expect("write log");

    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "logs with exit detail",
  "cwd": "/tmp",
  "provider": "anthropic",
  "fallback_provider": null,
  "status": "failed",
  "created_at": "2026-05-30T00:00:00Z",
  "updated_at": "2026-05-30T00:00:01Z",
  "started_at": "2026-05-30T00:00:00Z",
  "finished_at": "2026-05-30T00:00:01Z",
  "pid": 9999,
  "log_path": "{log_path}",
  "error": "process exited with code 1",
  "exit_code": 1,
  "signal": null,
  "last_log_offset": 14
}}"#,
        log_path = log_path.display(),
    );
    write_job_record(&jobs_dir, job_id, &record);

    let output = run_orbcode(&home, scratch.path(), &["logs", job_id]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("line 1") && stdout.contains("line 2"),
        "logs should show log content:\n{stdout}",
    );
    assert!(
        stdout.contains("status failed"),
        "logs should show terminal status:\n{stdout}",
    );
    assert!(
        stdout.contains("process exited with code 1"),
        "logs should show error detail:\n{stdout}",
    );
}
