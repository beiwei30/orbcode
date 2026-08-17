use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[test]
fn normal_mode_gj_and_gk_move_by_visual_rows() {
    let width = input_inner_width().max(8);
    let wrapped = "a".repeat(width + 5);
    let input = format!("{wrapped}\nZ");

    let mut state = normal_state(&input, 0);
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    let visual_row_offset = state.input_cursor;
    assert!(visual_row_offset > 0);
    assert!(visual_row_offset < current_line_end_boundary(&input, 0));

    let mut logical = normal_state(&input, 0);
    logical
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(logical.input_cursor, wrapped.len() + 1);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input_cursor, 0);
}

#[test]
fn normal_mode_operator_gj_is_characterwise_not_linewise() {
    let width = input_inner_width().max(8);
    let wrapped = "a".repeat(width + 5);
    let input = format!("{wrapped}\nZ");
    let mut state = normal_state(&input, 0);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, format!("{}\nZ", "a".repeat(5)));
}

#[test]
fn normal_mode_da_paren_removes_delimiters_and_contents() {
    let mut state = normal_state("call(foo)", "call(f".len());

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('('),
            KeyModifiers::SHIFT,
        ))
        .unwrap();

    assert_eq!(state.input, "call");
}

#[test]
fn normal_mode_yi_angle_yanks_inner_text_object() {
    let mut state = normal_state("<abc>", 2);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('<'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();

    assert_eq!(state.vim_state.register, "abc");
    assert!(!state.vim_state.register_is_linewise);
}

#[test]
fn normal_mode_f_waits_for_target_and_moves_cursor() {
    let mut state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: "hello world".to_string(),
        input_cursor: 0,
        input_tail_pinned: false,
        input_area: Rect::ZERO,
        input_selection: None,
        desired_column: None,
        prompt_history: Vec::new(),
        prompt_history_index: None,
        slash_command_selected: 0,
        steered_followups: std::collections::VecDeque::new(),
        queued_followups: std::collections::VecDeque::new(),
        pending_assistant: String::new(),
        compact_started_at: None,
        active_thinking: None,
        live_tool_cells: LiveToolCells::default(),
        in_progress_tool_use_ids: HashSet::new(),
        pending_hook_progress: Vec::new(),
        hook_progress_by_message_id: HashMap::new(),
        history_flushed_message_count: 0,
        retained_visible_transcript_cells: 0,
        focus_latest_message_start: false,
        pending_history_flush: false,
        overlay: None,
        status_line: String::new(),
        status_line_set_at: None,
        ui_version: "2.1.888".to_string(),
        cwd_display: "~".to_string(),
        model_display_name: "model".to_string(),
        context_window_options: ContextWindowOptions::default(),
        max_output_token_options: MaxOutputTokenOptions::default(),
        token_warning_options: TokenWarningOptions::default(),
        default_provider_label: "anthropic".to_string(),
        show_update_notice: false,
        expanded_tool_details: false,
        request_in_flight: false,
        spinner_frame: 0,
        spinner_verb_index: 0,
        request_count: 0,
        request_started_at: None,
        streamed_response_chars: 0,
        request_token_direction: RequestTokenDirection::Up,
        current_turn_total_tokens: 0,
        last_provider: None,
        last_usage: None,
        editor_mode: EditorMode::Normal,
        normal_pending: None,
        last_find: None,
        normal_count: None,
        vim_state: VimRuntimeState::default(),
        external_editor_request: None,
        slash_suggestion_lines_cache: SlashSuggestionLinesCache::default(),
        mcp_slash_suggestions: McpSlashSuggestionCatalog::default(),
        mcp_slash_suggestion_revision: 0,
        mcp_slash_suggestion_refresh_key: None,
        task_panel: TaskPanelState::new(Some("test-session"), true),
        background_agent_panel: BackgroundAgentPanelState::new(),
        transcript_task_cards: TranscriptTaskCardsState::new(),
        status: StatusLineState::default(),
        statusline_command: None,
        statusline_refresh_interval: std::time::Duration::from_secs(30),
        clear_session_info: None,
    };

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(
        state.normal_pending,
        Some(NormalPending::Find(FindKind::Forward))
    );

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input_cursor, 4);
    assert_eq!(
        state.last_find,
        Some(LastFind {
            kind: FindKind::Forward,
            target: 'o'
        })
    );
}

#[tokio::test]
async fn vim_slash_command_toggles_editor_mode_and_persists_setting() {
    let home_dir = test_temp_path("vim-command-home");
    let cwd = test_temp_path("vim-command-workspace");
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

    assert_eq!(state.editor_mode, EditorMode::Standard);

    state
        .handle_command(&app_server, "/vim", &local_command_tx)
        .await
        .expect("vim command succeeds");

    assert_eq!(state.editor_mode, EditorMode::Insert);
    assert_eq!(
        app_server.app_server().unwrap().editor_mode_setting(),
        orbcode_config::EditorModeSetting::Vim
    );
    assert_eq!(
        state.status_line,
        "Editor mode set to vim. Use Escape key to toggle between INSERT and NORMAL modes."
    );
    let settings: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(home_dir.join("settings.json"))
            .await
            .expect("read settings"),
    )
    .expect("settings json");
    assert_eq!(settings["editorMode"], "vim");

    assert_eq!(state.handle_escape_key(false), EscapeAction::StayInTui);
    assert_eq!(state.editor_mode, EditorMode::Normal);

    state
        .handle_command(&app_server, "/vim", &local_command_tx)
        .await
        .expect("vim command disables mode");

    assert_eq!(state.editor_mode, EditorMode::Standard);
    assert_eq!(
        app_server.app_server().unwrap().editor_mode_setting(),
        orbcode_config::EditorModeSetting::Normal
    );
    assert_eq!(
        state.status_line,
        "Editor mode set to normal. Using standard keyboard bindings."
    );
}

#[tokio::test]
async fn ctrl_o_toggles_tool_details_before_vim_normal_mode_open_line() {
    let home_dir = test_temp_path("ctrl-o-vim-normal-home");
    let cwd = test_temp_path("ctrl-o-vim-normal-workspace");
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
    state.editor_mode = EditorMode::Normal;
    state.input = "abc".to_string();
    state.input_cursor = state.input.len();
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();
    let mut turn_events = None;

    state
        .handle_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            &mut turn_events,
            &local_command_tx,
        )
        .await
        .expect("handle ctrl-o");

    assert!(state.overlay.is_some());
    assert_eq!(state.input, "abc");
    assert_eq!(state.input_cursor, 3);
    assert_eq!(state.editor_mode, EditorMode::Normal);
}
