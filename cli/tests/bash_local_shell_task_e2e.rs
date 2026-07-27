//! End-to-end coverage that a real Bash run driven through the CLI/app-server
//! tool runtime persists a durable local-shell task record on disk. The stub
//! provider replays a `#tool:bash` directive so the genuine Bash tool path runs
//! (subprocess spawn -> registry streaming -> terminal transition) rather than
//! a mock.

use std::process::Command;

#[test]
fn bash_prompt_persists_local_shell_task_record() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let prompt = r#"#tool:bash {"command":"printf e2e-durable-ok"}"#;

    let output = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env("ORBCODE_ALLOW_TOOLS", "true")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .env_remove("ORBCODE_ALLOWED_TOOLS")
        .env_remove("ORBCODE_DISALLOWED_TOOLS")
        .arg("prompt")
        .arg(prompt)
        .output()
        .expect("run orbcode prompt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "orbcode prompt failed\nstatus: {:?}\nstderr:\n{}\nstdout:\n{}",
        output.status.code(),
        stderr,
        stdout
    );
    assert!(
        stdout.contains("Tool `bash` completed."),
        "stdout should include completed tool result, got:\n{stdout}"
    );

    // The durable registry persists one JSON record per task under the Claude
    // home. Find the record for our command and assert it reached a terminal
    // success state with the streamed bytes accounted for.
    let tasks_dir = home.path().join("local_shell_tasks");
    let record = read_task_record(&tasks_dir, "printf e2e-durable-ok")
        .unwrap_or_else(|| panic!("no local-shell task record found under {tasks_dir:?}"));

    assert_eq!(record["status"], "succeeded");
    assert_eq!(record["command"], "printf e2e-durable-ok");
    let output_bytes = record["output_bytes"].as_u64().expect("output_bytes");
    assert!(
        output_bytes >= "e2e-durable-ok".len() as u64,
        "output_bytes ({output_bytes}) should be at least the command output length ({})",
        "e2e-durable-ok".len()
    );

    // The on-disk log referenced by the record exists and replays the output.
    let log_path = record["log_path"].as_str().expect("log_path present");
    let log = std::fs::read_to_string(log_path).expect("read durable log");
    assert!(
        log.contains("e2e-durable-ok"),
        "durable log should contain command output, got: {log:?}"
    );
}

fn read_task_record(tasks_dir: &std::path::Path, command: &str) -> Option<serde_json::Value> {
    let entries = std::fs::read_dir(tasks_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
            continue;
        };
        if value["command"] == command {
            return Some(value);
        }
    }
    None
}
