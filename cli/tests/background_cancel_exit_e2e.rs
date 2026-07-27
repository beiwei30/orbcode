//! End-to-end tests for background worker exit code / signal recording and
//! SIGTERM-based cancellation of running tools.
//!
//! These tests launch a real `orbcode prompt --bg` process (with a mock
//! provider), then verify that the persisted job record contains the expected
//! `exit_code` and `signal` values.
#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct BgHarness {
    home: PathBuf,
    cwd: tempfile::TempDir,
}

impl BgHarness {
    fn new() -> Self {
        let home_dir = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = home_dir.keep();
        fs::create_dir_all(home.join("background").join("jobs")).expect("jobs dir");
        fs::create_dir_all(home.join("background").join("logs")).expect("logs dir");
        Self { home, cwd }
    }

    fn spawn_bg(&self, base_url: &str, prompt: &str, extra_args: &[&str]) -> (String, String) {
        let mut cmd = Command::new(ORBCODE_BIN);
        cmd.current_dir(self.cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", &self.home)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("ANTHROPIC_BASE_URL", base_url)
            .env("ANTHROPIC_API_KEY", "stub-key")
            .env("RUST_LOG", "warn");
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.arg("prompt").arg("--bg").arg(prompt);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd.output().expect("spawn orbcode prompt --bg");
        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(
            output.status.success(),
            "prompt --bg failed (code {:?}):\nstdout: {stdout}\nstderr: {stderr}",
            output.status.code(),
        );
        (stdout, stderr)
    }

    fn jobs_dir(&self) -> PathBuf {
        self.home.join("background").join("jobs")
    }

    fn find_job_id(&self) -> String {
        for entry in fs::read_dir(self.jobs_dir()).expect("read jobs dir") {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") {
                return name.trim_end_matches(".json").to_string();
            }
        }
        panic!("no job record found in {:?}", self.jobs_dir());
    }

    fn load_job(&self, job_id: &str) -> serde_json::Value {
        let path = self.jobs_dir().join(format!("{job_id}.json"));
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read job record {}: {e}", path.display()));
        serde_json::from_str(&contents)
            .unwrap_or_else(|e| panic!("parse job record {}: {e}", path.display()))
    }
}

fn poll_job_status(harness: &BgHarness, job_id: &str, timeout: Duration) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let record = harness.load_job(job_id);
        let status = record["status"].as_str().unwrap_or("");
        if status != "queued" && status != "running" {
            return record;
        }
        if start.elapsed() > timeout {
            panic!(
                "job {job_id} still {status} after {:.1}s",
                timeout.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn poll_job_pid(harness: &BgHarness, job_id: &str, timeout: Duration) -> u32 {
    let start = Instant::now();
    loop {
        let record = harness.load_job(job_id);
        if let Some(pid) = record["pid"].as_u64()
            && pid > 0
        {
            return pid as u32;
        }
        if start.elapsed() > timeout {
            panic!(
                "job {job_id} has no pid after {:.1}s",
                timeout.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .is_ok_and(|o| o.status.success())
}

fn send_sigterm(pid: u32) {
    Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .output()
        .unwrap_or_else(|e| panic!("kill -TERM {pid}: {e}"));
}

fn poll_worker_started(harness: &BgHarness, job_id: &str, timeout: Duration) {
    let log_path = harness
        .home
        .join("background")
        .join("logs")
        .join(format!("{job_id}.log"));
    let start = Instant::now();
    loop {
        if let Ok(content) = fs::read_to_string(&log_path)
            && content.contains("session ")
        {
            return;
        }
        if start.elapsed() > timeout {
            panic!(
                "bg-worker log never showed startup marker within {:.1}s",
                timeout.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn bg_worker_completed_records_exit_code_zero() {
    let h = BgHarness::new();
    h.spawn_bg("mock://anthropic?scenario=success", "hello", &[]);
    let job_id = h.find_job_id();
    let record = poll_job_status(&h, &job_id, Duration::from_secs(15));

    assert_eq!(
        record["status"].as_str(),
        Some("completed"),
        "job should complete: {record:#}",
    );
    assert_eq!(record["exit_code"], 0, "exit_code should be 0");
    assert!(record["signal"].is_null(), "signal should be null");
}

#[test]
fn bg_worker_error_records_exit_code_one() {
    let h = BgHarness::new();
    h.spawn_bg("mock://anthropic?scenario=fatal", "hello", &[]);
    let job_id = h.find_job_id();
    let record = poll_job_status(&h, &job_id, Duration::from_secs(15));

    assert_eq!(
        record["status"].as_str(),
        Some("failed"),
        "job should fail: {record:#}",
    );
    assert_eq!(record["exit_code"], 1, "exit_code should be 1");
    assert!(record["signal"].is_null(), "signal should be null");
}

#[test]
fn bg_worker_sigterm_records_signal() {
    let h = BgHarness::new();
    h.spawn_bg("mock://anthropic?scenario=hang", "hello", &[]);
    let job_id = h.find_job_id();

    let pid = poll_job_pid(&h, &job_id, Duration::from_secs(10));
    poll_worker_started(&h, &job_id, Duration::from_secs(10));
    assert!(pid_alive(pid), "bg-worker should be alive before SIGTERM");

    send_sigterm(pid);
    let record = poll_job_status(&h, &job_id, Duration::from_secs(10));

    assert_eq!(
        record["status"].as_str(),
        Some("cancelled"),
        "job should be cancelled: {record:#}",
    );
    assert_eq!(record["signal"], 15, "signal should be 15 (SIGTERM)");
}

#[test]
fn bg_worker_sigterm_kills_running_bash() {
    let h = BgHarness::new();
    h.spawn_bg(
        "mock://anthropic?scenario=tool_use&command=sleep+30",
        "run sleep",
        &["--permission-mode", "bypass-permissions"],
    );
    let job_id = h.find_job_id();

    let pid = poll_job_pid(&h, &job_id, Duration::from_secs(10));

    let log_path = h
        .home
        .join("background")
        .join("logs")
        .join(format!("{job_id}.log"));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(log) = fs::read_to_string(&log_path)
            && log.contains("running tool")
        {
            break;
        }
        if Instant::now() > deadline {
            panic!("bash tool never started within 15s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    send_sigterm(pid);
    let record = poll_job_status(&h, &job_id, Duration::from_secs(10));

    assert_eq!(
        record["status"].as_str(),
        Some("cancelled"),
        "job should be cancelled: {record:#}",
    );
    assert_eq!(record["signal"], 15, "signal should be 15 (SIGTERM)");

    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !pid_alive(pid),
        "bg-worker process should be dead after SIGTERM"
    );
}
