use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[tokio::test]
async fn instructions_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("instructions-home");
    let cwd = test_temp_path("instructions-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(cwd.join("CLAUDE.md"), "Use local project instructions.")
        .await
        .expect("write claude md");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
            provider_allow_network: Some(false),
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
        .handle_command(&app_server, "/instructions", &local_command_tx)
        .await
        .expect("instructions command starts");

    assert_eq!(state.status_line, "Loading instructions...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("instructions event should arrive")
        .expect("instructions event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("# System prompt"))
    );
    assert!(state.messages.iter().any(|message| {
        message
            .content
            .contains("You are Orb Code, a terminal coding assistant")
    }));
    assert!(
        state
            .messages
            .iter()
            .any(|message| { message.content.contains("Current working directory:") })
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("# Context message"))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("# claudeMd"))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Use local project instructions."))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("# Tools"))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("## Bash"))
    );
    assert!(
        state
            .messages
            .iter()
            .all(|message| !message.content.contains("Context snapshot:"))
    );
    assert!(
        state
            .messages
            .iter()
            .all(|message| !message.content.contains("Token context:"))
    );
    assert!(
        state
            .messages
            .iter()
            .all(|message| !message.content.contains("blocking limit:"))
    );
    let note = parse_local_transcript_note(state.messages.last().expect("instructions note"))
        .expect("slash command output note");
    let rendered = plain_text_lines(&render_local_transcript_note_lines(note, 100, false));
    let rendered_text = rendered.join("\n");
    assert_eq!(rendered[0], "❯ /instructions");
    assert!(
        !rendered_text.contains("Instructions loaded."),
        "{rendered_text}"
    );
    assert!(
        rendered.iter().any(|line| line == "   System prompt"),
        "{rendered_text}"
    );
    assert!(
        rendered
            .iter()
            .all(|line| !line.contains("     ▎ System prompt")),
        "{rendered_text}"
    );
    assert_eq!(state.status_line, "Instructions loaded.");
}

#[tokio::test]
async fn memory_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("memory-home");
    let workspace = test_temp_path("memory-workspace");
    let extra = test_temp_path("memory-extra");
    let cwd = workspace.join("nested");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::create_dir_all(workspace.join(".claude").join("rules"))
        .await
        .expect("create rules");
    tokio::fs::create_dir_all(&extra)
        .await
        .expect("create extra");
    tokio::fs::write(home_dir.join("CLAUDE.md"), "User memory\n")
        .await
        .expect("write user memory");
    tokio::fs::write(workspace.join("CLAUDE.md"), "Project memory\n")
        .await
        .expect("write project memory");
    tokio::fs::write(
        workspace.join(".claude").join("CLAUDE.md"),
        "Dot claude memory\n",
    )
    .await
    .expect("write dot claude memory");
    tokio::fs::write(
        workspace.join(".claude").join("rules").join("style.md"),
        "Rule memory\n",
    )
    .await
    .expect("write rule memory");
    tokio::fs::write(cwd.join("CLAUDE.local.md"), "Local memory\n")
        .await
        .expect("write local memory");
    tokio::fs::write(extra.join("CLAUDE.md"), "Extra memory\n")
        .await
        .expect("write extra memory");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            add_dirs: vec![extra],
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
        .handle_command(&app_server, "/memory", &local_command_tx)
        .await
        .expect("memory command starts");

    assert_eq!(state.status_line, "Loading memory...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("memory event should arrive")
        .expect("memory event");
    state.apply_local_command_event(event.event);

    let Some(OverlayState::MemoryPicker(picker)) = state.overlay.as_ref() else {
        panic!("memory picker should open");
    };
    assert!(state.overlay_hides_footer());
    let rendered_paths = picker
        .items
        .iter()
        .map(|item| match item {
            MemoryPickerItem::File(memory) => memory.path.display().to_string(),
            MemoryPickerItem::OpenAutoMemoryFolder(path) => path.display().to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered_paths.contains("CLAUDE.md"));
    assert!(rendered_paths.contains(".claude"));
    assert!(rendered_paths.contains("style.md"));
    assert!(rendered_paths.contains("CLAUDE.local.md"));
    assert!(rendered_paths.contains("memory-extra"));
    assert!(
        picker
            .items
            .iter()
            .any(|item| matches!(item, MemoryPickerItem::OpenAutoMemoryFolder(_)))
    );
    assert!(state.messages.is_empty());
    assert_eq!(
        state.status_line,
        "Memory selector: Enter confirm, Esc cancel."
    );
    let menu_lines = plain_text_lines(&memory_picker_lines(picker, &state.cwd, 72));
    assert!(menu_lines.iter().any(|line| line == "Memory"));
    assert!(
        menu_lines
            .iter()
            .any(|line| line.contains("Auto-memory: on"))
    );
    assert!(
        menu_lines
            .iter()
            .any(|line| line.contains("❯ 1. User memory [edit,"))
    );
    assert!(
        menu_lines
            .iter()
            .any(|line| line.contains("Open auto-memory folder"))
    );
    assert!(
        menu_lines
            .iter()
            .any(|line| line.contains("Enter to confirm · Esc to cancel"))
    );
    assert!(!menu_lines.iter().any(|line| line.contains("Filter:")));
    let styled_menu = memory_picker_lines(picker, &state.cwd, 72);
    let auto_memory_line = styled_menu
        .iter()
        .find(|line| plain_text_line(line).contains("Auto-memory:"))
        .expect("auto-memory line");
    assert_eq!(auto_memory_line.spans[2].style, Style::default());
    let selected_line = styled_menu
        .iter()
        .find(|line| plain_text_line(line).contains("User memory"))
        .expect("selected memory line");
    assert_eq!(selected_line.spans[1].style, Style::default());
    assert_eq!(selected_line.spans[3].style, Style::default());
    let footer_line = styled_menu
        .iter()
        .find(|line| plain_text_line(line).contains("Enter to confirm"))
        .expect("footer line");
    assert_eq!(footer_line.spans[0].style, Style::default());
    assert_eq!(footer_line.spans[3].style, Style::default());
}

#[tokio::test]
async fn status_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("status-home");
    let cwd = test_temp_path("status-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            provider_allow_network: Some(false),
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
        .handle_command(&app_server, "/status", &local_command_tx)
        .await
        .expect("status command starts");

    assert_eq!(state.status_line, "Loading status...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("status event should arrive")
        .expect("status event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Status:"))
    );
    assert_eq!(state.status_line, "Status loaded.");
}

#[tokio::test]
async fn context_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("context-home");
    let cwd = test_temp_path("context-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            provider_allow_network: Some(false),
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
        .handle_command(&app_server, "/context", &local_command_tx)
        .await
        .expect("context command starts");

    assert_eq!(state.status_line, "Loading context usage...");
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Loading context usage..."))
    );

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("context event should arrive")
        .expect("context event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Context usage loaded."))
    );
    assert!(
        state
            .messages
            .iter()
            .all(|message| !message.content.contains("Loading context usage..."))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Free space:"))
    );
    assert!(
        state
            .messages
            .iter()
            .all(|message| !message.content.contains("Context diagnostics:"))
    );
    assert_eq!(state.status_line, "Context usage loaded.");
}

#[tokio::test]
async fn context_slash_command_full_flag_shows_diagnostics() {
    let home_dir = test_temp_path("context-full-home");
    let cwd = test_temp_path("context-full-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            provider_allow_network: Some(false),
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
        .handle_command(&app_server, "/context --full", &local_command_tx)
        .await
        .expect("context command starts");
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Loading context usage..."))
    );

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("context event should arrive")
        .expect("context event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Context diagnostics:"))
    );
    assert!(
        state
            .messages
            .iter()
            .all(|message| !message.content.contains("Loading context usage..."))
    );
}

#[tokio::test]
async fn usage_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("usage-home");
    let cwd = test_temp_path("usage-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            provider_allow_network: Some(false),
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
        .handle_command(&app_server, "/usage", &local_command_tx)
        .await
        .expect("usage command starts");

    assert_eq!(state.status_line, "Loading usage...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("usage event should arrive")
        .expect("usage event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Usage loaded."))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("No provider token usage"))
    );
    assert_eq!(state.status_line, "Usage loaded.");
}

#[tokio::test]
async fn stats_slash_command_runs_asynchronously() {
    let home_dir = test_temp_path("stats-home");
    let cwd = test_temp_path("stats-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            provider_allow_network: Some(false),
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
        .handle_command(&app_server, "/stats", &local_command_tx)
        .await
        .expect("stats command starts");

    assert_eq!(state.status_line, "Loading stats...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("stats event should arrive")
        .expect("stats event");
    state.apply_local_command_event(event.event);

    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Last 180 days"))
    );
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("messages."))
    );
    assert_eq!(state.status_line, "Stats loaded.");
}

#[tokio::test]
async fn tools_slash_command_uses_local_output_registry() {
    let home_dir = test_temp_path("tools-home");
    let cwd = test_temp_path("tools-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            provider_allow_network: Some(false),
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
        .handle_command(&app_server, "/tools", &local_command_tx)
        .await
        .expect("tools command succeeds");

    assert_eq!(state.status_line, "Listed tool registry.");
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("Listed tool registry."))
    );
}
