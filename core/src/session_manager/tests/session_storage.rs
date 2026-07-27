use std::path::PathBuf;

use orbcode_protocol::EffortLevel;

use super::support::*;
use super::*;

async fn write_parent_transcript(manager: &SessionManager, session_id: &str) {
    let payload = serde_json::to_string(&json!({
        "type": "user",
        "uuid": format!("{session_id}-user"),
        "timestamp": "2026-04-10T00:00:00.000Z",
        "message": { "role": "user", "content": "parent prompt" },
        "cwd": manager.config.cwd.display().to_string(),
        "sessionId": session_id,
    }))
    .expect("serialize parent transcript");
    tokio::fs::write(
        manager.transcript_store.path(session_id),
        format!("{payload}\n"),
    )
    .await
    .expect("write parent transcript");
}

async fn write_workflow_child_artifacts(
    manager: &SessionManager,
    parent_session_id: &str,
    child_session_id: &str,
) -> PathBuf {
    manager
        .child_session_store
        .start(orbcode_session_store::StartChildSessionInput {
            child_session_id: child_session_id.to_string(),
            parent_session_id: parent_session_id.to_string(),
            agent_id: "agent-cleanup".to_string(),
            agent_type: "general-purpose".to_string(),
            source_tool_use_id: "workflow:cleanup:step.0".to_string(),
            cwd: manager.config.cwd.display().to_string(),
            model: Some("test-model".to_string()),
            permission_mode: None,
            prompt: "cleanup child".to_string(),
        })
        .await
        .expect("start child session");
    let transcript_path = manager
        .child_session_store
        .transcript_path_for(child_session_id);
    tokio::fs::create_dir_all(transcript_path.parent().unwrap())
        .await
        .expect("child transcript dir");
    let payload = serde_json::to_string(&json!({
        "type": "user",
        "uuid": format!("{child_session_id}-user"),
        "timestamp": "2026-04-10T00:00:01.000Z",
        "message": { "role": "user", "content": "cleanup child prompt" },
        "cwd": manager.config.cwd.display().to_string(),
        "sessionId": child_session_id,
    }))
    .expect("serialize child transcript");
    tokio::fs::write(&transcript_path, format!("{payload}\n"))
        .await
        .expect("write child transcript");
    transcript_path
}

async fn set_child_last_activity(
    manager: &SessionManager,
    child_session_id: &str,
    last_activity_at: i64,
) {
    let path = manager
        .child_session_store
        .root()
        .join(format!("{child_session_id}.json"));
    let contents = tokio::fs::read_to_string(&path)
        .await
        .expect("read child metadata");
    let mut metadata: orbcode_session_store::ChildSessionMetadata =
        serde_json::from_str(&contents).expect("parse child metadata");
    metadata.last_activity_at = last_activity_at;
    tokio::fs::write(
        &path,
        serde_json::to_string_pretty(&metadata).expect("serialize child metadata"),
    )
    .await
    .expect("write child metadata");
}

#[tokio::test]
async fn persists_and_resumes_sessions() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "phase two")
        .await
        .expect("submit turn");

    let mut normalized = Vec::new();
    while let Some(event) = rx.recv().await {
        normalized.push(event.normalize());
        if matches!(
            normalized.last().map(|event| event.kind.as_str()),
            Some("turn_finished")
        ) {
            break;
        }
    }

    let resumed = manager
        .load_session(&session_id)
        .await
        .expect("load saved session");
    assert_eq!(resumed.messages.len(), 2);
    assert!(
        normalized
            .iter()
            .any(|event| event.kind == "request_started")
    );
    assert!(
        normalized
            .iter()
            .any(|event| event.kind == "assistant_completed")
    );
}

#[tokio::test]
async fn detached_interrupt_during_bash_allows_next_turn_without_stale_marker() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut first_rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"sleep 10"}"#)
        .await
        .expect("submit first turn");

    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = first_rx.recv().await {
            if matches!(event, StreamEvent::ToolUseStarted { .. }) {
                return;
            }
        }
        panic!("first turn ended before bash started");
    })
    .await
    .expect("bash tool should start");

    assert!(manager.interrupt_turn(&session_id).await);
    let mut second_rx = manager
        .submit_turn(&session_id, "new turn after bash interrupt")
        .await
        .expect("detached interrupt should free the active-turn slot");

    let (saw_second_user, second_finished) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut saw_second_user = false;
        while let Some(event) = second_rx.recv().await {
            match event {
                StreamEvent::UserMessage { message }
                    if message.content == "new turn after bash interrupt" =>
                {
                    saw_second_user = true;
                }
                StreamEvent::TurnFinished { .. } => return (saw_second_user, true),
                _ => {}
            }
        }
        (saw_second_user, false)
    })
    .await
    .expect("second turn should not wait for old bash");

    assert!(saw_second_user);
    assert!(second_finished);
    drop(first_rx);

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    let transcript = tokio::fs::read_to_string(manager.transcript_store.path(&session_id))
        .await
        .expect("read transcript");
    for (line_index, line) in transcript.lines().enumerate() {
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|error| panic!("invalid transcript json line {line_index}: {error}"));
    }
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content == "new turn after bash interrupt"),
        "detached interrupt follow-up prompt should survive concurrent old-turn cleanup"
    );
    assert!(
        !saved.messages.iter().any(|message| {
            message.content == INTERRUPTED_TURN_MESSAGE
                || message.content == INTERRUPTED_TURN_MESSAGE_FOR_TOOL_USE
        }),
        "detached UI interrupt should not inject stale interruption markers before the next user turn"
    );
}

#[tokio::test]
async fn forks_existing_sessions() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "original prompt"),
        )
        .await
        .expect("append user");

    let fork = manager
        .fork_session(
            &session_id,
            Some("forked title".to_string()),
            Some("new note".to_string()),
        )
        .await
        .expect("fork session");

    assert_ne!(fork.session_id, session_id);
    assert_eq!(fork.title.as_deref(), Some("forked title"));
    assert!(
        fork.messages
            .iter()
            .any(|message| message.content == "original prompt")
    );
    assert!(
        fork.messages
            .iter()
            .any(|message| message.content.contains("Forked from session"))
    );
}

#[tokio::test]
async fn rename_session_updates_summary_title_without_losing_auto_title() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "investigate registry"),
        )
        .await
        .expect("append user");

    manager
        .rename_session(&session_id, "Session registry sweep")
        .await
        .expect("rename session");

    let summaries = manager.list_sessions().await.expect("list sessions");
    let summary = summaries
        .iter()
        .find(|summary| summary.session_id == session_id)
        .expect("renamed session present");
    assert_eq!(summary.title.as_deref(), Some("Session registry sweep"));
    assert_eq!(
        summary.custom_title.as_deref(),
        Some("Session registry sweep")
    );
    assert_eq!(
        manager
            .session_id_for_exact_custom_title("session registry sweep")
            .await
            .expect("find by title")
            .as_deref(),
        Some(session_id.as_str())
    );
    manager
        .rename_session("other-session-id", "Session registry sweep")
        .await
        .expect("rename duplicate title");
    let duplicate_error = manager
        .session_id_for_exact_custom_title("Session registry sweep")
        .await
        .expect_err("duplicate custom titles should be ambiguous");
    assert!(
        duplicate_error
            .to_string()
            .contains("multiple sessions match")
    );

    let reloaded = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    assert_eq!(
        reloaded.custom_title.as_deref(),
        Some("Session registry sweep")
    );
    // Auto title from the first user message is preserved alongside the rename.
    assert!(reloaded.title.is_some());
    assert_ne!(reloaded.title.as_deref(), Some("Session registry sweep"));
}

#[tokio::test]
async fn rename_session_rejects_empty_titles_but_materializes_unrecorded_sessions() {
    let manager = test_manager().await;

    // Renaming a session id that has no transcript yet (e.g. the session
    // the TUI just bootstrapped before any message has been recorded)
    // appends a `custom-title` row that materializes the transcript so the
    // session shows up in `/sessions` and remains resumable instead of
    // being dropped on the floor.
    let fresh_session_id = "fresh-session-id";
    manager
        .rename_session(fresh_session_id, "Plan kickoff")
        .await
        .expect("rename of unrecorded session must materialize the transcript");
    let summaries = manager.list_sessions().await.expect("list sessions");
    let summary = summaries
        .iter()
        .find(|summary| summary.session_id == fresh_session_id)
        .expect("freshly renamed session must appear in /sessions");
    assert_eq!(summary.title.as_deref(), Some("Plan kickoff"));

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "hello"),
        )
        .await
        .expect("append user");

    let empty_error = manager
        .rename_session(&session_id, "   ")
        .await
        .expect_err("empty titles must be rejected");
    assert!(matches!(empty_error, crate::CoreError::Config(_)));
}

#[tokio::test]
async fn list_sessions_surfaces_corrupt_transcripts_with_path_and_status() {
    use orbcode_protocol::SessionStatus;

    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "real session"),
        )
        .await
        .expect("append user");

    let project_dir = manager.transcript_store.path(&session_id);
    let project_dir = project_dir.parent().expect("project dir");
    tokio::fs::write(project_dir.join("broken-session.jsonl"), "not-json\n")
        .await
        .expect("write corrupt transcript");

    let summaries = manager.list_sessions().await.expect("list sessions");
    let corrupt = summaries
        .iter()
        .find(|summary| summary.session_id == "broken-session")
        .expect("corrupt summary present");
    assert!(matches!(corrupt.status, SessionStatus::Corrupt { .. }));
    assert!(
        corrupt
            .transcript_path
            .as_deref()
            .is_some_and(|path| path.ends_with("broken-session.jsonl"))
    );
    let healthy = summaries
        .iter()
        .find(|summary| summary.session_id == session_id)
        .expect("healthy summary present");
    assert!(matches!(healthy.status, SessionStatus::Available));
}

#[tokio::test]
async fn continue_latest_resumes_newest_available_current_project_session() {
    let manager = test_manager().await;
    let older_session_id = "older-session";
    let newer_session_id = "newer-session";
    let mut older = TranscriptMessage::new(MessageRole::User, "older prompt");
    older.created_at = Utc::now() - ChronoDuration::seconds(30);
    let mut newer = TranscriptMessage::new(MessageRole::User, "newer prompt");
    newer.created_at = Utc::now();

    manager
        .append_message(older_session_id, older)
        .await
        .expect("append older session");
    manager
        .append_message(newer_session_id, newer)
        .await
        .expect("append newer session");
    tokio::fs::write(
        manager
            .config
            .current_project_dir
            .join("corrupt-session.jsonl"),
        "not-json\n",
    )
    .await
    .expect("write corrupt transcript");

    let (session, event) = manager.continue_latest().await.expect("continue latest");

    assert_eq!(session.session_id, newer_session_id);
    assert!(matches!(event, StreamEvent::SessionLoaded { .. }));
}

#[tokio::test]
async fn continue_latest_includes_same_repo_worktree_sessions() {
    let mut manager = test_manager().await;
    let main_cwd = manager.config.home_dir.join("repo-main");
    let linked_cwd = manager.config.home_dir.join("repo-linked");
    tokio::fs::create_dir_all(&main_cwd)
        .await
        .expect("create main repo");

    let git = |cwd: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&main_cwd, &["init"]);
    git(&main_cwd, &["config", "user.email", "test@example.com"]);
    git(&main_cwd, &["config", "user.name", "Test User"]);
    git(&main_cwd, &["commit", "--allow-empty", "-m", "initial"]);
    git(
        &main_cwd,
        &[
            "worktree",
            "add",
            linked_cwd.to_str().expect("linked path"),
            "-b",
            "linked-test",
        ],
    );

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&main_cwd)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("list worktrees");
    assert!(output.status.success());
    let worktree_paths = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let listed_main_cwd = worktree_paths
        .iter()
        .find(|path| path.file_name() == main_cwd.file_name())
        .cloned()
        .unwrap_or_else(|| main_cwd.clone());
    let listed_linked_cwd = worktree_paths
        .iter()
        .find(|path| path.file_name() == linked_cwd.file_name())
        .cloned()
        .unwrap_or_else(|| linked_cwd.clone());

    manager.config.cwd = listed_main_cwd.clone();
    let main_project_dir = manager
        .config
        .projects_dir
        .join(orbcode_config::sanitize_path(
            &listed_main_cwd.display().to_string(),
        ));
    let linked_project_dir = manager
        .config
        .projects_dir
        .join(orbcode_config::sanitize_path(
            &listed_linked_cwd.display().to_string(),
        ));
    tokio::fs::create_dir_all(&main_project_dir)
        .await
        .expect("create main project dir");
    tokio::fs::create_dir_all(&linked_project_dir)
        .await
        .expect("create linked project dir");

    let write_user = |dir: PathBuf,
                      session_id: &'static str,
                      timestamp: &'static str,
                      cwd: PathBuf| async move {
        let payload = serde_json::to_string(&json!({
            "type": "user",
            "uuid": format!("{session_id}-user"),
            "timestamp": timestamp,
            "message": { "role": "user", "content": session_id },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        }))
        .expect("serialize transcript");
        tokio::fs::write(
            dir.join(format!("{session_id}.jsonl")),
            format!("{payload}\n"),
        )
        .await
        .expect("write transcript");
    };
    write_user(
        main_project_dir,
        "main-worktree-session",
        "2026-04-10T00:00:00.000Z",
        listed_main_cwd.clone(),
    )
    .await;
    write_user(
        linked_project_dir,
        "linked-worktree-session",
        "2026-04-10T00:00:05.000Z",
        listed_linked_cwd.clone(),
    )
    .await;

    let summaries = manager.list_sessions().await.expect("list sessions");
    assert!(
        summaries
            .iter()
            .any(|summary| summary.session_id == "main-worktree-session")
    );
    assert!(
        summaries
            .iter()
            .any(|summary| summary.session_id == "linked-worktree-session")
    );

    let (session, _) = manager
        .continue_latest()
        .await
        .expect("continue latest same repo session");
    assert_eq!(session.session_id, "linked-worktree-session");
    assert_eq!(manager.effective_config().cwd, listed_linked_cwd);
}

#[tokio::test]
async fn explicit_resume_errors_for_missing_transcript() {
    let manager = test_manager().await;

    let error = manager
        .start_or_resume(Some("missing-session"))
        .await
        .expect_err("missing explicit session should error");

    assert!(
        matches!(error, CoreError::SessionNotFound(session_id) if session_id == "missing-session")
    );
}

#[tokio::test]
async fn explicit_resume_finds_cross_project_transcript_and_keeps_appends_there() {
    let manager = test_manager().await;
    let session_id = "cross-project-session";
    let other_project_dir = manager.config.projects_dir.join("other-project");
    let other_cwd = manager.config.home_dir.join("other-cwd");
    tokio::fs::create_dir_all(&other_project_dir)
        .await
        .expect("create other project dir");
    tokio::fs::create_dir_all(&other_cwd)
        .await
        .expect("create other cwd");
    tokio::fs::write(other_cwd.join("CLAUDE.md"), "Use resumed cwd instructions.")
        .await
        .expect("write resumed cwd instructions");
    let other_transcript = other_project_dir.join(format!("{session_id}.jsonl"));
    let current_transcript = manager
        .config
        .current_project_dir
        .join(format!("{session_id}.jsonl"));
    let payload = serde_json::to_string(&json!({
        "type": "user",
        "uuid": "user-1",
        "timestamp": "2026-04-10T00:00:00.000Z",
        "message": { "role": "user", "content": "from another project" },
        "cwd": other_cwd.display().to_string(),
        "sessionId": session_id,
    }))
    .expect("serialize transcript");
    tokio::fs::write(&other_transcript, format!("{payload}\n"))
        .await
        .expect("write cross-project transcript");

    let (session, _) = manager
        .start_or_resume(Some(session_id))
        .await
        .expect("resume cross-project session");
    assert_eq!(session.cwd, Some(other_cwd.display().to_string()));
    assert_eq!(manager.effective_config().cwd, other_cwd);
    let context = manager.context_preview().await;
    assert_eq!(context.cwd, other_cwd.display().to_string());
    assert!(
        context
            .claude_md
            .as_deref()
            .is_some_and(|contents| contents.contains("Use resumed cwd instructions."))
    );

    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "follow-up"),
        )
        .await
        .expect("append follow-up");

    assert!(
        !tokio::fs::try_exists(current_transcript)
            .await
            .expect("stat current transcript"),
        "explicit cross-project resume must not fork writes into the current project"
    );
    let contents = tokio::fs::read_to_string(other_transcript)
        .await
        .expect("read source transcript");
    assert!(contents.contains("from another project"));
    assert!(contents.contains("follow-up"));
    let cwd_values = contents
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parse transcript row"))
        .filter_map(|entry| entry.get("cwd").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    assert_eq!(
        cwd_values,
        vec![
            other_cwd.display().to_string(),
            other_cwd.display().to_string()
        ]
    );
}

#[tokio::test]
async fn resume_restores_session_added_additional_directories() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let extra = manager.config.home_dir.join("extra-workspace");
    tokio::fs::create_dir_all(&extra)
        .await
        .expect("create extra workspace");

    let changed = manager
        .add_runtime_additional_directory(&session_id, extra.clone())
        .await
        .expect("add runtime directory");
    assert!(changed);
    assert_eq!(
        manager.runtime_additional_directories(),
        vec![extra.clone()]
    );

    manager
        .clear_session(&session_id)
        .await
        .expect("clear session");
    assert!(manager.runtime_additional_directories().is_empty());

    let (resumed, _) = manager
        .start_or_resume(Some(&session_id))
        .await
        .expect("resume session");

    assert_eq!(
        resumed.additional_directories,
        vec![extra.display().to_string()]
    );
    assert_eq!(
        manager.runtime_additional_directories(),
        vec![extra.clone()]
    );
    assert!(manager.additional_directories().contains(&extra));
    assert_eq!(
        manager.context_preview().await.additional_directories,
        vec![extra.display().to_string()]
    );
}

#[tokio::test]
async fn clear_session_preserves_workflow_child_artifacts_for_resumable_parent() {
    let manager = test_manager().await;
    let parent_session_id = "clear-keeps-parent";
    let child_session_id = "clear-keeps-parent:workflow:agent-cleanup";
    write_parent_transcript(&manager, parent_session_id).await;
    let child_transcript_path =
        write_workflow_child_artifacts(&manager, parent_session_id, child_session_id).await;

    manager
        .clear_session(parent_session_id)
        .await
        .expect("clear session");

    assert!(
        manager
            .child_session_store
            .load(child_session_id)
            .await
            .expect("load child")
            .is_some(),
        "clear starts a new session but preserves the old parent and its child metadata"
    );
    assert!(
        tokio::fs::try_exists(&child_transcript_path)
            .await
            .expect("stat child transcript"),
        "clear must preserve child transcript while parent transcript remains resumable"
    );
    assert!(
        tokio::fs::try_exists(manager.transcript_store.path(parent_session_id))
            .await
            .expect("stat parent transcript")
    );
}

#[tokio::test]
async fn delete_acp_visible_session_removes_workflow_child_artifacts() {
    let manager = test_manager().await;
    let parent_session_id = "delete-removes-parent";
    let child_session_id = "delete-removes-parent:workflow:agent-cleanup";
    write_parent_transcript(&manager, parent_session_id).await;
    let child_transcript_path =
        write_workflow_child_artifacts(&manager, parent_session_id, child_session_id).await;
    let workflow_journal_path = manager
        .config
        .home_dir
        .join("workflow-runs")
        .join("cleanup")
        .join("journal.jsonl");
    tokio::fs::create_dir_all(workflow_journal_path.parent().unwrap())
        .await
        .expect("workflow journal dir");
    tokio::fs::write(&workflow_journal_path, "{}\n")
        .await
        .expect("workflow journal");

    manager
        .delete_acp_visible_session(parent_session_id, manager.config.cwd.clone())
        .await
        .expect("delete parent session");

    assert!(
        !tokio::fs::try_exists(manager.transcript_store.path(parent_session_id))
            .await
            .expect("stat parent transcript")
    );
    assert!(
        manager
            .child_session_store
            .load(child_session_id)
            .await
            .expect("load child")
            .is_none(),
        "deleting a parent transcript should remove child metadata"
    );
    assert!(
        !tokio::fs::try_exists(&child_transcript_path)
            .await
            .expect("stat child transcript"),
        "deleting a parent transcript should remove child transcript"
    );
    assert!(
        tokio::fs::try_exists(&workflow_journal_path)
            .await
            .expect("stat workflow journal"),
        "workflow run journals remain background-job history and old fallback data"
    );
}

#[tokio::test]
async fn gc_stale_sessions_removes_child_artifacts_for_removed_parent() {
    let manager = test_manager().await;
    let parent_session_id = "gc-removes-parent";
    let child_session_id = "gc-removes-parent:workflow:agent-cleanup";
    tokio::fs::write(
        manager.transcript_store.path(parent_session_id),
        "not-json\n",
    )
    .await
    .expect("write corrupt parent transcript");
    let child_transcript_path =
        write_workflow_child_artifacts(&manager, parent_session_id, child_session_id).await;

    let result = manager
        .gc_stale_sessions(0)
        .await
        .expect("gc stale sessions");

    assert!(result.removed_ids.contains(&parent_session_id.to_string()));
    assert_eq!(result.removed_child_metadata, 1);
    assert_eq!(result.removed_child_transcripts, 1);
    assert!(
        manager
            .child_session_store
            .load(child_session_id)
            .await
            .expect("load child")
            .is_none()
    );
    assert!(
        !tokio::fs::try_exists(&child_transcript_path)
            .await
            .expect("stat child transcript")
    );
}

#[tokio::test]
async fn session_storage_health_includes_child_session_artifacts() {
    let manager = test_manager().await;
    let parent_session_id = "health-parent";
    let child_session_id = "health-parent:workflow:agent-cleanup";
    write_parent_transcript(&manager, parent_session_id).await;
    write_workflow_child_artifacts(&manager, parent_session_id, child_session_id).await;

    let health = manager.session_storage_health().await;

    assert_eq!(health.child_sessions.metadata_records, 1);
    assert_eq!(health.child_sessions.transcript_records, 1);
    assert_eq!(health.child_sessions.orphan_metadata_records, 0);
    assert_eq!(health.child_sessions.orphan_transcripts, 0);
    assert_eq!(health.child_sessions.corrupt_transcripts, 0);
}

#[tokio::test]
async fn cleanup_orphan_child_sessions_dry_runs_then_removes_terminal_scoped_orphans() {
    let manager = test_manager().await;
    write_parent_transcript(&manager, "kept-parent").await;

    let eligible_child = "missing-parent:agent-orphan";
    let eligible_transcript =
        write_workflow_child_artifacts(&manager, "missing-parent", eligible_child).await;
    manager
        .child_session_store
        .complete(eligible_child)
        .await
        .expect("complete eligible orphan");

    let running_child = "missing-parent:agent-running";
    let running_transcript =
        write_workflow_child_artifacts(&manager, "missing-parent", running_child).await;

    let kept_child = "kept-parent:agent-kept";
    let kept_transcript = write_workflow_child_artifacts(&manager, "kept-parent", kept_child).await;
    manager
        .child_session_store
        .complete(kept_child)
        .await
        .expect("complete kept child");

    let out_of_scope_child = "missing-parent:agent-out-of-scope";
    manager
        .child_session_store
        .start(orbcode_session_store::StartChildSessionInput {
            child_session_id: out_of_scope_child.to_string(),
            parent_session_id: "missing-parent".to_string(),
            agent_id: "agent-cleanup".to_string(),
            agent_type: "general-purpose".to_string(),
            source_tool_use_id: "workflow:cleanup:step.out".to_string(),
            cwd: "/tmp/other-project".to_string(),
            model: Some("test-model".to_string()),
            permission_mode: None,
            prompt: "out of scope child".to_string(),
        })
        .await
        .expect("start out-of-scope child");
    manager
        .child_session_store
        .complete(out_of_scope_child)
        .await
        .expect("complete out-of-scope child");

    let dry_run = manager
        .cleanup_orphan_child_sessions(true, None)
        .await
        .expect("dry-run orphan cleanup");

    assert!(dry_run.dry_run);
    assert_eq!(dry_run.inspected_metadata, 3);
    assert_eq!(dry_run.orphan_metadata, 2);
    assert_eq!(dry_run.eligible_metadata, 1);
    assert_eq!(dry_run.skipped_running_metadata, 1);
    assert_eq!(dry_run.removed_metadata, 0);
    assert_eq!(dry_run.removed_transcripts, 0);
    assert_eq!(
        dry_run.orphan_child_session_ids,
        vec![eligible_child.to_string()]
    );
    assert!(
        tokio::fs::try_exists(&eligible_transcript)
            .await
            .expect("stat eligible transcript")
    );

    let applied = manager
        .cleanup_orphan_child_sessions(false, None)
        .await
        .expect("apply orphan cleanup");

    assert!(!applied.dry_run);
    assert_eq!(applied.inspected_metadata, 3);
    assert_eq!(applied.orphan_metadata, 2);
    assert_eq!(applied.eligible_metadata, 1);
    assert_eq!(applied.skipped_running_metadata, 1);
    assert_eq!(applied.removed_metadata, 1);
    assert_eq!(applied.removed_transcripts, 1);
    assert!(
        manager
            .child_session_store
            .load(eligible_child)
            .await
            .expect("load eligible child")
            .is_none()
    );
    assert!(
        !tokio::fs::try_exists(&eligible_transcript)
            .await
            .expect("stat eligible transcript after cleanup")
    );
    assert!(
        manager
            .child_session_store
            .load(running_child)
            .await
            .expect("load running child")
            .is_some()
    );
    assert!(
        tokio::fs::try_exists(&running_transcript)
            .await
            .expect("stat running transcript")
    );
    assert!(
        manager
            .child_session_store
            .load(kept_child)
            .await
            .expect("load kept child")
            .is_some()
    );
    assert!(
        tokio::fs::try_exists(&kept_transcript)
            .await
            .expect("stat kept transcript")
    );
    assert!(
        manager
            .child_session_store
            .load(out_of_scope_child)
            .await
            .expect("load out-of-scope child")
            .is_some()
    );
}

#[tokio::test]
async fn cleanup_orphan_child_sessions_can_include_stale_running_orphans() {
    let manager = test_manager().await;
    let stale_running_child = "stale-running-orphan";
    let fresh_running_child = "fresh-running-orphan";
    let stale_transcript =
        write_workflow_child_artifacts(&manager, "missing-parent", stale_running_child).await;
    let fresh_transcript =
        write_workflow_child_artifacts(&manager, "missing-parent", fresh_running_child).await;
    let cutoff = chrono::Utc::now().timestamp_millis() - 1_000;
    set_child_last_activity(&manager, stale_running_child, cutoff - 1_000).await;
    set_child_last_activity(&manager, fresh_running_child, cutoff + 1_000).await;

    let dry_run = manager
        .cleanup_orphan_child_sessions(true, Some(cutoff))
        .await
        .expect("dry-run stale running cleanup");

    assert_eq!(dry_run.orphan_metadata, 2);
    assert_eq!(dry_run.eligible_metadata, 1);
    assert_eq!(dry_run.stale_running_metadata, 1);
    assert_eq!(dry_run.skipped_running_metadata, 1);
    assert_eq!(
        dry_run.orphan_child_session_ids,
        vec![stale_running_child.to_string()]
    );

    let applied = manager
        .cleanup_orphan_child_sessions(false, Some(cutoff))
        .await
        .expect("apply stale running cleanup");

    assert_eq!(applied.removed_metadata, 1);
    assert_eq!(applied.removed_transcripts, 1);
    assert!(
        manager
            .child_session_store
            .load(stale_running_child)
            .await
            .expect("load stale running child")
            .is_none()
    );
    assert!(
        !tokio::fs::try_exists(&stale_transcript)
            .await
            .expect("stat stale transcript")
    );
    assert!(
        manager
            .child_session_store
            .load(fresh_running_child)
            .await
            .expect("load fresh running child")
            .is_some()
    );
    assert!(
        tokio::fs::try_exists(&fresh_transcript)
            .await
            .expect("stat fresh transcript")
    );
}

#[tokio::test]
async fn resume_restores_session_permission_rules() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .add_session_permission_rule_for_session(
            &session_id,
            PermissionRuleSettingKind::Allow,
            "Read(notes/**)",
        )
        .await
        .expect("add session allow");
    manager
        .add_session_permission_rule_for_session(
            &session_id,
            PermissionRuleSettingKind::Deny,
            "Bash(rm:*)",
        )
        .await
        .expect("add session deny");

    manager
        .clear_session(&session_id)
        .await
        .expect("clear session");
    assert!(
        manager
            .session_permission_rules(PermissionRuleSettingKind::Allow)
            .is_empty()
    );
    assert!(
        manager
            .session_permission_rules(PermissionRuleSettingKind::Deny)
            .is_empty()
    );

    let (resumed, _) = manager
        .start_or_resume(Some(&session_id))
        .await
        .expect("resume session");

    assert_eq!(resumed.session_allowed_tools, vec!["Read(notes/**)"]);
    assert_eq!(resumed.session_disallowed_tools, vec!["Bash(rm:*)"]);
    assert_eq!(
        manager.session_permission_rules(PermissionRuleSettingKind::Allow),
        vec!["Read(notes/**)"]
    );
    assert_eq!(
        manager.session_permission_rules(PermissionRuleSettingKind::Deny),
        vec!["Bash(rm:*)"]
    );
    assert!(
        manager
            .permission_context()
            .tool_allowed_without_prompt("Read", r#"{"file_path":"notes/today.md"}"#)
    );
    assert!(
        manager
            .permission_context()
            .tool_denied("bash", r#"{"command":"rm -rf /tmp/example"}"#)
            .is_some()
    );
}

#[tokio::test]
async fn resume_restores_session_effort_override() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .set_effort_override_for_session(&session_id, Some(EffortLevel::High))
        .await
        .expect("set effort");
    assert_eq!(manager.runtime_effort_override(), Some(EffortLevel::High));

    manager
        .clear_session(&session_id)
        .await
        .expect("clear session");
    assert_eq!(manager.runtime_effort_override(), None);

    let (resumed, _) = manager
        .start_or_resume(Some(&session_id))
        .await
        .expect("resume session");
    assert_eq!(resumed.session_effort, Some(EffortLevel::High));
    assert_eq!(manager.runtime_effort_override(), Some(EffortLevel::High));

    manager
        .set_effort_override_for_session(&session_id, None)
        .await
        .expect("clear effort");
    assert_eq!(manager.runtime_effort_override(), None);

    manager
        .clear_session(&session_id)
        .await
        .expect("clear session again");
    let (resumed, _) = manager
        .start_or_resume(Some(&session_id))
        .await
        .expect("resume cleared effort");
    assert_eq!(resumed.session_effort, None);
    assert_eq!(manager.runtime_effort_override(), None);
}

#[tokio::test]
async fn clear_session_resets_resumed_active_cwd() {
    let manager = test_manager().await;
    let session_id = "clear-resume-cwd-session";
    let resumed_cwd = manager.config.home_dir.join("resumed-cwd");
    tokio::fs::create_dir_all(&resumed_cwd)
        .await
        .expect("create resumed cwd");
    let payload = serde_json::to_string(&json!({
        "type": "user",
        "uuid": "user-1",
        "timestamp": "2026-04-10T00:00:00.000Z",
        "message": { "role": "user", "content": "resume then clear" },
        "cwd": resumed_cwd.display().to_string(),
        "sessionId": session_id,
    }))
    .expect("serialize transcript");
    tokio::fs::write(
        manager.transcript_store.path(session_id),
        format!("{payload}\n"),
    )
    .await
    .expect("write transcript");

    manager
        .start_or_resume(Some(session_id))
        .await
        .expect("resume session");
    assert_eq!(manager.effective_config().cwd, resumed_cwd);

    manager
        .clear_session(session_id)
        .await
        .expect("clear session");

    assert_eq!(manager.effective_config().cwd, manager.config.cwd);
}

#[tokio::test]
async fn resume_repairs_missing_tool_results_and_drops_orphan_tool_results() {
    let manager = test_manager().await;
    let session_id = "repair-session";
    let transcript_path = manager.transcript_store.path(session_id);
    let payload = [
        json!({
            "type": "assistant",
            "uuid": "assistant-tool-use",
            "timestamp": "2026-04-10T00:00:00.000Z",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "tool-1",
                    "name": "Read",
                    "input": { "file_path": "README.md" }
                }]
            },
            "sessionId": session_id,
        }),
        json!({
            "type": "user",
            "uuid": "orphan-result",
            "timestamp": "2026-04-10T00:00:01.000Z",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "orphan-tool",
                    "content": "orphaned",
                    "is_error": false
                }]
            },
            "sessionId": session_id,
        }),
        json!({
            "type": "assistant",
            "uuid": "assistant-final",
            "timestamp": "2026-04-10T00:00:02.000Z",
            "message": {
                "role": "assistant",
                "content": "done"
            },
            "sessionId": session_id,
        }),
    ]
    .into_iter()
    .map(|value| serde_json::to_string(&value).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    tokio::fs::write(&transcript_path, format!("{payload}\n"))
        .await
        .expect("write transcript");

    let (session, _) = manager
        .start_or_resume(Some(session_id))
        .await
        .expect("resume repaired session");

    assert_eq!(session.messages.len(), 3);
    assert!(matches!(
        session.messages[1].blocks.as_slice(),
        [TranscriptBlock::ToolResult { tool_use_id, content, is_error, .. }]
            if tool_use_id == "tool-1" && content == MISSING_TOOL_RESULT && *is_error
    ));
    assert!(
        !session.messages.iter().any(|message| {
            message.blocks.iter().any(|block| {
                matches!(block, TranscriptBlock::ToolResult { tool_use_id, .. } if tool_use_id == "orphan-tool")
            })
        }),
        "orphan tool results should not remain model-visible after resume repair"
    );

    let reloaded = manager
        .load_session(session_id)
        .await
        .expect("reload repaired session");
    assert_eq!(reloaded.messages.len(), 3);
    assert!(matches!(
        reloaded.messages[1].blocks.as_slice(),
        [TranscriptBlock::ToolResult { tool_use_id, content, is_error, .. }]
            if tool_use_id == "tool-1" && content == MISSING_TOOL_RESULT && *is_error
    ));
    assert!(!reloaded.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, TranscriptBlock::ToolResult { tool_use_id, .. } if tool_use_id == "orphan-tool")
        })
    }));
}

#[tokio::test]
async fn loads_structured_blocks_from_claude_transcript() {
    let manager = test_manager().await;
    let session_id = "structured-session";
    let transcript_path = manager.transcript_store.path(session_id);
    let payload = [
        json!({
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-04-10T00:00:00.000Z",
            "message": {
                "role": "user",
                "content": [
                    { "type": "text", "text": "read file" },
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "file contents",
                        "is_error": false
                    }
                ]
            }
        }),
        json!({
            "type": "assistant",
            "uuid": "assistant-1",
            "timestamp": "2026-04-10T00:00:01.000Z",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "need to inspect the file" },
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "Read",
                        "input": { "file_path": "/tmp/example.rs" }
                    },
                    { "type": "text", "text": "Here is what I found." }
                ]
            }
        }),
    ]
    .into_iter()
    .map(|value| serde_json::to_string(&value).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    tokio::fs::write(&transcript_path, format!("{payload}\n"))
        .await
        .expect("write transcript");

    let session = manager
        .load_session(session_id)
        .await
        .expect("load structured transcript");

    assert_eq!(session.messages.len(), 2);
    assert!(matches!(
        &session.messages[0].blocks[0],
        TranscriptBlock::Text { text } if text == "read file"
    ));
    assert!(matches!(
        &session.messages[0].blocks[1],
        TranscriptBlock::ToolResult { tool_use_id, content, is_error, .. }
            if tool_use_id == "tool-1" && content == "file contents" && !is_error
    ));
    assert!(matches!(
        &session.messages[1].blocks[0],
        TranscriptBlock::Thinking { text, .. } if text == "need to inspect the file"
    ));
    assert!(matches!(
        &session.messages[1].blocks[1],
        TranscriptBlock::ToolUse { id, name, input }
            if id == "tool-1" && name == "Read" && input.contains("/tmp/example.rs")
    ));
    assert!(matches!(
        &session.messages[1].blocks[2],
        TranscriptBlock::Text { text } if text == "Here is what I found."
    ));
    assert!(
        !session.messages[1]
            .content
            .contains("need to inspect the file")
    );
    assert!(!session.messages[1].content.contains("thinking"));
}

#[tokio::test]
async fn persists_and_loads_assistant_usage() {
    let manager = test_manager().await;
    let session_id = "usage-session";
    let usage = TokenUsage {
        input_tokens: 10,
        cache_creation_input_tokens: 5,
        cache_read_input_tokens: 3,
        output_tokens: 2,
        total_tokens: 20,
        ..TokenUsage::default()
    };
    let message =
        TranscriptMessage::new(MessageRole::Assistant, "tracked usage").with_usage(usage.clone());

    manager
        .append_message(session_id, message)
        .await
        .expect("append assistant usage");

    let transcript = tokio::fs::read_to_string(manager.transcript_store.path(session_id))
        .await
        .expect("read transcript");
    assert!(transcript.contains(r#""cache_read_input_tokens":3"#));

    let session = manager
        .load_session(session_id)
        .await
        .expect("load session");
    assert_eq!(session.messages[0].usage, Some(usage));
}

#[tokio::test]
async fn fork_preserves_structured_blocks() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::Thinking {
                        text: "plan first".to_string(),
                        signature: None,
                    },
                    TranscriptBlock::Text {
                        text: "done".to_string(),
                    },
                ],
            ),
        )
        .await
        .expect("append assistant");

    let fork = manager
        .fork_session(&session_id, Some("fork".to_string()), None)
        .await
        .expect("fork session");

    assert!(fork.messages.iter().any(|message| {
        matches!(
            message.blocks.as_slice(),
            [
                TranscriptBlock::Thinking { text, .. },
                TranscriptBlock::Text { .. }
            ] if text == "plan first"
        )
    }));
}

#[tokio::test]
async fn gc_drops_pre_compact_messages_on_load() {
    let manager = test_manager().await;
    let session_id = "gc-compact-session";
    let transcript_path = manager.transcript_store.path(session_id);

    let payload = [
        json!({
            "type": "user",
            "uuid": "old-user",
            "timestamp": "2026-01-01T00:00:00.000Z",
            "message": { "role": "user", "content": "old question that should be GC'd" },
            "cwd": "/tmp",
            "sessionId": session_id,
        }),
        json!({
            "type": "assistant",
            "uuid": "old-assistant",
            "timestamp": "2026-01-01T00:00:01.000Z",
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "old answer that should be GC'd" }],
                "model": "claude-opus-4-7",
            },
        }),
        json!({
            "type": "system",
            "uuid": "compact-summary",
            "timestamp": "2026-01-01T00:01:00.000Z",
            "message": {
                "role": "system",
                "content": "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\nSummary:\nUser asked an old question.\n\nTranscript: /tmp/test.jsonl"
            },
        }),
        json!({
            "type": "user",
            "uuid": "new-user",
            "timestamp": "2026-01-01T00:02:00.000Z",
            "message": { "role": "user", "content": "new question after compaction" },
            "cwd": "/tmp",
            "sessionId": session_id,
        }),
        json!({
            "type": "assistant",
            "uuid": "new-assistant",
            "timestamp": "2026-01-01T00:02:01.000Z",
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "fresh answer" }],
                "model": "claude-opus-4-7",
            },
        }),
    ]
    .into_iter()
    .map(|value| serde_json::to_string(&value).expect("serialize"))
    .collect::<Vec<_>>()
    .join("\n");
    tokio::fs::write(&transcript_path, format!("{payload}\n"))
        .await
        .expect("write transcript");

    let session = manager
        .load_session(session_id)
        .await
        .expect("load session with compact boundary");

    assert_eq!(
        session.messages.len(),
        3,
        "compact summary + 2 post-compact messages"
    );
    assert_eq!(session.messages[0].role, MessageRole::System);
    assert!(
        session.messages[0]
            .content
            .starts_with("This session is being continued"),
        "first message is the compact summary"
    );
    assert_eq!(session.messages[1].id, "new-user");
    assert_eq!(session.messages[2].id, "new-assistant");
    assert!(
        !session
            .messages
            .iter()
            .any(|m| m.id == "old-user" || m.id == "old-assistant"),
        "pre-compact messages were GC'd"
    );
}

#[tokio::test]
async fn concurrent_appends_keep_a_linear_parent_chain() {
    // Overlapping turn drivers can call `append_message` concurrently (e.g. an
    // interrupted turn still draining while the next turn begins). The per-session
    // append lock must serialize read-parent + write so the persisted transcript
    // stays a single unbroken chain rather than forking on a shared parent_uuid.
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut handles = Vec::new();
    for index in 0..12 {
        let manager = manager.clone();
        let session_id = session_id.clone();
        handles.push(tokio::spawn(async move {
            let message =
                TranscriptMessage::new(MessageRole::Assistant, format!("message {index}"));
            manager
                .append_message(&session_id, message)
                .await
                .expect("append message");
        }));
    }
    for handle in handles {
        handle.await.expect("append task");
    }

    // Parse the raw transcript: parentUuid links live in the JSONL, not in the
    // in-memory TranscriptMessage.
    let path = manager.transcript_store.path(&session_id);
    let contents = tokio::fs::read_to_string(&path)
        .await
        .expect("read transcript");
    let mut uuids = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("message").is_none() {
            continue;
        }
        let Some(uuid) = value.get("uuid").and_then(|v| v.as_str()) else {
            continue;
        };
        let parent = value
            .get("parentUuid")
            .and_then(|v| v.as_str())
            .map(String::from);
        uuids.insert(uuid.to_string());
        entries.push((uuid.to_string(), parent));
    }

    assert_eq!(
        entries.len(),
        12,
        "every concurrent append must be recorded"
    );
    assert_eq!(uuids.len(), 12, "message uuids must be unique");
    // No two messages may chain onto the same parent (a fork), and every
    // non-root parent must reference a real message.
    let mut seen_parents = std::collections::HashSet::new();
    for (_, parent) in entries.iter().skip(1) {
        let parent = parent
            .clone()
            .expect("non-root message must have a parentUuid");
        assert!(
            uuids.contains(&parent),
            "parentUuid must reference a real message"
        );
        assert!(
            seen_parents.insert(parent),
            "two messages share a parentUuid — the chain forked"
        );
    }
}
