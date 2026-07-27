//! End-to-end tests verifying that `orbcode attach --output-format stream-json`
//! outputs structured NDJSON events from a background worker.
#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

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

    fn spawn_bg(&self, base_url: &str, prompt: &str) -> (String, String) {
        let output = Command::new(ORBCODE_BIN)
            .current_dir(self.cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", &self.home)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("ANTHROPIC_BASE_URL", base_url)
            .env("ANTHROPIC_API_KEY", "stub-key")
            .env("RUST_LOG", "warn")
            .arg("prompt")
            .arg("--bg")
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn orbcode prompt --bg");
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

    fn load_job(&self, job_id: &str) -> Value {
        let path = self.jobs_dir().join(format!("{job_id}.json"));
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read job record {}: {e}", path.display()));
        serde_json::from_str(&contents)
            .unwrap_or_else(|e| panic!("parse job record {}: {e}", path.display()))
    }
}

fn poll_job_terminal(harness: &BgHarness, job_id: &str, timeout: Duration) -> Value {
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

#[test]
fn attach_stream_json_outputs_ndjson() {
    let harness = BgHarness::new();
    harness.spawn_bg("mock://anthropic?scenario=success", "say hello");
    let job_id = harness.find_job_id();

    let record = poll_job_terminal(&harness, &job_id, Duration::from_secs(30));
    assert_eq!(
        record["status"].as_str(),
        Some("completed"),
        "job must complete: {record}"
    );

    let output = Command::new(ORBCODE_BIN)
        .current_dir(harness.cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", &harness.home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &harness.home)
        .env("ANTHROPIC_BASE_URL", "mock://anthropic?scenario=success")
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .args([
            "--output-format",
            "stream-json",
            "--verbose",
            "attach",
            &job_id,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("attach command");

    assert!(
        output.status.success(),
        "attach failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect();

    assert!(
        !lines.is_empty(),
        "attach stream-json must produce at least one record"
    );

    assert_eq!(
        lines[0]["type"], "system",
        "first record must be system/init"
    );
    assert_eq!(lines[0]["subtype"], "init");

    let last = lines.last().expect("at least one record");
    assert_eq!(last["type"], "result", "last record must be result");

    assert!(
        lines
            .iter()
            .any(|r| r["type"] == "stream_event" || r["type"] == "assistant"),
        "stream must contain stream_event or assistant records"
    );
}

#[test]
fn attach_text_still_works() {
    let harness = BgHarness::new();
    harness.spawn_bg("mock://anthropic?scenario=success", "say hello");
    let job_id = harness.find_job_id();

    let record = poll_job_terminal(&harness, &job_id, Duration::from_secs(30));
    assert_eq!(record["status"].as_str(), Some("completed"));

    let output = Command::new(ORBCODE_BIN)
        .current_dir(harness.cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", &harness.home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &harness.home)
        .env("ANTHROPIC_BASE_URL", "mock://anthropic?scenario=success")
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .args(["attach", &job_id])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("attach command");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        !stdout.is_empty(),
        "plain-text attach must produce some output"
    );
    assert!(
        serde_json::from_str::<Value>(&stdout).is_err(),
        "plain-text attach output must not be valid JSON (it's free text)"
    );
}
