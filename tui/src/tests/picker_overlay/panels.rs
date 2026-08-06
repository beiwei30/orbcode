use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[tokio::test]
async fn memory_picker_creates_missing_file_and_requests_editor() {
    let home_dir = test_temp_path("memory-create-home");
    let cwd = test_temp_path("memory-create-workspace");
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
    state.overlay = Some(OverlayState::MemoryPicker(MemoryPickerState::new(
        "/memory",
        app_server
            .app_server()
            .unwrap()
            .memory_overview()
            .await
            .expect("memory overview"),
    )));
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();
    let mut turn_events = None;

    state
        .handle_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut turn_events,
            &local_command_tx,
        )
        .await
        .expect("enter memory selection");

    let selected_path = home_dir.join("CLAUDE.md");
    assert!(tokio::fs::try_exists(&selected_path).await.expect("stat"));
    let request = state
        .take_external_editor_request()
        .expect("editor request");
    assert_eq!(request.path, selected_path);
    assert_eq!(request.command, "/memory");
    assert!(matches!(request.target, ExternalEditorTarget::Memory));
    assert!(state.overlay.is_none());
}

#[tokio::test]
async fn ctrl_t_toggle_collapses_and_re_expands_task_panel() {
    use orbcode_app_server::{TaskListSnapshot, TaskListSummary, TaskStatusKind, TaskView};

    let home_dir = test_temp_path("task-panel-toggle-home");
    let cwd = test_temp_path("task-panel-toggle-cwd");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);

    let snapshot = TaskListSnapshot {
        task_list_id: "test".to_string(),
        directory: PathBuf::from("/tmp/orbcode-task-panel-tests"),
        tasks: vec![TaskView {
            id: "1".to_string(),
            subject: "Build".to_string(),
            description: String::new(),
            active_form: None,
            owner: None,
            status: TaskStatusKind::InProgress,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            open_blockers: Vec::new(),
        }],
        summary: TaskListSummary {
            total: 1,
            completed: 0,
            in_progress: 1,
            pending: 0,
        },
        fingerprint: 1,
    };
    state.task_panel.apply_snapshot(snapshot, Instant::now());
    assert!(
        !state.task_panel.is_visible(),
        "stale tasks should not auto-show"
    );
    state.toggle_task_panel();
    assert!(
        state.task_panel.is_visible(),
        "toggle should reveal stale tasks"
    );
    assert!(state.task_panel.is_expanded());
    state.toggle_task_panel();
    assert!(!state.task_panel.is_expanded());
    state.toggle_task_panel();
    assert!(state.task_panel.is_expanded());
}

#[tokio::test]
async fn task_panel_hydrates_from_disk_on_resume() {
    use orbcode_app_server::session_task_list_id;

    let home_dir = test_temp_path("task-panel-resume-home");
    let cwd = test_temp_path("task-panel-resume-cwd");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd.clone(),
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir.clone()),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap");
    let session_id = bootstrap.session.session_id.clone();

    let task_list_id = session_task_list_id(Some(&session_id));
    let task_dir = home_dir.join("tasks").join(&task_list_id);
    tokio::fs::create_dir_all(&task_dir)
        .await
        .expect("create task dir");
    tokio::fs::write(
        task_dir.join("1.json"),
        r#"{"id":"1","subject":"Resume me","description":"","status":"in_progress","blocks":[],"blockedBy":[]}"#,
    )
    .await
    .expect("seed task file");

    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);

    assert!(state.task_panel.needs_refresh(Instant::now()));
    state.task_panel.refresh(&app_server).await;
    let snapshot = state
        .task_panel
        .snapshot()
        .expect("snapshot loaded from disk");
    assert_eq!(snapshot.summary.in_progress, 1);
    assert_eq!(snapshot.tasks[0].id, "1");
    assert_eq!(snapshot.tasks[0].subject, "Resume me");
}

#[tokio::test]
async fn task_panel_id_honors_environment_override() {
    use orbcode_tools::workspace_task_list_id;

    let cwd_a = PathBuf::from("/Users/alice/projects/orbcode");
    let cwd_b = PathBuf::from("/Users/bob/another/dir");
    let default_id = workspace_task_list_id(&cwd_a);
    assert!(default_id.contains("orbcode"));

    // SAFETY: tests in this crate run single-threaded; the env var is restored
    // before any other test observes it.
    unsafe {
        std::env::set_var("ORBCODE_TASK_LIST_ID", "shared-team-tasklist");
    }
    let from_env_a = workspace_task_list_id(&cwd_a);
    let from_env_b = workspace_task_list_id(&cwd_b);
    unsafe {
        std::env::remove_var("ORBCODE_TASK_LIST_ID");
    }

    assert_eq!(from_env_a, "shared-team-tasklist");
    assert_eq!(from_env_b, "shared-team-tasklist");
}

#[test]
fn rewind_picker_builds_entries_and_restores_selected_turn() {
    let messages = vec![
        TranscriptMessage::new(MessageRole::User, "first prompt".to_string()),
        TranscriptMessage::new(MessageRole::Assistant, "first reply".to_string()),
        TranscriptMessage::new(MessageRole::User, "second prompt".to_string()),
    ];
    let mut picker = RewindPickerState::from_messages("/rewind", "session", &messages)
        .expect("user turns produce a picker");

    // One entry per user turn, each keeping every message before that turn.
    assert_eq!(picker.entries.len(), 2);
    assert_eq!(picker.entries[0].keep_messages, 0);
    assert_eq!(picker.entries[0].prompt, "first prompt");
    assert_eq!(picker.entries[1].keep_messages, 2);
    assert_eq!(picker.entries[1].prompt, "second prompt");
    // The rewind anchors on the persisted message id (not the display index),
    // so truncation resolves against the persisted record.
    assert_eq!(picker.entries[0].anchor_id, messages[0].id);
    assert_eq!(picker.entries[1].anchor_id, messages[2].id);
    // Selection defaults to the most recent user turn.
    assert_eq!(picker.selected, 1);

    // Up moves to the earlier turn; Enter restores it.
    apply_rewind_picker_key(
        &mut picker,
        &crossterm::event::KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
    );
    match apply_rewind_picker_key(
        &mut picker,
        &crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    ) {
        RewindPickerKeyAction::Rewind {
            keep_messages,
            restore_prompt,
            ..
        } => {
            assert_eq!(keep_messages, 0);
            assert_eq!(restore_prompt, "first prompt");
        }
        _ => panic!("Enter should produce a rewind action"),
    }
}

#[test]
fn rewind_picker_requires_a_user_turn() {
    let messages = vec![TranscriptMessage::new(
        MessageRole::Assistant,
        "only assistant".to_string(),
    )];
    assert!(RewindPickerState::from_messages("/rewind", "session", &messages).is_none());
}

#[test]
fn render_hook_discovery_formats_hooks_and_warnings() {
    use orbcode_app_server_client::{
        DiscoveredHook, HookDiscovery, HookDiscoveryWarning, HookLayer, HookProvenance,
        HookValidationStatus,
    };

    let discovery = HookDiscovery {
        hooks: vec![
            DiscoveredHook {
                event: "PreToolUse".to_string(),
                provenance: HookProvenance::Settings(HookLayer::User),
                matcher: Some("Bash".to_string()),
                command: "echo check".to_string(),
                trusted: true,
                validation: HookValidationStatus::Valid,
            },
            DiscoveredHook {
                event: "PostToolUse".to_string(),
                provenance: HookProvenance::Agent {
                    name: "reviewer".to_string(),
                },
                matcher: None,
                command: "echo done".to_string(),
                trusted: false,
                validation: HookValidationStatus::Valid,
            },
        ],
        warnings: vec![HookDiscoveryWarning {
            provenance: HookProvenance::Settings(HookLayer::Project),
            event: "Stop".to_string(),
            message: "empty command".to_string(),
        }],
    };

    let rendered = render_hook_discovery(&discovery);
    assert!(rendered.contains("empty command"));
    assert!(rendered.contains("PreToolUse"));
    assert!(rendered.contains("Bash"));
    assert!(rendered.contains("echo check"));
    assert!(rendered.contains("trusted"));
    assert!(rendered.contains("untrusted"));
}

#[test]
fn render_hook_discovery_shows_empty_message() {
    use orbcode_app_server_client::HookDiscovery;

    let discovery = HookDiscovery::default();
    let rendered = render_hook_discovery(&discovery);
    assert_eq!(rendered, "No hooks configured.");
}

#[test]
fn render_skill_definitions_formats_skills_with_source() {
    use orbcode_app_server_client::{SkillDefinition, SkillSource};

    let definitions = vec![
        SkillDefinition {
            name: "code-review".to_string(),
            description: Some("Review code for quality".to_string()),
            when_to_use: Some("When the user asks for code review".to_string()),
            source: SkillSource::Project,
            ..SkillDefinition::default()
        },
        SkillDefinition {
            name: "init".to_string(),
            description: Some("Initialize project".to_string()),
            when_to_use: None,
            source: SkillSource::Bundled,
            ..SkillDefinition::default()
        },
    ];

    let rendered = render_skill_definitions(&definitions);
    assert!(rendered.contains("[project]"));
    assert!(rendered.contains("code-review"));
    assert!(rendered.contains("When the user asks for code review"));
    assert!(rendered.contains("[bundled]"));
    assert!(rendered.contains("init"));
}

#[test]
fn render_skill_definitions_shows_empty_message() {
    use orbcode_app_server_client::SkillDefinition;

    let rendered = render_skill_definitions(&Vec::<SkillDefinition>::new());
    assert_eq!(rendered, "No skills available.");
}

#[test]
fn render_agent_definitions_formats_agents_with_tools_and_model() {
    use orbcode_app_server_client::{AgentDefinition, AgentSource};

    let definitions = vec![AgentDefinition {
        agent_type: "general-purpose".to_string(),
        description: "General purpose agent".to_string(),
        prompt: "You are a helpful assistant.".to_string(),
        tools: None,
        disallowed_tools: None,
        model: None,
        permission_mode: None,
        skills: Vec::new(),
        mcp_server_names: None,
        hooks: std::collections::BTreeMap::new(),
        source: AgentSource::BuiltIn,
        path: None,
    }];

    let rendered = render_agent_definitions(&definitions);
    assert!(rendered.contains("[built-in]"));
    assert!(rendered.contains("general-purpose"));
    assert!(rendered.contains("You are a helpful assistant."));
}

#[test]
fn render_agent_definitions_shows_empty_message() {
    use orbcode_app_server_client::AgentDefinition;

    let rendered = render_agent_definitions(&Vec::<AgentDefinition>::new());
    assert_eq!(rendered, "No agent definitions available.");
}

// ---------------------------------------------------------------------------
// Agent warnings render
// ---------------------------------------------------------------------------

#[test]
fn render_agent_definitions_with_warnings_appends_warnings_section() {
    use orbcode_app_server_client::{
        AgentDefinition, AgentLoadWarning, AgentSource, AgentWarningKind,
    };
    use std::path::PathBuf;

    use crate::render::slash_output::render_agent_definitions_with_warnings;

    let definitions = vec![AgentDefinition {
        agent_type: "general-purpose".to_string(),
        description: "General purpose agent".to_string(),
        prompt: "You are a helpful assistant.".to_string(),
        tools: None,
        disallowed_tools: None,
        model: None,
        permission_mode: None,
        skills: Vec::new(),
        mcp_server_names: None,
        hooks: std::collections::BTreeMap::new(),
        source: AgentSource::BuiltIn,
        path: None,
    }];
    let warnings = vec![AgentLoadWarning {
        kind: AgentWarningKind::MissingField,
        source: AgentSource::ProjectSettings,
        path: Some(PathBuf::from(".claude/agents/bad.md")),
        agent_type: Some("bad".to_string()),
        message: "missing description".to_string(),
    }];

    let rendered = render_agent_definitions_with_warnings(&definitions, &warnings);
    assert!(rendered.contains("[built-in]"));
    assert!(rendered.contains("general-purpose"));
    assert!(rendered.contains("Warnings:"));
    assert!(rendered.contains("agent 'bad'"));
    assert!(rendered.contains("missing description"));
    assert!(rendered.contains(".claude/agents/bad.md"));
}

#[test]
fn render_agent_definitions_with_no_warnings_omits_section() {
    use orbcode_app_server_client::{AgentDefinition, AgentLoadWarning, AgentSource};

    use crate::render::slash_output::render_agent_definitions_with_warnings;

    let definitions = vec![AgentDefinition {
        agent_type: "general-purpose".to_string(),
        description: "General purpose agent".to_string(),
        prompt: "You are a helpful assistant.".to_string(),
        tools: None,
        disallowed_tools: None,
        model: None,
        permission_mode: None,
        skills: Vec::new(),
        mcp_server_names: None,
        hooks: std::collections::BTreeMap::new(),
        source: AgentSource::BuiltIn,
        path: None,
    }];

    let rendered =
        render_agent_definitions_with_warnings(&definitions, &Vec::<AgentLoadWarning>::new());
    assert!(rendered.contains("[built-in]"));
    assert!(!rendered.contains("Warnings:"));
}

// ---------------------------------------------------------------------------
// Output style picker locked state
// ---------------------------------------------------------------------------

#[test]
fn permission_picker_lines_at_wide_200_col() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server_client::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: true,
            provider_allow_network: false,
            allow_tools: false,
            allowed_rules: Vec::new(),
            denied_rules: Vec::new(),
            ask_rules: Vec::new(),
            additional_directories: Vec::new(),
        },
        allow_all: false,
        effective_rules: Default::default(),
        settings_allowed_rules: vec!["Bash(cargo test:*)".to_string()],
        settings_denied_rules: Vec::new(),
        startup_allowed_rules: vec!["Read(src/**)".to_string()],
        startup_denied_rules: Vec::new(),
        edited_allowed_rules: Vec::new(),
        edited_denied_rules: Vec::new(),
        runtime_allowed_rules: vec!["Grep(orbcode/**)".to_string()],
        runtime_denied_rules: Vec::new(),
        configured_additional_directories: Vec::new(),
        session_additional_directories: Vec::new(),
    };
    let mut picker = PermissionPickerState::new("/permissions", overview, Vec::new());
    let lines = picker.cached_lines(200);
    assert_eq!(lines.len(), PERMISSION_PICKER_PANEL_HEIGHT);
    let rendered = plain_text_lines(lines).join("\n");
    assert!(rendered.contains("Permissions"));
    assert!(rendered.contains("Allow"));
    assert!(rendered.contains("Bash(cargo test:*)"));
}

#[test]
fn background_jobs_overlay_list_renders_jobs_with_status() {
    use chrono::Utc;
    use orbcode_app_server::{
        BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus, ProviderId,
    };

    fn make_view(
        id: &str,
        prompt: &str,
        status: BackgroundTaskViewStatus,
        pid: Option<u32>,
        exit_code: Option<i32>,
    ) -> BackgroundTaskView {
        BackgroundTaskView {
            task_id: id.to_string(),
            session_id: "s1".to_string(),
            kind: BackgroundTaskViewKind::BackgroundJob,
            status,
            description: prompt.to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid,
            exit_code,
            signal: None,
            error: None,
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some(ProviderId::Anthropic),
            permission_mode: None,
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: None,
            log_tail: None,
            progress_events: None,
            workflow_steps: None,
        }
    }

    let jobs = vec![
        make_view(
            "aaaa1111-0000-0000-0000-000000000000",
            "Fix the authentication bug",
            BackgroundTaskViewStatus::Running,
            Some(12345),
            None,
        ),
        make_view(
            "bbbb2222-0000-0000-0000-000000000000",
            "Run the full test suite and report results",
            BackgroundTaskViewStatus::Completed,
            None,
            Some(0),
        ),
    ];
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 10);
    state.overlay = Some(OverlayState::BackgroundJobs(
        BackgroundJobsOverlayState::new(jobs, "s1".to_string()),
    ));
    let mut fixture = RenderMetricsFixture::new(100, 24);
    let metrics = fixture.draw(&mut state);
    assert!(metrics.initial_frame);
    assert!(metrics.output_bytes > 0);
}

#[test]
fn background_jobs_overlay_detail_renders_metadata_and_log() {
    use chrono::Utc;
    use orbcode_app_server::{
        BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus, ProviderId,
    };

    fn make_view(id: &str, status: BackgroundTaskViewStatus) -> BackgroundTaskView {
        BackgroundTaskView {
            task_id: id.to_string(),
            session_id: "s1".to_string(),
            kind: BackgroundTaskViewKind::BackgroundJob,
            status,
            description: "Fix bug".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid: Some(99),
            exit_code: None,
            signal: None,
            error: None,
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some(ProviderId::Anthropic),
            permission_mode: None,
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: None,
            log_tail: None,
            progress_events: None,
            workflow_steps: None,
        }
    }

    let jobs = vec![make_view("aaaa1111", BackgroundTaskViewStatus::Running)];
    let mut overlay_state = BackgroundJobsOverlayState::new(jobs, "s1".to_string());
    overlay_state.set_detail(BackgroundTaskView {
        task_id: "aaaa1111".to_string(),
        session_id: "s1".to_string(),
        kind: BackgroundTaskViewKind::BackgroundJob,
        status: BackgroundTaskViewStatus::Running,
        description: "Fix the authentication bug in login flow".to_string(),
        cwd: "/home/user/project".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        started_at: None,
        finished_at: None,
        pid: Some(99),
        exit_code: None,
        signal: None,
        error: None,
        model: Some("claude-sonnet-4-6".to_string()),
        provider: Some(ProviderId::Anthropic),
        permission_mode: Some("default".to_string()),
        agent_type: None,
        child_session_id: None,
        cancellation_reason: None,
        label: None,
        log_tail: Some(vec![
            "Reading file auth.rs...".to_string(),
            "Applying fix to validate_token()...".to_string(),
            "Running cargo test...".to_string(),
        ]),
        progress_events: None,
        workflow_steps: None,
    });
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 10);
    state.overlay = Some(OverlayState::BackgroundJobs(overlay_state));
    let mut fixture = RenderMetricsFixture::new(100, 30);
    let metrics = fixture.draw(&mut state);
    assert!(metrics.initial_frame);
    assert!(metrics.output_bytes > 0);
}

#[test]
fn background_jobs_overlay_empty_shows_placeholder() {
    let lines = background_jobs_list_lines(&[], 0, "", 80);
    let text = plain_text_lines(&lines).join("\n");
    assert!(text.contains("BACKGROUND JOBS"));
    assert!(text.contains("No background jobs"));
}

#[test]
fn background_jobs_overlay_close_restores_normal_state() {
    use chrono::Utc;
    use orbcode_app_server::{
        BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus, ProviderId,
    };

    let jobs = vec![BackgroundTaskView {
        task_id: "test".to_string(),
        session_id: "s1".to_string(),
        kind: BackgroundTaskViewKind::BackgroundJob,
        status: BackgroundTaskViewStatus::Running,
        description: "test".to_string(),
        cwd: "/tmp".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        started_at: None,
        finished_at: None,
        pid: None,
        exit_code: None,
        signal: None,
        error: None,
        model: Some("opus".to_string()),
        provider: Some(ProviderId::Anthropic),
        permission_mode: None,
        agent_type: None,
        child_session_id: None,
        cancellation_reason: None,
        label: None,
        log_tail: None,
        progress_events: None,
        workflow_steps: None,
    }];
    let mut state = normal_state("hello", 5);
    fill_long_transcript(&mut state, 10);
    state.overlay = Some(OverlayState::BackgroundJobs(
        BackgroundJobsOverlayState::new(jobs, "s1".to_string()),
    ));

    let mut fixture = RenderMetricsFixture::new(100, 24);
    let with_overlay = fixture.draw(&mut state);
    assert!(with_overlay.output_bytes > 0);

    state.overlay = None;
    let without_overlay = fixture.draw(&mut state);
    assert!(without_overlay.output_bytes > 0);
}

#[tokio::test]
// The clipboard assertion lock serializes this test against other clipboard
// tests that share a process-global capture buffer; it is intentionally held
// across the async setup, and each `#[tokio::test]` resumes on its own thread,
// so holding the std guard across await is safe here.
#[allow(clippy::await_holding_lock)]
async fn background_jobs_overlay_y_copies_selected_workflow_step_output() {
    use chrono::Utc;
    use crossterm::event::KeyEvent;
    use orbcode_app_server::{
        BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus, ProviderId,
    };
    use orbcode_protocol::{WorkflowStepView, WorkflowStepViewStatus};

    let _clipboard_guard = test_clipboard_assertion_lock()
        .lock()
        .expect("test clipboard assertion mutex poisoned");
    let _ = take_test_clipboard_capture();

    let home_dir = test_temp_path("background-jobs-copy-home");
    let cwd = test_temp_path("background-jobs-copy-workspace");
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
    let detail = BackgroundTaskView {
        task_id: "workflow-copy".to_string(),
        session_id: "s1".to_string(),
        kind: BackgroundTaskViewKind::Workflow,
        status: BackgroundTaskViewStatus::Completed,
        description: "Copy workflow step output".to_string(),
        cwd: "/tmp".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        started_at: None,
        finished_at: None,
        pid: None,
        exit_code: None,
        signal: None,
        error: None,
        model: Some("opus".to_string()),
        provider: Some(ProviderId::Anthropic),
        permission_mode: None,
        agent_type: None,
        child_session_id: None,
        cancellation_reason: None,
        label: None,
        log_tail: None,
        progress_events: None,
        workflow_steps: Some(vec![WorkflowStepView {
            step_key: "step.0".to_string(),
            parent_key: None,
            depth: 0,
            kind: "agent".to_string(),
            label: "task1".to_string(),
            status: WorkflowStepViewStatus::Completed,
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            output: Some("copy me\nfrom workflow step".to_string()),
            error: None,
            child_session_id: None,
        }]),
    };
    let mut overlay_state = BackgroundJobsOverlayState::new(vec![detail.clone()], "s1".to_string());
    overlay_state.set_detail(detail.clone());
    state.overlay = Some(OverlayState::BackgroundJobs(overlay_state));

    state
        .handle_overlay_key(
            &app_server,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        )
        .await
        .expect("handle y");

    assert_eq!(
        take_test_clipboard_capture().as_deref(),
        Some("copy me\nfrom workflow step")
    );
    assert_eq!(state.status_line, "Copied workflow step output (26 chars).");
    assert!(matches!(
        state.overlay,
        Some(OverlayState::BackgroundJobs(_))
    ));
}

#[tokio::test]
// See the sibling test above: the clipboard assertion lock intentionally spans
// the async setup to serialize against other clipboard tests.
#[allow(clippy::await_holding_lock)]
async fn background_jobs_child_session_y_copies_selected_workflow_step_output() {
    use chrono::Utc;
    use crossterm::event::KeyEvent;
    use orbcode_app_server::{
        BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus, ProviderId,
    };
    use orbcode_protocol::WorkflowStepViewStatus;
    use orbcode_protocol::{MessageRole, SessionRecord, TranscriptMessage, WorkflowStepView};

    let _clipboard_guard = test_clipboard_assertion_lock()
        .lock()
        .expect("test clipboard assertion mutex poisoned");
    let _ = take_test_clipboard_capture();

    let home_dir = test_temp_path("background-jobs-child-copy-home");
    let cwd = test_temp_path("background-jobs-child-copy-workspace");
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
    let detail = BackgroundTaskView {
        task_id: "workflow-copy".to_string(),
        session_id: "s1".to_string(),
        kind: BackgroundTaskViewKind::Workflow,
        status: BackgroundTaskViewStatus::Completed,
        description: "Copy workflow step output".to_string(),
        cwd: "/tmp".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        started_at: None,
        finished_at: None,
        pid: None,
        exit_code: None,
        signal: None,
        error: None,
        model: Some("opus".to_string()),
        provider: Some(ProviderId::Anthropic),
        permission_mode: None,
        agent_type: None,
        child_session_id: None,
        cancellation_reason: None,
        label: None,
        log_tail: None,
        progress_events: None,
        workflow_steps: Some(vec![WorkflowStepView {
            step_key: "step.0".to_string(),
            parent_key: None,
            depth: 0,
            kind: "agent".to_string(),
            label: "task1".to_string(),
            status: WorkflowStepViewStatus::Completed,
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            output: Some("copy me\nfrom child view".to_string()),
            error: None,
            child_session_id: Some("s1:workflow-copy:agent-a".to_string()),
        }]),
    };
    let mut overlay_state = BackgroundJobsOverlayState::new(vec![detail.clone()], "s1".to_string());
    overlay_state.set_detail(detail);
    let mut child_session = SessionRecord::new();
    child_session.session_id = "s1:workflow-copy:agent-a".to_string();
    child_session.push_message(TranscriptMessage::new(
        MessageRole::Assistant,
        "child output view",
    ));
    overlay_state.set_child_session(child_session);
    state.overlay = Some(OverlayState::BackgroundJobs(overlay_state));

    state
        .handle_overlay_key(
            &app_server,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        )
        .await
        .expect("handle child y");

    assert_eq!(
        take_test_clipboard_capture().as_deref(),
        Some("copy me\nfrom child view")
    );
    assert_eq!(state.status_line, "Copied workflow step output (23 chars).");
    assert!(matches!(
        state.overlay,
        Some(OverlayState::BackgroundJobs(_))
    ));
}
