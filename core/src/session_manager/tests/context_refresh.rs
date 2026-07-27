use std::time::Duration as StdDuration;

use orbcode_protocol::{MemorySourceKind, MemorySourceStatus, StreamEvent, TurnContext};

use super::support::test_manager;
use super::*;

/// After a tool round modifies `CLAUDE.md`, the next provider request within
/// the same turn should reflect the updated file content because the turn loop
/// rebuilds `TurnContext` before every provider request.
#[tokio::test]
async fn context_refreshed_between_tool_rounds() {
    let manager = test_manager().await;
    let cwd = manager.config.cwd.clone();

    tokio::fs::write(cwd.join("CLAUDE.md"), "CONTEXT_V1_MARKER")
        .await
        .expect("write initial CLAUDE.md");

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let claude_md_path = cwd.join("CLAUDE.md");
    let prompt = format!(
        "#tool:bash {}",
        serde_json::json!({"command": format!("printf CONTEXT_V2_MARKER > {}", claude_md_path.display())})
    );

    let mut rx = manager
        .submit_turn(&session_id, &prompt)
        .await
        .expect("submit turn");

    let timeout = tokio::time::timeout(StdDuration::from_secs(15), async {
        let mut request_contexts: Vec<TurnContext> = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { context, .. } => {
                    request_contexts.push(context);
                }
                StreamEvent::PermissionRequested { request } => {
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Approve,
                        )
                        .await;
                }
                StreamEvent::TurnFinished { .. } => break,
                StreamEvent::Error { message, .. } => {
                    panic!("unexpected error: {message}");
                }
                _ => {}
            }
        }

        assert!(
            request_contexts.len() >= 2,
            "expected >= 2 RequestStarted events (initial + post-tool), got {}",
            request_contexts.len()
        );

        let first_claude_md = request_contexts[0].claude_md.as_deref().unwrap_or("");
        assert!(
            first_claude_md.contains("CONTEXT_V1_MARKER"),
            "first request should contain initial CLAUDE.md"
        );

        let second_claude_md = request_contexts[1].claude_md.as_deref().unwrap_or("");
        assert!(
            second_claude_md.contains("CONTEXT_V2_MARKER"),
            "second request should reflect updated CLAUDE.md (per-request rebuild)"
        );
    });

    timeout
        .await
        .expect("context refresh between tool rounds timed out");
}

/// When `CLAUDE.md` is modified between two separate turns, the second turn's
/// provider request should use the updated content.
#[tokio::test]
async fn claude_md_change_reflected_between_turns() {
    let manager = test_manager().await;
    let cwd = manager.config.cwd.clone();

    tokio::fs::write(cwd.join("CLAUDE.md"), "BETWEEN_TURNS_V1")
        .await
        .expect("write initial CLAUDE.md");

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut first_context: Option<TurnContext> = None;
    let mut rx = manager
        .submit_turn(&session_id, "first turn")
        .await
        .expect("submit first turn");
    let timeout = tokio::time::timeout(StdDuration::from_secs(15), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { context, .. } if first_context.is_none() => {
                    first_context = Some(context);
                }
                StreamEvent::TurnFinished { .. } | StreamEvent::Error { .. } => break,
                _ => {}
            }
        }
    });
    timeout.await.expect("first turn timed out");

    tokio::fs::write(cwd.join("CLAUDE.md"), "BETWEEN_TURNS_V2")
        .await
        .expect("update CLAUDE.md");

    let mut second_context: Option<TurnContext> = None;
    let mut rx = manager
        .submit_turn(&session_id, "second turn")
        .await
        .expect("submit second turn");
    let timeout = tokio::time::timeout(StdDuration::from_secs(15), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { context, .. } if second_context.is_none() => {
                    second_context = Some(context);
                }
                StreamEvent::TurnFinished { .. } | StreamEvent::Error { .. } => break,
                _ => {}
            }
        }
    });
    timeout.await.expect("second turn timed out");

    let first = first_context.expect("first turn should have RequestStarted");
    let second = second_context.expect("second turn should have RequestStarted");

    assert!(
        first
            .claude_md
            .as_deref()
            .unwrap_or("")
            .contains("BETWEEN_TURNS_V1"),
        "first turn should contain initial CLAUDE.md"
    );
    assert!(
        second
            .claude_md
            .as_deref()
            .unwrap_or("")
            .contains("BETWEEN_TURNS_V2"),
        "second turn should contain updated CLAUDE.md"
    );
}

/// A tool round that creates a new skill file should cause the next provider
/// request's context to include the new skill as a memory source.
#[tokio::test]
async fn skill_addition_reflected_between_tool_rounds() {
    let manager = test_manager().await;
    let home = manager.config.home_dir.clone();

    let skill_dir = home.join("skills").join("test-dynamic-skill");
    let skill_path = skill_dir.join("SKILL.md");

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let prompt = format!(
        "#tool:bash {}",
        serde_json::json!({
            "command": format!(
                "mkdir -p {} && printf '%s' '---\\nname: test-dynamic-skill\\ndescription: dynamically added skill\\n---\\nskill body' > {}",
                skill_dir.display(),
                skill_path.display()
            )
        })
    );

    let mut rx = manager
        .submit_turn(&session_id, &prompt)
        .await
        .expect("submit turn");

    let timeout = tokio::time::timeout(StdDuration::from_secs(15), async {
        let mut request_contexts: Vec<TurnContext> = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { context, .. } => {
                    request_contexts.push(context);
                }
                StreamEvent::PermissionRequested { request } => {
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Approve,
                        )
                        .await;
                }
                StreamEvent::TurnFinished { .. } => break,
                StreamEvent::Error { message, .. } => {
                    panic!("unexpected error: {message}");
                }
                _ => {}
            }
        }

        assert!(
            request_contexts.len() >= 2,
            "expected >= 2 RequestStarted events, got {}",
            request_contexts.len()
        );

        let first_has_skill = request_contexts[0]
            .memory_sources
            .iter()
            .any(|s| s.kind == MemorySourceKind::Skill && s.status == MemorySourceStatus::Loaded);
        assert!(
            !first_has_skill,
            "first request should not have the dynamically added skill"
        );

        let second_has_skill = request_contexts[1].memory_sources.iter().any(|s| {
            s.kind == MemorySourceKind::Skill
                && s.status == MemorySourceStatus::Loaded
                && s.content
                    .as_deref()
                    .is_some_and(|c| c.contains("skill body"))
        });
        assert!(
            second_has_skill,
            "second request should include the dynamically added skill"
        );
    });

    timeout
        .await
        .expect("skill addition between tool rounds timed out");
}

/// After a git branch switch, the context should reflect the new branch name.
#[tokio::test]
async fn git_branch_change_reflected_in_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();

    run_git(cwd, &["init", "-q", "-b", "main"]).await;
    run_git(cwd, &["config", "user.email", "ci@example.com"]).await;
    run_git(cwd, &["config", "user.name", "CI"]).await;
    run_git(cwd, &["config", "commit.gpgsign", "false"]).await;
    tokio::fs::write(cwd.join("file.txt"), "a")
        .await
        .expect("write");
    run_git(cwd, &["add", "file.txt"]).await;
    run_git(cwd, &["commit", "-q", "-m", "init"]).await;

    let ctx1 = crate::context::build_turn_context(cwd).await;
    assert_eq!(ctx1.git_branch.as_deref(), Some("main"));

    run_git(cwd, &["checkout", "-q", "-b", "feature-branch"]).await;

    let ctx2 = crate::context::build_turn_context(cwd).await;
    assert_eq!(ctx2.git_branch.as_deref(), Some("feature-branch"));
}

/// When nothing changes between two context builds, the rendered `claude_md`
/// field should be identical (idempotent).
#[tokio::test]
async fn context_idempotent_when_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();

    tokio::fs::write(cwd.join("CLAUDE.md"), "idempotent content")
        .await
        .expect("write CLAUDE.md");

    let ctx1 = crate::context::build_turn_context(cwd).await;
    let ctx2 = crate::context::build_turn_context(cwd).await;

    assert_eq!(ctx1.claude_md, ctx2.claude_md);
    assert_eq!(ctx1.memory_sources.len(), ctx2.memory_sources.len());
    for (a, b) in ctx1.memory_sources.iter().zip(ctx2.memory_sources.iter()) {
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.status, b.status);
        assert_eq!(a.content, b.content);
    }
}

async fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .expect("git command spawn");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}
