//! End-to-end tests verifying that the turn loop rebuilds `TurnContext` before
//! each provider request, so changes to memory sources and git state made
//! during tool execution are reflected in the follow-up request.
//!
//! The print-mode stderr output includes a `context ...` line for every
//! `RequestStarted` event, showing `compact_summary()` which carries the
//! current git branch. The stream-json stdout includes `system` init events
//! that record the session context.

use std::process::{Command, Stdio};

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

fn init_git_repo(path: &std::path::Path) {
    for (args, desc) in [
        (vec!["init", "-q", "-b", "main"], "git init"),
        (
            vec!["config", "user.email", "ci@example.com"],
            "git config email",
        ),
        (vec!["config", "user.name", "CI"], "git config name"),
        (
            vec!["config", "commit.gpgsign", "false"],
            "git config gpgsign",
        ),
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(&args)
            .output()
            .unwrap_or_else(|_| panic!("{desc} spawn"));
        assert!(output.status.success(), "{desc} failed");
    }
    std::fs::write(path.join("file.txt"), "a").expect("write tracked file");
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["add", "file.txt"])
        .output()
        .expect("git add");
    assert!(output.status.success(), "git add failed");
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["commit", "-q", "-m", "init"])
        .output()
        .expect("git commit");
    assert!(output.status.success(), "git commit failed");
}

/// When a bash tool switches git branches during a tool round, the second
/// `RequestStarted` context line on stderr should show the new branch.
///
/// This verifies the full binary path: CLI → AppServer → SessionManager turn
/// loop → per-request `build_turn_context` rebuild.
///
/// Uses the `prompt` subcommand (not `-p`) because `run_headless_prompt`
/// emits `context ...` lines to stderr for each `RequestStarted` event.
#[test]
fn git_branch_refreshed_between_tool_rounds_cli() {
    let harness = Harness::new();
    init_git_repo(harness.cwd.path());

    let (code, _stdout, stderr) = harness.run(&[
        "--permission-mode",
        "bypassPermissions",
        "prompt",
        "#tool:bash {\"command\":\"git checkout -q -b refreshed-branch\"}",
    ]);
    assert_eq!(code, 0, "stderr:\n{stderr}");

    let context_lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("context "))
        .collect();

    assert!(
        context_lines.len() >= 2,
        "expected >= 2 context lines, got {}: {context_lines:?}",
        context_lines.len()
    );
    assert!(
        context_lines[0].contains("branch=main"),
        "first context should show branch=main: {}",
        context_lines[0]
    );
    assert!(
        context_lines[1].contains("branch=refreshed-branch"),
        "second context should show branch=refreshed-branch: {}",
        context_lines[1]
    );
}

/// When CLAUDE.md is modified during a tool round, verify two provider
/// rounds occur (visible as two `context ...` lines on stderr) and the bash
/// tool's side-effect persists.
///
/// The core-crate integration test `context_refreshed_between_tool_rounds`
/// verifies the actual `claude_md` field via `RequestStarted` events; this
/// CLI-level test confirms the binary wiring is intact.
#[test]
fn claude_md_change_during_tool_round_completes_cli() {
    let harness = Harness::new();
    let claude_md_path = harness.cwd.path().join("CLAUDE.md");
    std::fs::write(&claude_md_path, "CLI_E2E_V1").expect("write initial CLAUDE.md");

    let prompt = format!(
        "#tool:bash {}",
        serde_json::json!({"command": format!("printf CLI_E2E_V2 > {}", claude_md_path.display())})
    );
    let (code, _stdout, stderr) =
        harness.run(&["--permission-mode", "bypassPermissions", "prompt", &prompt]);
    assert_eq!(code, 0, "stderr:\n{stderr}");

    let context_lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("context "))
        .collect();
    assert!(
        context_lines.len() >= 2,
        "expected >= 2 context lines (tool round + follow-up), got {}: {context_lines:?}",
        context_lines.len()
    );

    let updated = std::fs::read_to_string(&claude_md_path).expect("read updated CLAUDE.md");
    assert_eq!(
        updated, "CLI_E2E_V2",
        "bash tool should have overwritten CLAUDE.md"
    );
}
