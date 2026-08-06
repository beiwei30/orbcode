use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[test]
fn permission_picker_lines_show_source_editability_and_add_rows() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server_client::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: true,
            provider_allow_network: false,
            allow_tools: false,
            allowed_rules: Vec::new(),
            denied_rules: Vec::new(),
            ask_rules: Vec::new(),
            additional_directories: vec![PathBuf::from("/tmp/extra")],
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
        runtime_denied_rules: vec!["Bash(rm:*)".to_string()],
        configured_additional_directories: Vec::new(),
        session_additional_directories: vec![PathBuf::from("/tmp/extra")],
    };
    let mut picker = PermissionPickerState::new("/permissions", overview, Vec::new());

    let rendered = plain_text_lines(picker.cached_lines(120)).join("\n");

    assert_eq!(plain_text_line(&picker.cached_lines(120)[0]), "Permissions");
    assert!(rendered.contains("Recently denied"), "{rendered}");
    assert!(rendered.contains("⌕ Search"), "{rendered}");
    assert!(rendered.contains("Add a new rule"), "{rendered}");
    assert!(rendered.contains("Bash(cargo test:*)"), "{rendered}");
    assert!(rendered.contains("Read(src/**)"), "{rendered}");
    assert!(rendered.contains("Grep(orbcode/**)"), "{rendered}");
    assert!(
        rendered.find("Add a new rule").expect("add row")
            < rendered.find("Bash(cargo test:*)").expect("rule row"),
        "{rendered}"
    );
    assert_eq!(
        picker.cached_lines(120).len(),
        PERMISSION_PICKER_PANEL_HEIGHT
    );
    let title = &picker.cached_lines(120)[0];
    assert!(
        !title.spans[0]
            .style
            .add_modifier
            .contains(Modifier::REVERSED),
        "dialog title should not look like selected tab"
    );
    let header = &picker.cached_lines(120)[2];
    assert!(
        !header.spans[0]
            .style
            .add_modifier
            .contains(Modifier::REVERSED),
        "title should not look like selected tab"
    );
    let allow_tab = header
        .spans
        .iter()
        .find(|span| span.content.contains("Allow"))
        .expect("allow tab span");
    assert!(
        allow_tab.style.add_modifier.contains(Modifier::REVERSED),
        "selected tab should use reversed style"
    );

    picker.focus = PermissionPickerFocus::Content;
    let content_lines = picker.cached_lines(120);
    let selected_add_line = content_lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("Add a new rule"))
        })
        .expect("selected add row");
    let selected_add_label = selected_add_line
        .spans
        .iter()
        .find(|span| span.content.contains("Add a new rule"))
        .expect("selected add label");
    assert!(
        selected_add_label
            .style
            .add_modifier
            .contains(Modifier::BOLD),
        "selected item should use highlighted foreground style"
    );
    assert!(
        !selected_add_label
            .style
            .add_modifier
            .contains(Modifier::REVERSED),
        "selected item should not use reversed style"
    );
    let unselected_rule_line = content_lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("Bash(cargo test:*)"))
        })
        .expect("unselected rule row");
    let unselected_rule_label = unselected_rule_line
        .spans
        .iter()
        .find(|span| span.content.contains("Bash(cargo test:*)"))
        .expect("unselected rule label");
    assert_eq!(
        unselected_rule_label.style,
        inactive_style(),
        "unselected rule text should use normal foreground, not dimmed"
    );

    picker.set_tab(PermissionPickerTab::Deny);
    let deny_rendered = plain_text_lines(picker.cached_lines(120)).join("\n");
    assert!(deny_rendered.contains("Bash(rm:*)"), "{deny_rendered}");
    assert!(
        deny_rendered.contains("Orb Code will always reject requests to use denied tools."),
        "{deny_rendered}"
    );
    assert!(deny_rendered.contains("Add a new rule"), "{deny_rendered}");

    let rule = picker
        .items
        .iter()
        .find_map(|item| match item {
            PermissionPickerItem::Rule(rule) if rule.rule == "Bash(rm:*)" => Some(rule.clone()),
            _ => None,
        })
        .expect("deny rule");
    picker.rule_details = Some(PermissionRuleDetailsState { rule, selected: 1 });
    let details = plain_text_lines(picker.cached_lines(120)).join("\n");
    assert!(details.contains("Delete denied tool?"), "{details}");
    assert!(details.contains("From Session"), "{details}");
    assert!(details.contains("Are you sure"), "{details}");
}

#[test]
fn permission_picker_main_tabs_keep_fixed_height() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server_client::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: true,
            provider_allow_network: true,
            allow_tools: false,
            allowed_rules: Vec::new(),
            denied_rules: Vec::new(),
            ask_rules: Vec::new(),
            additional_directories: Vec::new(),
        },
        allow_all: false,
        effective_rules: Default::default(),
        settings_allowed_rules: vec!["Read(src/**)".to_string()],
        settings_denied_rules: vec!["Bash(rm:*)".to_string()],
        startup_allowed_rules: Vec::new(),
        startup_denied_rules: Vec::new(),
        edited_allowed_rules: Vec::new(),
        edited_denied_rules: Vec::new(),
        runtime_allowed_rules: Vec::new(),
        runtime_denied_rules: Vec::new(),
        configured_additional_directories: Vec::new(),
        session_additional_directories: Vec::new(),
    };
    let mut picker = PermissionPickerState::new("/permissions", overview, Vec::new());

    for tab in PERMISSION_PICKER_TABS {
        picker.set_tab(tab);
        assert_eq!(
            picker.cached_lines(120).len(),
            PERMISSION_PICKER_PANEL_HEIGHT,
            "{tab:?} should keep picker height stable"
        );
    }
}

#[test]
fn permission_picker_empty_state_keeps_add_rows_visible_first() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server_client::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: true,
            provider_allow_network: true,
            allow_tools: false,
            allowed_rules: Vec::new(),
            denied_rules: Vec::new(),
            ask_rules: Vec::new(),
            additional_directories: Vec::new(),
        },
        allow_all: false,
        effective_rules: Default::default(),
        settings_allowed_rules: Vec::new(),
        settings_denied_rules: Vec::new(),
        startup_allowed_rules: Vec::new(),
        startup_denied_rules: Vec::new(),
        edited_allowed_rules: Vec::new(),
        edited_denied_rules: Vec::new(),
        runtime_allowed_rules: Vec::new(),
        runtime_denied_rules: Vec::new(),
        configured_additional_directories: Vec::new(),
        session_additional_directories: Vec::new(),
    };
    let mut picker = PermissionPickerState::new("/permissions", overview, Vec::new());

    let rendered = plain_text_lines(picker.cached_lines(120));
    let top = rendered
        .iter()
        .take(12)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert!(top.contains("Allow"), "{top}");
    assert!(top.contains("Add a new rule"), "{top}");

    picker.start_add(
        PermissionRuleScope::Settings,
        PermissionRuleSettingKind::Allow,
    );
    let adding = plain_text_lines(picker.cached_lines(120));
    let adding_top = adding
        .iter()
        .take(9)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert!(adding_top.contains("Add permission rule"), "{adding_top}");
    assert!(adding_top.contains("Enter permission rule"), "{adding_top}");
    assert_eq!(permission_picker_add_input_line_index(&picker), 7);
}

#[test]
fn permission_picker_recently_denied_includes_denied_requests() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server_client::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: true,
            provider_allow_network: true,
            allow_tools: false,
            allowed_rules: Vec::new(),
            denied_rules: Vec::new(),
            ask_rules: Vec::new(),
            additional_directories: Vec::new(),
        },
        allow_all: false,
        effective_rules: Default::default(),
        settings_allowed_rules: Vec::new(),
        settings_denied_rules: Vec::new(),
        startup_allowed_rules: Vec::new(),
        startup_denied_rules: Vec::new(),
        edited_allowed_rules: Vec::new(),
        edited_denied_rules: Vec::new(),
        runtime_allowed_rules: Vec::new(),
        runtime_denied_rules: Vec::new(),
        configured_additional_directories: Vec::new(),
        session_additional_directories: Vec::new(),
    };
    let mut state = normal_state("", 0);
    state.record_recent_denied_permission(PermissionRequest {
        request_id: "req-denied".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-denied".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({
            "command": "find orbcode -name \"*.rs\" -type f",
            "description": "Count Rust files"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    state.open_permission_picker("/permissions", overview);
    let picker = match state.overlay.as_mut() {
        Some(OverlayState::PermissionPicker(picker)) => picker,
        _ => panic!("expected permission picker"),
    };
    picker.set_tab(PermissionPickerTab::RecentlyDenied);
    let rendered = plain_text_lines(picker.cached_lines(120)).join("\n");

    assert!(rendered.contains("find orbcode"), "{rendered}");
    assert!(rendered.contains("Bash"), "{rendered}");
    assert!(!rendered.contains("No recently denied"), "{rendered}");
}

#[test]
fn permission_picker_desired_height_includes_picker_lines() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server_client::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: true,
            provider_allow_network: true,
            allow_tools: false,
            allowed_rules: Vec::new(),
            denied_rules: Vec::new(),
            ask_rules: Vec::new(),
            additional_directories: Vec::new(),
        },
        allow_all: false,
        effective_rules: Default::default(),
        settings_allowed_rules: Vec::new(),
        settings_denied_rules: Vec::new(),
        startup_allowed_rules: Vec::new(),
        startup_denied_rules: Vec::new(),
        edited_allowed_rules: Vec::new(),
        edited_denied_rules: Vec::new(),
        runtime_allowed_rules: Vec::new(),
        runtime_denied_rules: Vec::new(),
        configured_additional_directories: Vec::new(),
        session_additional_directories: Vec::new(),
    };
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionPicker(PermissionPickerState::new(
        "/permissions",
        overview,
        Vec::new(),
    )));

    let panel_height = match state.overlay.as_mut() {
        Some(OverlayState::PermissionPicker(picker)) => picker
            .cached_lines(permission_picker_dialog_inner_width(120))
            .len(),
        _ => 0,
    };
    let outer_height = permission_picker_outer_height(panel_height);

    assert!(panel_height >= 10);
    assert_eq!(state.request_status_height_for_layout(120), outer_height);
    assert!(
        state.desired_viewport_height(120, 100) >= outer_height.saturating_add(4),
        "picker panel should be part of desired viewport height"
    );
}

#[tokio::test]
async fn permissions_slash_command_opens_interactive_picker_asynchronously() {
    let home_dir = test_temp_path("permissions-home");
    let cwd = test_temp_path("permissions-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(false),
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
        .handle_command(&app_server, "/permissions", &local_command_tx)
        .await
        .expect("permissions command starts");

    assert_eq!(state.status_line, "Loading permissions...");
    assert!(state.messages.is_empty());

    let event = tokio::time::timeout(Duration::from_secs(10), local_command_rx.recv())
        .await
        .expect("permissions event should arrive")
        .expect("permissions event");
    state.apply_local_command_event(event.event);

    assert!(state.messages.is_empty());
    assert!(matches!(
        state.overlay,
        Some(OverlayState::PermissionPicker(_))
    ));
    assert_eq!(
        state.status_line,
        "Permissions: ←/→ tabs, ↓ select, Esc close."
    );
}

#[tokio::test]
async fn permissions_picker_removes_selected_session_rule() {
    let home_dir = test_temp_path("permissions-picker-remove-home");
    let cwd = test_temp_path("permissions-picker-remove-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(false),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    app_server
        .app_server()
        .unwrap()
        .add_session_permission_rule(
            &state.session_id,
            orbcode_config::PermissionRuleSettingKind::Deny,
            "Bash(rm:*)",
        )
        .await
        .expect("add session deny");
    state.open_permission_picker(
        "/permissions",
        app_server.app_server().unwrap().permission_overview().await,
    );
    if let Some(OverlayState::PermissionPicker(picker)) = state.overlay.as_mut() {
        picker.set_tab(PermissionPickerTab::Deny);
        picker.focus = PermissionPickerFocus::Content;
        picker.selected = picker
            .items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    PermissionPickerItem::Rule(rule)
                        if rule.kind == PermissionRuleSettingKind::Deny
                            && rule.rule == "Bash(rm:*)"
                            && rule.scope == Some(PermissionRuleScope::Session)
                )
            })
            .expect("session deny item");
    }

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
        .expect("open rule details");
    state
        .handle_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &mut turn_events,
            &local_command_tx,
        )
        .await
        .expect("select yes");
    state
        .handle_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut turn_events,
            &local_command_tx,
        )
        .await
        .expect("remove selected rule");

    assert_eq!(state.status_line, "Removed session deny permission rule.");
    assert!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_denied(
                "bash",
                &serde_json::json!({ "command": "rm -rf /tmp/example" }).to_string()
            )
            .is_none()
    );
    assert!(matches!(
        state.overlay,
        Some(OverlayState::PermissionPicker(_))
    ));
}

#[tokio::test]
async fn permissions_picker_adds_settings_rule_from_inline_draft() {
    let home_dir = test_temp_path("permissions-picker-add-home");
    let cwd = test_temp_path("permissions-picker-add-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir.clone()),
            allow_tools: Some(false),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    state.open_permission_picker(
        "/permissions",
        app_server.app_server().unwrap().permission_overview().await,
    );
    if let Some(OverlayState::PermissionPicker(picker)) = state.overlay.as_mut() {
        picker.set_tab(PermissionPickerTab::Deny);
        picker.focus = PermissionPickerFocus::Content;
        picker.selected = picker
            .items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    PermissionPickerItem::AddRule(PermissionRuleSettingKind::Deny)
                )
            })
            .expect("settings deny add item");
    }

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
        .expect("start add");
    assert!(matches!(
        state.overlay,
        Some(OverlayState::PermissionPicker(PermissionPickerState {
            adding: Some(_),
            ..
        }))
    ));

    for ch in "Bash(rm:*)".chars() {
        state
            .handle_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &mut turn_events,
                &local_command_tx,
            )
            .await
            .expect("type rule");
    }
    state
        .handle_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut turn_events,
            &local_command_tx,
        )
        .await
        .expect("finish rule input");
    assert!(matches!(
        state.overlay,
        Some(OverlayState::PermissionPicker(PermissionPickerState {
            add_destination: Some(_),
            ..
        }))
    ));
    state
        .handle_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut turn_events,
            &local_command_tx,
        )
        .await
        .expect("save rule");

    assert_eq!(state.status_line, "Added deny permission rule.");
    let settings = tokio::fs::read_to_string(home_dir.join("settings.json"))
        .await
        .expect("read settings");
    let value: Value = serde_json::from_str(&settings).expect("settings json");
    assert_eq!(
        value["permissions"]["deny"],
        serde_json::json!(["Bash(rm:*)"])
    );
    let Some(OverlayState::PermissionPicker(picker)) = state.overlay.as_ref() else {
        panic!("permission picker should stay open");
    };
    assert!(picker.adding.is_none());
    assert_eq!(picker.tab, PermissionPickerTab::Deny);
    assert!(matches!(
        picker.selected_item(),
        Some(PermissionPickerItem::Rule(rule))
            if rule.kind == PermissionRuleSettingKind::Deny
                && rule.rule == "Bash(rm:*)"
                && rule.scope == Some(PermissionRuleScope::Settings)
    ));
    assert!(
        picker.items.iter().any(|item| {
            matches!(
                item,
                PermissionPickerItem::Rule(rule)
                    if rule.kind == PermissionRuleSettingKind::Deny
                        && rule.rule == "Bash(rm:*)"
                        && rule.scope == Some(PermissionRuleScope::Settings)
            )
        }),
        "picker should refresh with new settings rule"
    );
}

#[tokio::test]
async fn permissions_slash_command_adds_and_removes_settings_rules() {
    let home_dir = test_temp_path("permissions-edit-home");
    let cwd = test_temp_path("permissions-edit-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir.clone()),
            allow_tools: Some(false),
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
        .handle_command(
            &app_server,
            "/permissions add settings deny Bash(rm:*)",
            &local_command_tx,
        )
        .await
        .expect("add deny rule");

    assert_eq!(state.status_line, "Added deny permission rule.");
    assert!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_denied(
                "bash",
                &serde_json::json!({ "command": "rm -rf /tmp/example" }).to_string()
            )
            .is_some()
    );

    let settings = tokio::fs::read_to_string(home_dir.join("settings.json"))
        .await
        .expect("read settings");
    let value: Value = serde_json::from_str(&settings).expect("settings json");
    assert_eq!(
        value["permissions"]["deny"],
        serde_json::json!(["Bash(rm:*)"])
    );

    state
        .handle_command(
            &app_server,
            "/permissions remove settings deny Bash(rm:*)",
            &local_command_tx,
        )
        .await
        .expect("remove deny rule");

    assert_eq!(state.status_line, "Removed deny permission rule.");
    assert!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_denied(
                "bash",
                &serde_json::json!({ "command": "rm -rf /tmp/example" }).to_string()
            )
            .is_none()
    );
}

#[tokio::test]
async fn permissions_slash_command_rejects_invalid_rules_with_context() {
    let home_dir = test_temp_path("permissions-invalid-rule-home");
    let cwd = test_temp_path("permissions-invalid-rule-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir.clone()),
            allow_tools: Some(false),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let error = state
        .handle_command(
            &app_server,
            "/permissions add allow Bash(",
            &local_command_tx,
        )
        .await
        .expect_err("invalid rule should fail");
    let message = error.to_string();

    assert!(
        message.contains("invalid permission rule `Bash(`"),
        "{message}"
    );
    assert!(
        message.contains("opening `(` without a closing `)`"),
        "{message}"
    );
    assert!(message.contains(PERMISSIONS_USAGE), "{message}");
    assert!(
        !tokio::fs::try_exists(home_dir.join("settings.json"))
            .await
            .expect("stat settings")
    );
}

#[tokio::test]
async fn permissions_slash_command_explains_uneditable_sources() {
    let home_dir = test_temp_path("permissions-source-edit-home");
    let cwd = test_temp_path("permissions-source-edit-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(false),
            disallowed_tools: vec!["Bash(rm:*)".to_string()],
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
        .handle_command(
            &app_server,
            "/permissions remove deny Bash(rm:*)",
            &local_command_tx,
        )
        .await
        .expect("remove settings rule should report no-op");

    assert_eq!(
        state.status_line,
        "No settings deny permission rule matched."
    );
    assert!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_denied(
                "bash",
                &serde_json::json!({ "command": "rm -rf /tmp/example" }).to_string()
            )
            .is_some()
    );
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(
        transcript.contains("matching active source(s): env/CLI"),
        "{transcript}"
    );
    let normalized_transcript = transcript.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized_transcript
            .contains("Env/CLI and project-local or managed settings are read-only here"),
        "{transcript}"
    );
}

#[tokio::test]
async fn permissions_slash_command_adds_and_removes_session_rules() {
    let home_dir = test_temp_path("permissions-session-edit-home");
    let cwd = test_temp_path("permissions-session-edit-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir.clone()),
            allow_tools: Some(false),
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
        .handle_command(
            &app_server,
            "/permissions add session allow Read(notes/**)",
            &local_command_tx,
        )
        .await
        .expect("add session allow rule");

    assert_eq!(state.status_line, "Added session allow permission rule.");
    assert!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_allowed_without_prompt("Read", r#"{"file_path":"notes/hello.txt"}"#)
    );
    assert!(
        !tokio::fs::try_exists(home_dir.join("settings.json"))
            .await
            .expect("stat settings")
    );

    state
        .handle_command(
            &app_server,
            "/permissions remove session allow Read(notes/**)",
            &local_command_tx,
        )
        .await
        .expect("remove session allow rule");

    assert_eq!(state.status_line, "Removed session allow permission rule.");
    assert!(
        !app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_allowed_without_prompt("Read", r#"{"file_path":"notes/hello.txt"}"#)
    );

    state
        .handle_command(
            &app_server,
            "/permissions add session deny Bash(rm:*)",
            &local_command_tx,
        )
        .await
        .expect("add session deny rule");

    assert_eq!(state.status_line, "Added session deny permission rule.");
    assert!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_denied(
                "bash",
                &serde_json::json!({ "command": "rm -rf /tmp/example" }).to_string()
            )
            .is_some()
    );

    state
        .handle_command(
            &app_server,
            "/permissions remove session deny Bash(rm:*)",
            &local_command_tx,
        )
        .await
        .expect("remove session deny rule");

    assert_eq!(state.status_line, "Removed session deny permission rule.");
    assert!(
        app_server
            .app_server()
            .unwrap()
            .permissions()
            .tool_denied(
                "bash",
                &serde_json::json!({ "command": "rm -rf /tmp/example" }).to_string()
            )
            .is_none()
    );
}

#[test]
fn permission_panel_renders_mcp_tool_preview_and_rule() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "call-mcp-tool".to_string(),
        tool_input: serde_json::json!({
            "server_id": "docs",
            "tool_name": "inspect",
            "input": { "mode": "brief" }
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Call MCP tool"));
    assert!(rendered.iter().any(|line| line.contains("Server docs")));
    assert!(rendered.iter().any(|line| line.contains("Tool inspect")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Permission target:"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("mcp__docs__inspect"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to call this MCP tool?"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| { line.contains("Yes, and don't ask again for: MCP docs::inspect") })
    );
}
