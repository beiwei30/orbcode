//! End-to-end coverage for the durable background job model's orphan
//! detection, driven through the real `orbcode` binary rather than in-process
//! unit tests.
//!
//! A crashed background worker leaves a `status: running` record on disk whose
//! pid no longer refers to a live process. These tests reproduce that on-disk
//! state and assert that the CLI read paths (`ps`, `logs`) reconcile it to a
//! stable terminal `orphaned` state and persist the repair, and that an
//! unrecognized status string round-trips through the forward-compatible
//! `unknown` fallback.
//!
//! Unix only: orphan detection probes liveness via `kill -0`, which is a no-op
//! on non-Unix targets (there we assume the process is alive to avoid falsely
//! orphaning live jobs).
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// Spawn a trivial child, capture its pid, then reap it so the pid is
/// guaranteed to refer to a terminated process. More robust than hard-coding a
/// large constant, which can collide with a live pid on platforms with wide PID
/// ranges (e.g. Linux).
fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().expect("spawn helper process");
    let pid = child.id();
    child.wait().expect("reap helper process");
    pid
}

/// Run the `orbcode` binary against an isolated home directory.
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

#[test]
fn ps_reconciles_crashed_running_job_to_orphaned() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let jobs_dir = home.join("background").join("jobs");
    let logs_dir = home.join("background").join("logs");
    fs::create_dir_all(&jobs_dir).expect("create jobs dir");
    fs::create_dir_all(&logs_dir).expect("create logs dir");

    let job_id = "job-e2e-orphan";
    let log_path = logs_dir.join(format!("{job_id}.log"));
    let log_body = "worker log line 1\n";
    fs::write(&log_path, log_body).expect("write job log");

    let pid = dead_pid();

    // Legacy on-disk record: status=running with a dead pid, and crucially
    // WITHOUT the new exit_code/signal/last_log_offset/cancellation_reason
    // fields. This also exercises serde backward-compatibility on the read path.
    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "simulated crashed worker",
  "cwd": "/tmp",
  "provider": "anthropic",
  "fallback_provider": null,
  "status": "running",
  "created_at": "2026-05-30T00:00:00Z",
  "updated_at": "2026-05-30T00:00:00Z",
  "started_at": "2026-05-30T00:00:00Z",
  "finished_at": null,
  "pid": {pid},
  "log_path": "{log_path}",
  "error": null
}}"#,
        log_path = log_path.display(),
    );
    write_job_record(&jobs_dir, job_id, &record);

    // First `ps` lists jobs, which triggers orphan reconciliation + persistence.
    let output = run_orbcode(&home, scratch.path(), &["ps"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "ps should succeed\nstatus: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stdout.contains(job_id) && stdout.contains("orphaned"),
        "ps should report the crashed job as orphaned:\n{stdout}",
    );

    // The record was rewritten to a terminal state and upgraded with the new
    // fields (serde defaults filled on read, then populated on the orphan
    // transition).
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(jobs_dir.join(format!("{job_id}.json"))).expect("reread record"),
    )
    .expect("parse persisted record");
    assert_eq!(persisted["status"], "orphaned");
    assert!(
        persisted["finished_at"].is_string(),
        "finished_at should be stamped: {persisted}",
    );
    assert!(
        persisted["error"]
            .as_str()
            .is_some_and(|detail| detail.contains("no longer alive")),
        "error should explain the orphan reason: {persisted}",
    );
    assert_eq!(
        persisted["last_log_offset"].as_u64(),
        Some(log_body.len() as u64),
        "last_log_offset should equal the log byte length: {persisted}",
    );
    // New fields exist (defaulted) even though the input JSON omitted them.
    assert!(
        persisted.get("exit_code").is_some(),
        "exit_code field present"
    );
    assert!(persisted.get("signal").is_some(), "signal field present");
    assert!(
        persisted.get("cancellation_reason").is_some(),
        "cancellation_reason field present",
    );

    // `logs` reports the terminal status and the orphan detail, and does not
    // treat the job as active.
    let logs = run_orbcode(&home, scratch.path(), &["logs", job_id]);
    let logs_out = String::from_utf8_lossy(&logs.stdout);
    assert!(
        logs_out.contains("worker log line 1"),
        "logs should print captured output:\n{logs_out}",
    );
    assert!(
        logs_out.contains("status orphaned"),
        "logs should report terminal status:\n{logs_out}",
    );

    // A fresh process keeps the persisted terminal state; it is not re-processed.
    let again = run_orbcode(&home, scratch.path(), &["ps"]);
    let again_out = String::from_utf8_lossy(&again.stdout);
    assert!(
        again_out.contains(job_id) && again_out.contains("orphaned"),
        "orphaned terminal state should survive reload:\n{again_out}",
    );
}

#[test]
fn ps_renders_unrecognized_status_as_unknown() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let jobs_dir = home.join("background").join("jobs");
    let logs_dir = home.join("background").join("logs");
    fs::create_dir_all(&jobs_dir).expect("create jobs dir");
    fs::create_dir_all(&logs_dir).expect("create logs dir");

    let job_id = "job-e2e-future";
    // A status this build does not recognize must deserialize to `unknown`
    // (forward compatibility) rather than failing the whole listing.
    let record = format!(
        r#"{{
  "job_id": "{job_id}",
  "session_id": "session-e2e",
  "prompt": "from a newer build",
  "cwd": "/tmp",
  "provider": "anthropic",
  "fallback_provider": null,
  "status": "some_future_state",
  "created_at": "2026-05-30T00:00:00Z",
  "updated_at": "2026-05-30T00:00:00Z",
  "started_at": null,
  "finished_at": null,
  "pid": null,
  "log_path": "{log_path}",
  "error": null
}}"#,
        log_path = logs_dir.join(format!("{job_id}.log")).display(),
    );
    write_job_record(&jobs_dir, job_id, &record);

    let output = run_orbcode(&home, scratch.path(), &["ps"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "ps should succeed even with an unrecognized status:\n{stdout}",
    );
    assert!(
        stdout.contains(job_id) && stdout.contains("unknown"),
        "ps should render an unrecognized status as unknown:\n{stdout}",
    );
}
