#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use orbcode_mcp::McpRegistry;
use orbcode_tools::{FileReadState, SandboxMode, ToolCancellationToken, ToolContext};

fn test_root(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "orbcode-golden-{label}-{}-{unique}",
        std::process::id()
    ))
}

pub async fn test_context(label: &str) -> ToolContext {
    let root = test_root(label);
    let cwd = root.join("cwd");
    let home = root.join("home");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    std::fs::create_dir_all(&home).expect("create home");
    let mcp = McpRegistry::load(&home, &cwd).await.expect("load mcp");
    ToolContext {
        cwd,
        additional_directories: Vec::new(),
        home_dir: home,
        sandbox_mode: SandboxMode::DangerFullAccess,
        sandbox_allow_network: true,
        allow_network: true,
        allow_tools: true,
        mcp,
        progress: None,
        cancellation: ToolCancellationToken::default(),
        read_state: None,
        session_id: Some(label.to_string()),
        local_shell_tasks: None,
        on_cwd_change: None,
        plans_directory_override: None,
        ask_user_tx: None,
        settings_env: std::collections::BTreeMap::new(),
        skill_definitions: None,
    }
}

pub async fn context_with_read_state(label: &str) -> ToolContext {
    let mut ctx = test_context(label).await;
    ctx.read_state = Some(Arc::new(FileReadState::new()));
    ctx
}

pub fn advance_mtime(path: &std::path::Path, secs: u64) {
    let future = SystemTime::now() + std::time::Duration::from_secs(secs);
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open for mtime bump");
    file.set_modified(future).expect("set mtime");
}

pub fn init_git_repo(cwd: &std::path::Path) {
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(cwd)
        .output()
        .expect("git init");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(cwd)
        .output()
        .expect("git config email");
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(cwd)
        .output()
        .expect("git config name");
}
