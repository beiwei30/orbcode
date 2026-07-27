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

    fn write_user_settings(&self, json: &str) {
        std::fs::write(self.home_path.join("settings.json"), json).expect("write user settings");
    }

    fn write_project_agent(&self, file_name: &str, contents: &str) {
        let dir = self.cwd.path().join(".claude").join("agents");
        std::fs::create_dir_all(&dir).expect("create .claude/agents");
        std::fs::write(dir.join(file_name), contents).expect("write agent definition");
    }

    fn run_with_env(&self, args: &[&str], extra: &[(&str, &str)]) -> (i32, String, String) {
        let mut cmd = Command::new(ORBCODE_BIN);
        cmd.args(args)
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
        for (key, value) in extra {
            cmd.env(key, value);
        }
        let output = cmd.output().expect("spawn orbcode");
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

fn tool_result_text(records: &[Value]) -> String {
    for record in records {
        if record["type"] != "user" {
            continue;
        }
        let Some(blocks) = record["message"]["content"].as_array() else {
            continue;
        };
        for block in blocks {
            if block["type"] == "tool_result" {
                if let Some(text) = block["content"].as_str() {
                    return text.to_string();
                }
                if let Some(items) = block["content"].as_array() {
                    return items
                        .iter()
                        .filter_map(|item| item["text"].as_str().or_else(|| item.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
        }
    }
    String::new()
}

fn agent_definition(cwd: &std::path::Path, marker_name: &str) -> String {
    let marker = cwd.join(marker_name);
    let marker_display = marker.display();
    format!(
        concat!(
            "---\n",
            "name: scoped-explorer\n",
            "description: Locked-down explorer used in subagent lifecycle integration tests.\n",
            "tools: Read\n",
            "hooks: {{\"SubagentStart\":[{{\"hooks\":[{{\"type\":\"command\",",
            "\"command\":\"printf '%s' 'CHILD_HOOK_FIRED' > '{marker}'; ",
            "printf '%s' '{{\\\"hookSpecificOutput\\\":{{\\\"hookEventName\\\":\\\"SubagentStart\\\",",
            "\\\"additionalContext\\\":\\\"agent overlay context\\\"}}}}'\",",
            "\"timeout\":5.0}}]}}]}}\n",
            "---\n",
            "You are scoped-explorer. Respond briefly.\n"
        ),
        marker = marker_display
    )
}

fn agent_tool_invocation(prompt: &str) -> String {
    format!(
        r#"#tool:Agent {{"description":"e2e","prompt":{prompt:?},"subagent_type":"scoped-explorer"}}"#
    )
}

fn common_args(prompt: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        prompt.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--permission-mode".to_string(),
        "acceptEdits".to_string(),
        "--allowed-tools".to_string(),
        "Agent,Read,Bash".to_string(),
    ]
}

fn args_refs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

#[test]
fn agent_frontmatter_subagent_start_hook_fires_in_child_loop() {
    let harness = Harness::new();
    let marker_name = "child-subagent-start.marker";
    harness.write_project_agent(
        "scoped-explorer.md",
        &agent_definition(harness.cwd.path(), marker_name),
    );

    let prompt = agent_tool_invocation("just say done");
    let args = common_args(&prompt);
    let (code, stdout, stderr) =
        harness.run_with_env(&args_refs(&args), &[("ORBCODE_TRUSTED_PROJECT", "1")]);
    assert_eq!(
        code, 0,
        "orbcode should exit 0\nstderr: {stderr}\nstdout:\n{stdout}"
    );

    let marker_path = harness.cwd.path().join(marker_name);
    let marker_body = std::fs::read_to_string(&marker_path).unwrap_or_else(|err| {
        panic!(
            "expected SubagentStart hook to write {}: {err}\nstdout:\n{stdout}",
            marker_path.display()
        )
    });
    assert!(
        marker_body.contains("CHILD_HOOK_FIRED"),
        "marker missing payload: {marker_body:?}"
    );

    let records = parse_lines(&stdout);
    let agent_result = tool_result_text(&records);
    assert!(
        !agent_result.is_empty(),
        "expected an Agent tool_result block in: {records:#?}"
    );
}

#[test]
fn agent_frontmatter_hooks_do_not_leak_into_parent_stop_hook_table() {
    let harness = Harness::new();
    let child_marker = "child-subagent-start.marker";
    let parent_stop_marker = harness.cwd.path().join("parent-stop.marker");
    let parent_stop_display = parent_stop_marker.display().to_string();

    // Parent has its own Stop hook in user settings (not subject to the
    // trusted-project filter). Agent declares a SubagentStart hook. If the
    // overlay leaks into the parent table, the parent's Stop hook would also
    // see a SubagentStart entry — instead the parent's marker file should
    // only ever be written by the explicit Stop hook command below.
    harness.write_user_settings(&format!(
        r#"{{
            "env":{{"ANTHROPIC_BASE_URL":"stub://test","ANTHROPIC_API_KEY":"stub-key"}},
            "hooks":{{
                "Stop":[{{"hooks":[{{"type":"command","command":"printf '%s' 'PARENT_STOP_FIRED' > '{parent_stop_display}'","timeout":5.0}}]}}]
            }}
        }}"#
    ));
    harness.write_project_agent(
        "scoped-explorer.md",
        &agent_definition(harness.cwd.path(), child_marker),
    );

    let prompt = agent_tool_invocation("just say done");
    let args = common_args(&prompt);
    let (code, stdout, stderr) =
        harness.run_with_env(&args_refs(&args), &[("ORBCODE_TRUSTED_PROJECT", "1")]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let child_marker_path = harness.cwd.path().join(child_marker);
    assert!(
        child_marker_path.exists(),
        "agent's SubagentStart hook must have fired"
    );
    let parent_body = std::fs::read_to_string(&parent_stop_marker)
        .expect("parent's Stop hook must have written its marker");
    assert_eq!(
        parent_body.trim(),
        "PARENT_STOP_FIRED",
        "parent's Stop marker should reflect ONLY the parent's hook command; the agent's hook payload must not leak into the parent hook table"
    );
    assert!(
        !parent_body.contains("CHILD_HOOK_FIRED"),
        "parent's Stop hook command must not be replaced or supplemented by the agent's hook overlay: {parent_body}"
    );
}

#[test]
fn untrusted_project_skips_agent_frontmatter_hooks() {
    let harness = Harness::new();
    let marker_name = "child-subagent-start.marker";
    harness.write_project_agent(
        "scoped-explorer.md",
        &agent_definition(harness.cwd.path(), marker_name),
    );

    let prompt = agent_tool_invocation("just say done");
    let args = common_args(&prompt);
    let (code, stdout, stderr) =
        harness.run_with_env(&args_refs(&args), &[("ORBCODE_TRUSTED_PROJECT", "0")]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let marker_path = harness.cwd.path().join(marker_name);
    assert!(
        !marker_path.exists(),
        "project-sourced agent hooks must be filtered when project is untrusted; got marker at {}",
        marker_path.display()
    );

    let records = parse_lines(&stdout);
    let agent_result = tool_result_text(&records);
    assert!(
        !agent_result.contains("agent overlay context"),
        "untrusted-project agent hooks must not contribute additionalContext: {agent_result}"
    );
}

#[test]
fn nested_agent_tool_use_is_rejected_inside_child_loop() {
    let harness = Harness::new();
    let marker_name = "child-subagent-start-nested.marker";
    harness.write_project_agent(
        "scoped-explorer.md",
        &agent_definition(harness.cwd.path(), marker_name),
    );

    // Child's prompt itself contains a #tool:Agent directive — the stub
    // provider will emit an Agent tool_use from inside the child loop, which
    // must be rejected by invoke_nested_agent_tool.
    let inner_invocation =
        r#"#tool:Agent {"description":"nested","prompt":"x","subagent_type":"scoped-explorer"}"#;
    let prompt = agent_tool_invocation(inner_invocation);
    let args = common_args(&prompt);
    let (code, stdout, stderr) =
        harness.run_with_env(&args_refs(&args), &[("ORBCODE_TRUSTED_PROJECT", "1")]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout:\n{stdout}");

    let records = parse_lines(&stdout);
    let agent_result = tool_result_text(&records);
    assert!(
        agent_result.contains("nested Agent tool use is not supported"),
        "expected nested-Agent rejection text to surface in parent tool_result, got: {agent_result}\nstdout:\n{stdout}"
    );
}
