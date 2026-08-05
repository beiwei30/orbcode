use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[tokio::test]
async fn plan_slash_command_enters_and_displays_plan_mode() {
    let home_dir = test_temp_path("plan-home");
    let cwd = test_temp_path("plan-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/plan update diff UI", &local_command_tx)
        .await
        .expect("plan command starts");

    assert_eq!(state.status_line, "Loading plan...");
    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("plan enter event should arrive")
        .expect("plan enter event");
    let submit_prompt = state.apply_local_command_event(event.event);

    assert_eq!(state.status_line, "Plan mode enabled.");
    assert_eq!(submit_prompt.as_deref(), Some("update diff UI"));
    let plan_path = app_server
        .app_server()
        .unwrap()
        .plan_overview()
        .await
        .expect("plan overview")
        .plan_file;
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Entered plan mode"))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("update diff UI"))
    );

    tokio::fs::write(&plan_path, "# Plan\n\nUpdate diff UI.\n")
        .await
        .expect("write plan");
    state
        .handle_command(&app_server, "/plan", &local_command_tx)
        .await
        .expect("plan command starts");
    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("plan view event should arrive")
        .expect("plan view event");
    let submit_prompt = state.apply_local_command_event(event.event);

    assert_eq!(state.status_line, "Plan loaded.");
    assert_eq!(submit_prompt, None);
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Current Plan:")
                && message.content.contains("Update diff UI."))
    );
}

#[tokio::test]
async fn add_dir_slash_command_adds_session_working_directory() {
    let home_dir = test_temp_path("add-dir-home");
    let cwd = test_temp_path("add-dir-workspace");
    let extra = test_temp_path("add-dir-extra");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::create_dir_all(&extra)
        .await
        .expect("create extra");

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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();
    let command = format!("/add-dir {}", extra.display());

    state
        .handle_command(&app_server, &command, &local_command_tx)
        .await
        .expect("add-dir command succeeds");

    let extra = std::fs::canonicalize(extra).expect("canonical extra");
    assert_eq!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .additional_directories,
        vec![extra.clone()]
    );
    assert!(state.status_line.contains("Added "));
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains(&extra.display().to_string()))
    );
}

#[tokio::test]
async fn diff_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("diff-home");
    let cwd = test_temp_path("diff-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    run_git_test_command(&cwd, &["init"]);
    std::fs::write(cwd.join("tracked.rs"), "fn main() {}\n").expect("write tracked");
    run_git_test_command(&cwd, &["add", "tracked.rs"]);
    std::fs::write(
        cwd.join("tracked.rs"),
        "fn main() {\n    println!(\"hi\");\n}\n",
    )
    .expect("modify tracked");
    std::fs::write(cwd.join("scratch.txt"), "scratch\n").expect("write untracked");

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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/diff", &local_command_tx)
        .await
        .expect("diff command starts");

    assert_eq!(state.status_line, "Loading workspace diff...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("diff event should arrive")
        .expect("diff event");
    state.apply_local_command_event(event.event);

    let Some(OverlayState::Diff(diff)) = state.overlay.as_ref() else {
        panic!("diff overlay should open");
    };
    let files = diff_files_for_overlay(diff);
    assert!(files.iter().any(|file| file.path == "tracked.rs"));
    assert!(files.iter().any(|file| file.path == "scratch.txt"));
    assert!(files.iter().any(|file| {
        file.path == "scratch.txt"
            && file
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.content == "scratch")
    }));
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("/diff"))
    );
    assert!(state.status_line.starts_with("Opened diff mode:"));
}

#[tokio::test]
async fn ctrl_c_in_diff_overlay_uses_global_active_turn_interrupt_path() {
    let home_dir = test_temp_path("diff-ctrl-c-home");
    let cwd = test_temp_path("diff-ctrl-c-workspace");
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
    let bootstrap = app_server.bootstrap_typed(None).await.expect("bootstrap");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    state.overlay = Some(OverlayState::Diff(DiffOverlayState::new(WorkspaceDiff {
        cwd,
        status: String::new(),
        staged_diff: String::new(),
        unstaged_diff: String::new(),
        untracked_files: Vec::new(),
    })));
    let (_turn_tx, turn_rx) = mpsc::unbounded_channel();
    let mut turn_events = Some(turn_rx);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut turn_events,
            &local_command_tx,
        )
        .await
        .expect("ctrl-c");

    assert!(turn_events.is_none());
    assert!(state.overlay.is_none());
    assert_eq!(state.status_line, "Turn interrupted.");
}

#[tokio::test]
async fn doctor_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("doctor-home");
    let cwd = test_temp_path("doctor-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/doctor", &local_command_tx)
        .await
        .expect("doctor command starts");

    assert_eq!(state.status_line, "Running doctor...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("doctor event should arrive")
        .expect("doctor event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Doctor summary:"))
    );
    assert!(state.status_line.starts_with("Doctor "));
}

#[tokio::test]
async fn clear_slash_command_starts_fresh_session_and_preserves_allow_all() {
    let home_dir = test_temp_path("clear-home");
    let cwd = test_temp_path("clear-workspace");
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
    app_server.app_server().unwrap().set_allow_all(true);

    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let previous_session_id = state.session_id.clone();
    state.push_local_system_message("old visible context".to_string());
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/clear", &local_command_tx)
        .await
        .expect("clear command succeeds");

    assert_ne!(state.session_id, previous_session_id);
    assert!(app_server.app_server().unwrap().allow_all());
    assert_eq!(
        state.status_line,
        "Conversation cleared. Allow-all remains enabled."
    );
    let info = state
        .clear_session_info
        .as_ref()
        .expect("should have clear info");
    assert_eq!(info.session_id, previous_session_id);
    assert!(state.transcript_ui.emission.needs_scrollback_clear);
}

#[tokio::test]
async fn allow_all_slash_command_uses_registry_and_canonicalized_alias() {
    let home_dir = test_temp_path("allow-all-home");
    let cwd = test_temp_path("allow-all-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/yolo on", &local_command_tx)
        .await
        .expect("allow-all command succeeds");

    assert!(app_server.app_server().unwrap().allow_all());
    assert_eq!(state.status_line, "Allow-all mode enabled.");
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("/allow-all on"))
    );
}

#[tokio::test]
async fn effort_slash_command_updates_runtime_effort() {
    let home_dir = test_temp_path("effort-command-home");
    let cwd = test_temp_path("effort-command-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/effort high", &local_command_tx)
        .await
        .expect("effort command succeeds");

    assert_eq!(
        app_server.app_server().unwrap().effort_level(),
        Some(EffortLevel::High)
    );
    assert_eq!(
        state.status_line,
        "Set effort level to high: Comprehensive implementation with extensive testing and documentation"
    );
    let transcript = plain_text_lines(&state.transcript_lines(90)).join("\n");
    assert!(transcript.contains("❯ /effort high"), "{transcript}");
    assert!(
        !transcript.contains("└  Set effort level to high: Comprehensive"),
        "{transcript}"
    );

    state
        .handle_command(&app_server, "/effort auto", &local_command_tx)
        .await
        .expect("effort auto succeeds");

    assert_eq!(app_server.app_server().unwrap().effort_level(), None);
    assert_eq!(state.status_line, "Effort level set to auto.");
}

#[tokio::test]
async fn keybindings_slash_command_creates_typescript_compatible_template() {
    let home_dir = test_temp_path("keybindings-command-home");
    let cwd = test_temp_path("keybindings-command-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/keybindings", &local_command_tx)
        .await
        .expect("keybindings command succeeds");

    let path = home_dir.join("keybindings.json");
    let request = state
        .take_external_editor_request()
        .expect("editor request");
    assert_eq!(request.path, path);
    assert_eq!(request.command, "/keybindings");
    assert!(matches!(
        request.target,
        ExternalEditorTarget::Keybindings { created: true }
    ));

    let contents = tokio::fs::read_to_string(&path)
        .await
        .expect("read keybindings");
    let value: serde_json::Value = serde_json::from_str(&contents).expect("valid json");
    assert_eq!(
        value["$schema"],
        "https://www.schemastore.org/claude-code-keybindings.json"
    );
    assert_eq!(
        value["$docs"],
        "https://code.claude.com/docs/en/keybindings"
    );
    let bindings = value["bindings"].as_array().expect("bindings array");
    assert!(bindings.iter().any(|block| block["context"] == "Chat"));
    assert!(bindings.iter().any(|block| block["context"] == "Select"));
    assert!(contents.ends_with('\n'));
    assert!(state.status_line.starts_with("Opening keybindings file "));
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(!transcript.contains("❯ /keybindings"), "{transcript}");
}

#[tokio::test]
async fn keybindings_slash_command_opens_editor_without_subcommand() {
    let home_dir = test_temp_path("keybindings-noarg-home");
    let cwd = test_temp_path("keybindings-noarg-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/keybindings", &local_command_tx)
        .await
        .expect("keybindings command succeeds");

    let request = state
        .take_external_editor_request()
        .expect("editor request");
    assert_eq!(request.path, home_dir.join("keybindings.json"));
    assert_eq!(request.command, "/keybindings");
    assert!(matches!(
        request.target,
        ExternalEditorTarget::Keybindings { created: true }
    ));
    assert!(state.overlay.is_none());
    assert!(state.status_line.starts_with("Opening keybindings file "));
    let rendered = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(!rendered.contains("❯ /keybindings"), "{rendered}");
}

#[tokio::test]
async fn keybindings_slash_command_does_not_overwrite_existing_file() {
    let home_dir = test_temp_path("keybindings-existing-home");
    let cwd = test_temp_path("keybindings-existing-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    let path = home_dir.join("keybindings.json");
    let existing = r#"{"bindings":[{"context":"Chat","bindings":{"enter":"chat:submit"}}]}"#;
    tokio::fs::write(&path, existing)
        .await
        .expect("write existing keybindings");

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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/keybindings", &local_command_tx)
        .await
        .expect("keybindings command succeeds");

    let request = state
        .take_external_editor_request()
        .expect("editor request");
    assert!(matches!(
        request.target,
        ExternalEditorTarget::Keybindings { created: false }
    ));
    assert_eq!(
        tokio::fs::read_to_string(&path)
            .await
            .expect("read existing"),
        existing
    );
}
