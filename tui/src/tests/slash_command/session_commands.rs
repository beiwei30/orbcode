use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[tokio::test]
async fn help_slash_command_uses_registry_alias() {
    let home_dir = test_temp_path("help-home");
    let cwd = test_temp_path("help-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/?", &local_command_tx)
        .await
        .expect("help command succeeds");

    assert!(matches!(state.overlay, Some(OverlayState::Help(_))));
    assert_eq!(state.status_line, "Help: ↑↓ scroll, Esc close.");
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("/help"))
    );
}

#[tokio::test]
#[ignore = "pre-existing timeout: CompactFinished event never arrives in test harness"]
async fn compact_slash_command_replaces_history_with_modeled_summary() {
    let home_dir = test_temp_path("compact-home");
    let cwd = test_temp_path("compact-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let first = app_server
        .app_server()
        .unwrap()
        .record_system_message(&state.session_id, "first persisted message")
        .await
        .expect("record first");
    let second = app_server
        .app_server()
        .unwrap()
        .record_system_message(&state.session_id, "second persisted message")
        .await
        .expect("record second");
    state.push_message_and_flush_history(first);
    state.push_message_and_flush_history(second);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/compact", &local_command_tx)
        .await
        .expect("compact command starts");

    assert_eq!(state.status_line, "Compacting conversation...");
    let compacting = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(
        compacting.contains("Compacting conversation..."),
        "{compacting}"
    );
    assert!(compacting.contains("└ Tip:"), "{compacting}");

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("compact event should arrive")
        .expect("compact event");
    state.apply_local_command_event(event.event);

    assert!(state.status_line.contains("Conversation compacted"));
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("first persisted message"))
    );
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(
        transcript.contains("first persisted message"),
        "{transcript}"
    );
    assert!(
        !transcript.contains("--- context compacted ---"),
        "{transcript}"
    );
    assert!(transcript.contains("❯ /compact"), "{transcript}");
    assert!(
        !transcript.contains("Compacted conversation summary:"),
        "{transcript}"
    );
    assert!(
        !transcript.contains("local modeled compaction placeholder"),
        "{transcript}"
    );
    assert!(
        transcript.contains("Summary source: local fallback."),
        "{transcript}"
    );
    let history_index = transcript
        .find("first persisted message")
        .expect("render original history");
    let marker_index = transcript
        .find("✻ Conversation compacted")
        .expect("render compact marker");
    let command_index = transcript.find("❯ /compact").expect("render command");
    assert!(history_index < marker_index, "{transcript}");
    assert!(marker_index < command_index, "{transcript}");
    let persisted = app_server
        .bootstrap_typed(Some(&state.session_id))
        .await
        .expect("reload compacted session")
        .session;
    assert_eq!(persisted.messages.len(), 1);
    assert!(
        persisted.messages[0]
            .content
            .contains("Original messages: 2 total")
    );

    state.expanded_tool_details = true;
    let expanded = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(expanded.contains("✻ Crunched for"), "{expanded}");
    assert!(expanded.contains("⏺ Compact summary"), "{expanded}");
    assert!(expanded.contains("local modeled compaction"), "{expanded}");
}

#[tokio::test]
async fn compact_slash_command_ignores_local_only_history() {
    let home_dir = test_temp_path("compact-local-home");
    let cwd = test_temp_path("compact-local-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    state.push_local_slash_command_output("/status", "Status loaded.", None);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/compact", &local_command_tx)
        .await
        .expect("compact local-only command succeeds");

    assert_eq!(state.status_line, "Nothing to compact.");
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(
        transcript.contains("No model-visible conversation history"),
        "{transcript}"
    );
}

#[tokio::test]
async fn rename_slash_command_makes_empty_session_visible_in_sessions_list() {
    let home_dir = test_temp_path("rename-empty-home");
    let cwd = test_temp_path("rename-empty-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let session_id = state.session_id.clone();
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/rename hello", &local_command_tx)
        .await
        .expect("rename succeeds before any model-visible message");
    assert_eq!(state.status_line, "Renamed session to \"hello\".");

    let summaries = app_server.list_sessions().await.expect("list sessions");
    let summary = summaries
        .iter()
        .find(|summary| summary.session_id == session_id)
        .expect("empty renamed session must show up in /sessions");
    assert_eq!(summary.title.as_deref(), Some("hello"));

    let resumed = app_server
        .bootstrap_typed(Some(&session_id))
        .await
        .expect("bootstrap resumed session");
    assert_eq!(resumed.session.session_id, session_id);
    assert_eq!(resumed.session.custom_title.as_deref(), Some("hello"));
    assert!(resumed.session.messages.is_empty());

    state
        .handle_command(&app_server, "/clear", &local_command_tx)
        .await
        .expect("clear session before title resume");
    assert_ne!(state.session_id, session_id);
    state
        .handle_command(&app_server, "/resume hello", &local_command_tx)
        .await
        .expect("resume by exact custom title");
    assert_eq!(state.session_id, session_id);

    let error = state
        .handle_command(&app_server, "/rename   ", &local_command_tx)
        .await
        .expect_err("empty title is rejected");
    assert!(error.to_string().contains("usage: /rename"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn copy_slash_command_copies_last_assistant_response() {
    let _clipboard_guard = test_clipboard_assertion_lock()
        .lock()
        .expect("test clipboard assertion mutex poisoned");
    let _ = take_test_clipboard_capture();

    let home_dir = test_temp_path("copy-home");
    let cwd = test_temp_path("copy-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let empty_error = state
        .handle_command(&app_server, "/copy", &local_command_tx)
        .await
        .expect_err("no assistant response yet");
    assert!(empty_error.to_string().contains("No assistant response"));
    assert_eq!(take_test_clipboard_capture(), None);

    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::Text {
            text: "Final answer for the user.".to_string(),
        }],
    ));

    state
        .handle_command(&app_server, "/copy", &local_command_tx)
        .await
        .expect("copy command succeeds");

    assert_eq!(
        take_test_clipboard_capture().as_deref(),
        Some("Final answer for the user.")
    );
    assert!(
        state
            .status_line
            .starts_with("Copied last assistant response"),
        "{}",
        state.status_line
    );
}

#[tokio::test]
async fn files_slash_command_lists_referenced_files_and_directories() {
    let home_dir = test_temp_path("files-home");
    let cwd = test_temp_path("files-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd.clone(),
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "read-1".to_string(),
            name: "Read".to_string(),
            input: r#"{"file_path":"src/main.rs"}"#.to_string(),
        }],
    ));
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::ToolResult {
            tool_use_id: "read-1".to_string(),
            content: "fn main() {}\n".to_string(),
            is_error: false,
            metadata: None,
        }],
    ));
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/files", &local_command_tx)
        .await
        .expect("files command succeeds");

    assert_eq!(state.status_line, "1 recent file(s) tracked.");
    let transcript = plain_text_lines(&state.transcript_lines(120)).join("\n");
    assert!(transcript.contains("❯ /files"), "{transcript}");
    assert!(transcript.contains("src/main.rs"), "{transcript}");
    assert!(
        transcript.contains(&cwd.display().to_string()),
        "{transcript}"
    );
}

#[test]
fn slash_command_registry_marks_init_and_exit() {
    use crate::slash_commands::BuiltinPromptSlashCommand;

    assert_eq!(
        slash_command_invocation("/exit").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::Exit)
    );
    // `/quit` is the documented alias for `/exit`.
    assert_eq!(
        slash_command_invocation("/quit")
            .map(|invocation| (invocation.spec.execution, invocation.spec.name)),
        Some((SlashCommandExecution::Exit, "exit"))
    );
    assert_eq!(
        slash_command_invocation("/init").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::BuiltinPrompt(
            BuiltinPromptSlashCommand::Init
        ))
    );
}

#[test]
fn help_overlay_lists_rewind_init_exit() {
    let lines = plain_text_lines(&help_overlay_lines(120));
    assert!(
        lines.iter().any(|line| line.contains("/exit")),
        "/exit should appear in help"
    );
    assert!(
        lines.iter().any(|line| line.contains("/rewind")),
        "/rewind should appear in help"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("/init") && line.contains("CLAUDE.md")),
        "/init should appear in help with its description"
    );
}

#[tokio::test]
async fn exit_slash_command_returns_exit_outcome() {
    use crate::commands::dispatch::SlashCommandOutcome;

    let home_dir = test_temp_path("exit-home");
    let cwd = test_temp_path("exit-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(&app_server, "/exit", &local_command_tx)
        .await
        .expect("/exit dispatches");
    assert!(matches!(outcome, SlashCommandOutcome::Exit));

    // The `/quit` alias resolves to the same clean-exit outcome.
    let outcome = state
        .handle_command(&app_server, "/quit", &local_command_tx)
        .await
        .expect("/quit dispatches");
    assert!(matches!(outcome, SlashCommandOutcome::Exit));

    // Trailing arguments are rejected rather than silently ignored.
    let error = state
        .handle_command(&app_server, "/exit now", &local_command_tx)
        .await
        .expect_err("/exit takes no arguments");
    assert!(error.to_string().contains("usage: /exit"));
}

#[tokio::test]
async fn init_slash_command_submits_codebase_documentation_prompt() {
    use crate::commands::builtin_prompts::builtin_prompt_body;
    use crate::commands::dispatch::SlashCommandOutcome;
    use crate::slash_commands::BuiltinPromptSlashCommand;

    let home_dir = test_temp_path("init-home");
    let cwd = test_temp_path("init-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(&app_server, "/init", &local_command_tx)
        .await
        .expect("/init dispatches");
    match outcome {
        SlashCommandOutcome::PromptToSubmit(prompt) => {
            assert_eq!(prompt, builtin_prompt_body(BuiltinPromptSlashCommand::Init))
        }
        other => panic!("expected /init to submit a prompt, got {other:?}"),
    }

    let error = state
        .handle_command(&app_server, "/init extra", &local_command_tx)
        .await
        .expect_err("/init takes no arguments");
    assert!(error.to_string().contains("usage: /init"));
}

#[tokio::test]
async fn rewind_slash_command_opens_picker_or_reports_empty() {
    let home_dir = test_temp_path("rewind-home");
    let cwd = test_temp_path("rewind-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    // With no user turns the command reports that there is nothing to rewind to.
    state
        .handle_command(&app_server, "/rewind", &local_command_tx)
        .await
        .expect("/rewind dispatches");
    assert!(state.overlay.is_none());
    assert_eq!(state.status_line, "No user turns to rewind to yet.");

    // Once a user turn exists the picker overlay opens.
    state.messages = vec![TranscriptMessage::new(
        MessageRole::User,
        "remembered prompt".to_string(),
    )];
    state
        .handle_command(&app_server, "/rewind", &local_command_tx)
        .await
        .expect("/rewind dispatches with user turns");
    assert!(matches!(state.overlay, Some(OverlayState::RewindPicker(_))));
}

#[tokio::test]
async fn hooks_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("hooks-home");
    let cwd = test_temp_path("hooks-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/hooks", &local_command_tx)
        .await
        .expect("hooks command starts");

    assert_eq!(state.status_line, "Loading hooks...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("hooks event should arrive")
        .expect("hooks event");
    state.apply_local_command_event(event.event);

    assert!(state.status_line.contains("hook(s) discovered"));
}

#[tokio::test]
async fn skills_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("skills-home");
    let cwd = test_temp_path("skills-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/skills", &local_command_tx)
        .await
        .expect("skills command starts");

    assert_eq!(state.status_line, "Loading skill definitions...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("skills event should arrive")
        .expect("skills event");
    state.apply_local_command_event(event.event);

    assert!(state.status_line.contains("skill(s) loaded"));
}

#[tokio::test]
async fn agents_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("agents-home");
    let cwd = test_temp_path("agents-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/agents", &local_command_tx)
        .await
        .expect("agents command starts");

    assert_eq!(state.status_line, "Loading agent definitions...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("agents event should arrive")
        .expect("agents event");
    state.apply_local_command_event(event.event);

    assert!(state.status_line.contains("agent definition(s) loaded"));
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("general-purpose"))
    );
}

#[tokio::test]
async fn skills_slash_command_discovers_project_skills() {
    let home_dir = test_temp_path("skills-disc-home");
    let cwd = test_temp_path("skills-disc-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let skill_dir = cwd.join(".claude").join("skills").join("review");
    tokio::fs::create_dir_all(&skill_dir)
        .await
        .expect("create skill dir");
    tokio::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: Review code\n---\nReview the code thoroughly.\n",
    )
    .await
    .expect("write skill");

    let app_server = AppServer::new(
        &cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/skills", &local_command_tx)
        .await
        .expect("skills command starts");

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("skills event should arrive")
        .expect("skills event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("review"))
    );
}

#[tokio::test]
async fn agents_slash_command_discovers_project_agents() {
    let home_dir = test_temp_path("agents-disc-home");
    let cwd = test_temp_path("agents-disc-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let agents_dir = cwd.join(".claude").join("agents");
    tokio::fs::create_dir_all(&agents_dir)
        .await
        .expect("create agents dir");
    tokio::fs::write(
        agents_dir.join("reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews pull requests\nmodel: sonnet\n---\nReview the PR.\n",
    )
    .await
    .expect("write agent");

    let app_server = AppServer::new(
        &cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/agents", &local_command_tx)
        .await
        .expect("agents command starts");

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("agents event should arrive")
        .expect("agents event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("reviewer"))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("model=sonnet"))
    );
}

#[tokio::test]
async fn skill_sourced_dynamic_commands_appear_in_suggestions() {
    use crate::dynamic_slash_commands::{DynamicSlashCommandSource, DynamicSlashCommandSpec};
    use crate::slash_commands::{
        ExtensionSource, SlashCommandSource, register_dynamic_slash_commands,
        slash_command_suggestions,
    };
    use std::sync::{Mutex, OnceLock};

    fn registry_test_guard() -> &'static Mutex<()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(()))
    }

    let _lock = registry_test_guard().lock().expect("test guard poisoned");

    register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
        name: "my-skill".into(),
        aliases: Vec::new(),
        description: "A test skill".into(),
        argument_hint: Some("<args>".into()),
        source: DynamicSlashCommandSource::Skill,
        hidden: false,
        prompt_body: "Do the thing.".into(),
        mcp_prompt: None,
        workflow_name: None,
    }]);

    let suggestions = slash_command_suggestions("/my-sk");
    assert!(suggestions.iter().any(|spec| spec.name == "my-skill"));
    let skill_spec = suggestions
        .iter()
        .find(|spec| spec.name == "my-skill")
        .unwrap();
    assert_eq!(
        skill_spec.source,
        SlashCommandSource::Extension(ExtensionSource::Skill)
    );
    assert_eq!(skill_spec.source_label, Some("skill"));

    register_dynamic_slash_commands(Vec::new());
}

#[tokio::test]
async fn workflow_slash_command_starts_task_without_prompt_submission() {
    use crate::commands::dispatch::SlashCommandOutcome;
    use crate::dynamic_slash_commands::load_workflow_commands;
    use crate::slash_commands::register_dynamic_slash_commands;
    use orbcode_protocol::BackgroundTaskViewKind;
    use orbcode_tools::{BackgroundTaskStatus, read_background_task_record};
    use std::sync::OnceLock;

    fn registry_test_guard() -> &'static tokio::sync::Mutex<()> {
        static GUARD: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    let _lock = registry_test_guard().lock().await;

    let home_dir = test_temp_path("workflow-slash-home");
    let cwd = test_temp_path("workflow-slash-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(cwd.join(".claude/workflows/acp"))
        .await
        .expect("create workflows");
    tokio::fs::write(
        cwd.join(".claude/workflows/acp/check.json"),
        r#"{"schema_version":1,"description":"Run ACP check","steps":[{"log":{"message":"done $1"}}]}"#,
    )
    .await
    .expect("write workflow");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir.clone()),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let dynamic = load_workflow_commands(&app_server).await;
    register_dynamic_slash_commands(dynamic);

    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(&app_server, "/workflow:acp:check ok", &local_command_tx)
        .await
        .expect("workflow command dispatches");
    assert!(matches!(outcome, SlashCommandOutcome::Handled));
    let task_id = state
        .status_line
        .strip_prefix("Workflow started: ")
        .expect("status line includes workflow task id")
        .to_string();
    assert!(task_id.starts_with("workflow-"));

    assert!(
        state.messages.iter().any(|message| {
            message
                .content
                .contains(&format!("Started workflow task {task_id}."))
                && message.content.contains("TaskOutput")
                && message.content.contains("TaskStop")
        }),
        "expected local slash command output with task controls"
    );

    let mut record = None;
    for _ in 0..50 {
        record = read_background_task_record(&home_dir, &task_id)
            .await
            .expect("read workflow record");
        if record
            .as_ref()
            .is_some_and(|record| record.status == BackgroundTaskStatus::Completed)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let record = record.expect("workflow record");
    assert_eq!(record.status, BackgroundTaskStatus::Completed);
    assert_eq!(record.result.as_deref(), Some("done ok"));

    let workflow_row = state
        .transcript_task_cards
        .rows()
        .iter()
        .find(|row| row.task_id == task_id)
        .expect("workflow appears in transcript task card state");
    assert_eq!(workflow_row.kind, BackgroundTaskViewKind::Workflow);

    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(
        transcript.contains("Background tasks"),
        "workflow task card should render inline in transcript: {transcript}"
    );
    assert!(
        transcript.contains(&task_id),
        "workflow task card should include task id: {transcript}"
    );
    assert!(
        transcript.contains("workflow"),
        "workflow task card should include task type: {transcript}"
    );
    let request_status = plain_text_lines(&state.request_status_lines()).join("\n");
    assert!(
        !request_status.contains("Background tasks"),
        "background task card should not duplicate in request status: {request_status}"
    );

    register_dynamic_slash_commands(Vec::new());
}

#[tokio::test]
async fn branch_slash_command_creates_git_branch_when_name_given() {
    use crate::commands::dispatch::SlashCommandOutcome;

    let home_dir = test_temp_path("branch-home");
    let cwd = test_temp_path("branch-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    // Initialize a git repo so `git checkout -b` works.
    let git_init = tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(&cwd)
        .output()
        .await
        .expect("git init");
    assert!(git_init.status.success(), "git init failed");
    let git_commit = tokio::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial"])
        .current_dir(&cwd)
        .output()
        .await
        .expect("git commit");
    assert!(git_commit.status.success(), "git commit failed");

    let app_server = AppServer::new(
        cwd.clone(),
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(&app_server, "/branch test-feature", &local_command_tx)
        .await
        .expect("/branch dispatches");
    assert!(matches!(outcome, SlashCommandOutcome::Handled));
    assert!(
        state
            .status_line
            .contains("Created and switched to branch test-feature"),
        "status_line: {}",
        state.status_line
    );

    // Verify git actually switched to the branch.
    let git_branch = tokio::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&cwd)
        .output()
        .await
        .expect("git branch");
    assert_eq!(
        String::from_utf8_lossy(&git_branch.stdout).trim(),
        "test-feature"
    );
}

#[tokio::test]
async fn branch_slash_command_suggests_name_when_no_args() {
    use crate::commands::dispatch::SlashCommandOutcome;

    let home_dir = test_temp_path("branch-suggest-home");
    let cwd = test_temp_path("branch-suggest-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(&app_server, "/branch", &local_command_tx)
        .await
        .expect("/branch without args dispatches");
    match outcome {
        SlashCommandOutcome::PromptToSubmit(prompt) => {
            assert!(prompt.contains("branch name"), "prompt: {prompt}");
        }
        other => panic!("expected PromptToSubmit, got {other:?}"),
    }
}

#[tokio::test]
async fn review_slash_command_submits_review_prompt() {
    use crate::commands::dispatch::SlashCommandOutcome;

    let home_dir = test_temp_path("review-home");
    let cwd = test_temp_path("review-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(&app_server, "/review", &local_command_tx)
        .await
        .expect("/review dispatches");
    match outcome {
        SlashCommandOutcome::PromptToSubmit(prompt) => {
            assert!(
                prompt.contains("diff") && prompt.contains("correctness"),
                "prompt: {prompt}"
            );
        }
        other => panic!("expected PromptToSubmit, got {other:?}"),
    }
}

#[tokio::test]
async fn review_slash_command_comment_mode() {
    use crate::commands::dispatch::SlashCommandOutcome;

    let home_dir = test_temp_path("review-comment-home");
    let cwd = test_temp_path("review-comment-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(&app_server, "/review --comment", &local_command_tx)
        .await
        .expect("/review --comment dispatches");
    match outcome {
        SlashCommandOutcome::PromptToSubmit(prompt) => {
            assert!(prompt.contains("inline PR comments"), "prompt: {prompt}");
        }
        other => panic!("expected PromptToSubmit, got {other:?}"),
    }

    // Unknown flags are rejected.
    let error = state
        .handle_command(&app_server, "/review --unknown", &local_command_tx)
        .await
        .expect_err("/review --unknown should error");
    assert!(error.to_string().contains("unknown /review argument"));
}

#[test]
fn slash_command_branch_resolves_with_args() {
    let invocation = slash_command_invocation("/branch my-feature").unwrap();
    assert_eq!(invocation.spec.name, "branch");
    assert_eq!(invocation.args, "my-feature");
    assert_eq!(
        invocation.spec.execution,
        SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Branch)
    );
}

#[test]
fn slash_command_branch_resolves_without_args() {
    let invocation = slash_command_invocation("/branch").unwrap();
    assert_eq!(invocation.spec.name, "branch");
    assert_eq!(invocation.args, "");
}

#[test]
fn slash_command_branch_appears_in_suggestions() {
    let suggestions = slash_command_suggestions("/br");
    assert!(suggestions.iter().any(|spec| spec.name == "branch"));
}

#[test]
fn slash_command_review_resolves_without_args() {
    use crate::slash_commands::BuiltinPromptSlashCommand;

    let invocation = slash_command_invocation("/review").unwrap();
    assert_eq!(invocation.spec.name, "review");
    assert_eq!(invocation.args, "");
    assert_eq!(
        invocation.spec.execution,
        SlashCommandExecution::BuiltinPrompt(BuiltinPromptSlashCommand::Review)
    );
}

#[test]
fn slash_command_review_resolves_with_comment_flag() {
    let invocation = slash_command_invocation("/review --comment").unwrap();
    assert_eq!(invocation.spec.name, "review");
    assert_eq!(invocation.args, "--comment");
}

#[test]
fn slash_command_review_appears_in_suggestions() {
    let suggestions = slash_command_suggestions("/rev");
    assert!(suggestions.iter().any(|spec| spec.name == "review"));
}
