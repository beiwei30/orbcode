//! End-to-end tests for TaskCreate / TaskList / TaskUpdate through the
//! headless CLI.  The stub provider's `#tool:` / `#then:` directive parsing
//! drives the tool-use rounds without a real LLM.

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
        let home_path = home.path().to_path_buf();
        Self {
            _home: home,
            cwd,
            home_path,
        }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let output = Command::new(ORBCODE_BIN)
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
            .stderr(Stdio::piped())
            .output()
            .expect("spawn orbcode");
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

fn extract_tool_result_contents(records: &[Value]) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for record in records {
        if record["type"] != "user" {
            continue;
        }
        let Some(blocks) = record["message"]["content"].as_array() else {
            continue;
        };
        for block in blocks {
            if block["type"] != "tool_result" {
                continue;
            }
            let tool_use_id = block["tool_use_id"].as_str().unwrap_or("").to_string();
            let content = if let Some(arr) = block["content"].as_array() {
                arr.iter()
                    .filter_map(|c| c["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            } else {
                block["content"].as_str().unwrap_or("").to_string()
            };
            results.push((tool_use_id, content));
        }
    }
    results
}

#[test]
fn task_create_and_list_round_trip_produces_ts_compatible_output() {
    let harness = Harness::new();
    let prompt = concat!(
        r#"#tool:TaskCreate {"subject":"Alpha","description":"First task"}"#,
        r#" #then:TaskList {}"#,
    );
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
        "--allowed-tools",
        "task-create,task-list",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let tool_results = extract_tool_result_contents(&records);
    assert!(
        tool_results.len() >= 2,
        "expected at least 2 tool results (TaskCreate + TaskList), got {}",
        tool_results.len()
    );

    let create_output = &tool_results[0].1;
    assert_eq!(
        create_output, "Task #1 created successfully: Alpha",
        "TaskCreate output must match TS format"
    );

    let list_output = &tool_results[1].1;
    let list_json: Value =
        serde_json::from_str(list_output).expect("TaskList output must be valid JSON");
    let tasks = list_json["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], "1");
    assert_eq!(tasks[0]["subject"], "Alpha");
    assert_eq!(tasks[0]["status"], "pending");
    assert_eq!(tasks[0]["owner"], "");
    assert_eq!(tasks[0]["blockedBy"], Value::Array(vec![]));

    let task_keys: std::collections::BTreeSet<&str> = tasks[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected_keys: std::collections::BTreeSet<&str> =
        ["id", "subject", "status", "owner", "blockedBy"]
            .into_iter()
            .collect();
    assert_eq!(
        task_keys, expected_keys,
        "TaskList task objects must contain exactly the TS fields"
    );
}

#[test]
fn task_update_changes_status_and_list_reflects_it() {
    let harness = Harness::new();
    let prompt = concat!(
        r#"#tool:TaskCreate {"subject":"Beta","description":"Second task"}"#,
        r#" #then:TaskUpdate {"taskId":"1","status":"in_progress","owner":"agent-1"}"#,
        r#" #then:TaskList {}"#,
    );
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
        "--allowed-tools",
        "task-create,task-update,task-list",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let tool_results = extract_tool_result_contents(&records);
    assert!(
        tool_results.len() >= 3,
        "expected 3 tool results, got {}",
        tool_results.len()
    );

    let update_output = &tool_results[1].1;
    assert!(
        update_output.starts_with("Updated task #1"),
        "TaskUpdate output must start with TS format prefix, got: {update_output}"
    );
    assert!(
        update_output.contains("status") && update_output.contains("owner"),
        "TaskUpdate output must list changed fields, got: {update_output}"
    );

    let list_output = &tool_results[2].1;
    let list_json: Value =
        serde_json::from_str(list_output).expect("TaskList output must be valid JSON");
    let tasks = list_json["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks[0]["status"], "in_progress");
    assert_eq!(tasks[0]["owner"], "agent-1");
}

#[test]
fn task_update_state_transition_error_surfaces_in_tool_result() {
    let harness = Harness::new();
    let prompt = concat!(
        r#"#tool:TaskCreate {"subject":"Done","description":"Will complete"}"#,
        r#" #then:TaskUpdate {"taskId":"1","status":"completed"}"#,
        r#" #then:TaskUpdate {"taskId":"1","status":"in_progress"}"#,
    );
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
        "--allowed-tools",
        "task-create,task-update",
    ]);
    // The run may succeed (exit 0) even when a tool returns an error — the
    // error is surfaced inside the tool_result, not as a top-level failure.
    let _ = (code, stderr);

    let records = parse_lines(&stdout);
    let tool_results = extract_tool_result_contents(&records);
    assert!(
        tool_results.len() >= 3,
        "expected 3 tool results, got {}",
        tool_results.len()
    );

    let error_output = &tool_results[2].1;
    assert!(
        error_output.contains("Cannot transition task from completed to in_progress"),
        "expected TS-compatible state transition error, got: {error_output}"
    );
}

#[test]
fn task_list_filters_completed_blockers_in_blocked_by() {
    let harness = Harness::new();
    let prompt = concat!(
        r#"#tool:TaskCreate {"subject":"Prereq","description":"Must finish first"}"#,
        r#" #then:TaskCreate {"subject":"Dependent","description":"Depends on prereq"}"#,
        r#" #then:TaskUpdate {"taskId":"2","addBlockedBy":["1"]}"#,
        r#" #then:TaskList {}"#,
        r#" #then:TaskUpdate {"taskId":"1","status":"completed"}"#,
        r#" #then:TaskList {}"#,
    );
    let (code, stdout, stderr) = harness.run(&[
        "-p",
        prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "default",
        "--allowed-tools",
        "task-create,task-update,task-list",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let tool_results = extract_tool_result_contents(&records);

    // 4th result is the first TaskList (before completing prereq)
    let list_before: Value =
        serde_json::from_str(&tool_results[3].1).expect("parse first TaskList");
    let task2_before = list_before["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "2")
        .expect("task #2 in first list");
    assert_eq!(
        task2_before["blockedBy"],
        serde_json::json!(["1"]),
        "before completion, blockedBy should include task #1"
    );

    // 6th result is the second TaskList (after completing prereq)
    let list_after: Value =
        serde_json::from_str(&tool_results[5].1).expect("parse second TaskList");
    let task2_after = list_after["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "2")
        .expect("task #2 in second list");
    assert_eq!(
        task2_after["blockedBy"],
        serde_json::json!([]),
        "after completion, blockedBy should filter out the completed blocker"
    );
}
