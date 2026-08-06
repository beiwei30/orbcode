use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[tokio::test]
async fn sandbox_exclude_slash_command_updates_local_settings() {
    let home_dir = test_temp_path("sandbox-command-home");
    let cwd = test_temp_path("sandbox-command-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(
            &app_server,
            "/sandbox exclude \"npm run test:*\"",
            &local_command_tx,
        )
        .await
        .expect("sandbox exclude command succeeds");

    let settings_path = cwd.join(".claude/settings.local.json");
    let contents = tokio::fs::read_to_string(&settings_path)
        .await
        .expect("read local settings");
    let value: serde_json::Value = serde_json::from_str(&contents).expect("settings json");
    assert_eq!(value["sandbox"]["excludedCommands"][0], "npm run test:*");
    assert_eq!(
        state.status_line,
        "Added \"npm run test:*\" to excluded commands in ./.claude/settings.local.json"
    );
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(
        transcript.contains("❯ /sandbox exclude \"npm run test:*\""),
        "{transcript}"
    );
    assert!(
        !transcript.contains("Added \"npm run test:*\" to excluded commands"),
        "{transcript}"
    );
}

#[tokio::test]
async fn sandbox_exclude_slash_command_rejects_missing_pattern() {
    let home_dir = test_temp_path("sandbox-missing-pattern-home");
    let cwd = test_temp_path("sandbox-missing-pattern-workspace");
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

    let error = state
        .handle_command(&app_server, "/sandbox exclude", &local_command_tx)
        .await
        .expect_err("missing pattern");

    assert!(
        error
            .to_string()
            .contains("please provide a command pattern to exclude"),
        "{error}"
    );
}

#[tokio::test]
async fn sandbox_slash_command_opens_mode_picker() {
    let home_dir = test_temp_path("sandbox-picker-home");
    let cwd = test_temp_path("sandbox-picker-workspace");
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
        .handle_command(&app_server, "/sandbox", &local_command_tx)
        .await
        .expect("sandbox command succeeds");

    assert!(matches!(
        state.overlay,
        Some(OverlayState::SandboxPicker(_))
    ));
    assert_eq!(
        state.status_line,
        "Sandbox: ←/→ tabs, Enter select, Esc quit."
    );
    let Some(OverlayState::SandboxPicker(picker)) = state.overlay.as_ref() else {
        panic!("expected sandbox picker");
    };
    let rendered = plain_text_lines(&sandbox_picker_lines(picker, 100)).join("\n");
    assert!(
        rendered.contains("Sandbox:  Mode   Overrides   Config"),
        "{rendered}"
    );
    assert!(rendered.contains("Configure Mode:"), "{rendered}");
    assert!(rendered.contains("No Sandbox (current)"), "{rendered}");
    assert!(
        rendered.contains("Auto-allow mode: Commands will try to run"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  ←/→ tabs · Enter to select · Esc to quit"),
        "{rendered}"
    );
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        )
        .await
        .expect("quit sandbox picker");
    assert!(state.overlay.is_none());
    assert_eq!(state.status_line, "Closed sandbox settings.");
}

#[tokio::test]
async fn sandbox_config_tab_shows_local_diagnostics() {
    let home_dir = test_temp_path("sandbox-config-home");
    let cwd = test_temp_path("sandbox-config-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(cwd.join(".claude"))
        .await
        .expect("create settings dir");
    tokio::fs::write(
        cwd.join(".claude/settings.local.json"),
        r#"{
              "sandbox": {
                "enabled": true,
                "autoAllowBashIfSandboxed": false,
                "allowUnsandboxedCommands": false,
                "excludedCommands": ["npm run test:*"],
                "filesystem": {
                  "allowWrite": ["./tmp"],
                  "denyWrite": ["./secrets"],
                  "denyRead": ["./private"],
                  "allowRead": ["./private/public.md"]
                },
                "network": {
                  "allowedDomains": ["example.com"],
                  "allowUnixSockets": ["/tmp/service.sock"],
                  "allowAllUnixSockets": false,
                  "allowLocalBinding": true,
                  "httpProxyPort": 8080,
                  "socksProxyPort": 1080
                }
              }
            }"#,
    )
    .await
    .expect("write local settings");

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
        .handle_command(&app_server, "/sandbox", &local_command_tx)
        .await
        .expect("sandbox command succeeds");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        )
        .await
        .expect("switch to overrides");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        )
        .await
        .expect("switch to config");

    let Some(OverlayState::SandboxPicker(picker)) = state.overlay.as_ref() else {
        panic!("expected sandbox picker");
    };
    let rendered = plain_text_lines(&sandbox_picker_lines(picker, 120)).join("\n");
    assert!(rendered.contains("Config:"), "{rendered}");
    assert!(rendered.contains("Mode: Sandbox BashTool, with regular permissions"));
    assert!(rendered.contains("Unsandboxed fallback: strict"));
    assert!(rendered.contains("Excluded Commands: npm run test:*"));
    assert!(rendered.contains("Filesystem:"), "{rendered}");
    assert!(rendered.contains("Allow write: ./tmp"), "{rendered}");
    assert!(rendered.contains("Deny write: ./secrets"), "{rendered}");
    assert!(rendered.contains("Deny read: ./private"), "{rendered}");
    assert!(
        rendered.contains("Allow read: ./private/public.md"),
        "{rendered}"
    );
    assert!(rendered.contains("Network:"), "{rendered}");
    assert!(
        rendered.contains("Allowed domains: example.com"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Allowed Unix sockets: /tmp/service.sock"),
        "{rendered}"
    );
    assert!(rendered.contains("Allow all Unix sockets: disabled"));
    assert!(rendered.contains("Allow local binding: enabled"));
    assert!(rendered.contains("HTTP proxy port: 8080"));
    assert!(rendered.contains("SOCKS proxy port: 1080"));
}

#[tokio::test]
async fn sandbox_slash_command_enter_opens_mode_picker_without_forcing_args() {
    let home_dir = test_temp_path("sandbox-enter-picker-home");
    let cwd = test_temp_path("sandbox-enter-picker-workspace");
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
    state.input = "/sandbox".to_string();
    state.input_cursor = state.input.len();
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
        .expect("submit sandbox command");

    assert!(matches!(
        state.overlay,
        Some(OverlayState::SandboxPicker(_))
    ));
    assert!(state.input.is_empty());
    assert_eq!(
        state.status_line,
        "Sandbox: ←/→ tabs, Enter select, Esc quit."
    );
}

#[tokio::test]
async fn sandbox_picker_can_set_auto_allow_mode() {
    let home_dir = test_temp_path("sandbox-picker-auto-home");
    let cwd = test_temp_path("sandbox-picker-auto-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/sandbox", &local_command_tx)
        .await
        .expect("sandbox command succeeds");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
        )
        .await
        .expect("select first mode");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        )
        .await
        .expect("select auto allow with space");

    let settings: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(cwd.join(".claude/settings.local.json"))
            .await
            .expect("read local settings"),
    )
    .expect("settings json");
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["autoAllowBashIfSandboxed"], true);
    assert_eq!(
        state.status_line,
        "✓ Sandbox enabled with auto-allow for bash commands"
    );
    assert!(matches!(
        state.overlay,
        Some(OverlayState::SandboxPicker(_))
    ));
}

#[tokio::test]
async fn sandbox_picker_can_set_strict_override() {
    let home_dir = test_temp_path("sandbox-picker-override-home");
    let cwd = test_temp_path("sandbox-picker-override-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(cwd.join(".claude"))
        .await
        .expect("create local settings dir");
    tokio::fs::write(
            cwd.join(".claude/settings.local.json"),
            r#"{"sandbox":{"enabled":true,"autoAllowBashIfSandboxed":true,"allowUnsandboxedCommands":true}}"#,
        )
        .await
        .expect("write local settings");

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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/sandbox", &local_command_tx)
        .await
        .expect("sandbox command succeeds");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        )
        .await
        .expect("switch to overrides");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        )
        .await
        .expect("select strict");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("apply strict override");

    let settings: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(cwd.join(".claude/settings.local.json"))
            .await
            .expect("read local settings"),
    )
    .expect("settings json");
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
    assert_eq!(
        state.status_line,
        "✓ Strict sandbox mode - all commands must run in sandbox or be excluded via the `excludedCommands` option"
    );
    assert!(matches!(
        state.overlay,
        Some(OverlayState::SandboxPicker(_))
    ));
}

#[tokio::test]
async fn config_slash_command_opens_terminal_picker() {
    let home_dir = test_temp_path("config-command-home");
    let cwd = test_temp_path("config-command-workspace");
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
        .handle_command(&app_server, "/config", &local_command_tx)
        .await
        .expect("config command succeeds");

    assert!(matches!(state.overlay, Some(OverlayState::ConfigPicker(_))));
    assert_eq!(
        state.status_line,
        "Config: type to search, Space change, Enter save, Esc cancel."
    );
    let Some(OverlayState::ConfigPicker(picker)) = state.overlay.as_ref() else {
        panic!("expected config picker");
    };
    let rendered = plain_text_lines(&config_picker_lines(picker, 100)).join("\n");
    assert!(rendered.contains("Search settings"), "{rendered}");
    assert!(rendered.contains("Auto-compact"), "{rendered}");
    assert!(rendered.contains("Type to search"), "{rendered}");
    assert!(rendered.contains("more below"), "{rendered}");
}

#[tokio::test]
async fn config_picker_can_update_effort() {
    let home_dir = test_temp_path("config-effort-home");
    let cwd = test_temp_path("config-effort-workspace");
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
        .handle_command(&app_server, "/config", &local_command_tx)
        .await
        .expect("config command succeeds");
    for character in "effort".chars() {
        state
            .handle_overlay_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            )
            .await
            .expect("type config search");
    }
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        )
        .await
        .expect("select config option");

    assert_eq!(
        app_server.app_server().unwrap().effort_level(),
        Some(EffortLevel::Low)
    );
    assert!(matches!(state.overlay, Some(OverlayState::ConfigPicker(_))));
    assert_eq!(
        state.status_line,
        "Set effort level to low: Quick, straightforward implementation with minimal overhead"
    );
    let transcript = plain_text_lines(&state.transcript_lines(90)).join("\n");
    assert!(transcript.contains("❯ /config"), "{transcript}");
    assert!(
        !transcript.contains("└  Set effort level to low: Quick"),
        "{transcript}"
    );
}

#[tokio::test]
async fn config_picker_can_toggle_editor_mode() {
    let home_dir = test_temp_path("config-editor-mode-home");
    let cwd = test_temp_path("config-editor-mode-workspace");
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
        .handle_command(&app_server, "/config", &local_command_tx)
        .await
        .expect("config command succeeds");
    for character in "editor".chars() {
        state
            .handle_overlay_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            )
            .await
            .expect("type config search");
    }
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        )
        .await
        .expect("toggle editor mode");

    assert_eq!(state.editor_mode, EditorMode::Insert);
    assert_eq!(
        app_server.app_server().unwrap().editor_mode_setting(),
        orbcode_config::EditorModeSetting::Vim
    );
    assert!(matches!(state.overlay, Some(OverlayState::ConfigPicker(_))));
    let Some(OverlayState::ConfigPicker(picker)) = state.overlay.as_ref() else {
        panic!("expected config picker");
    };
    let rendered = plain_text_lines(&config_picker_lines(picker, 100)).join("\n");
    assert!(rendered.contains("Editor mode"), "{rendered}");
    assert!(rendered.contains("vim ✔"), "{rendered}");
}

#[tokio::test]
async fn config_picker_can_open_output_style_picker() {
    let home_dir = test_temp_path("config-output-style-home");
    let cwd = test_temp_path("config-output-style-workspace");
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
        .handle_command(&app_server, "/config", &local_command_tx)
        .await
        .expect("config command succeeds");
    for character in "response".chars() {
        state
            .handle_overlay_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            )
            .await
            .expect("type config search");
    }
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("select output style config");

    assert!(matches!(
        state.overlay,
        Some(OverlayState::OutputStylePicker(_))
    ));
    assert_eq!(
        state.status_line,
        "Output style: Enter to select, Esc to cancel."
    );
    let Some(OverlayState::OutputStylePicker(picker)) = state.overlay.as_ref() else {
        panic!("expected output style picker");
    };
    let rendered = plain_text_lines(&output_style_picker_lines(picker, 100)).join("\n");
    assert!(rendered.contains("Preferred output style"), "{rendered}");
    assert!(rendered.contains("Default ✔"), "{rendered}");
    assert!(rendered.contains("Explanatory"), "{rendered}");
    assert!(rendered.contains("Learning"), "{rendered}");
}

#[tokio::test]
async fn output_style_picker_persists_learning_style() {
    let home_dir = test_temp_path("output-style-learning-home");
    let cwd = test_temp_path("output-style-learning-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    state.overlay = Some(OverlayState::OutputStylePicker(
        OutputStylePickerState::new(
            "/config",
            app_server
                .app_server()
                .unwrap()
                .output_style_options()
                .await
                .unwrap(),
            false,
        ),
    ));

    for _ in 0..2 {
        state
            .handle_overlay_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            )
            .await
            .expect("move output style selection");
    }
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("select output style");

    let settings: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(cwd.join(".claude/settings.local.json"))
            .await
            .expect("read local settings"),
    )
    .expect("settings json");
    assert_eq!(settings["outputStyle"], "Learning");
    assert_eq!(state.status_line, "Set output style to Learning");
    assert_eq!(
        app_server.app_server().unwrap().active_output_style_name(),
        "Learning"
    );
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(transcript.contains("❯ /config"), "{transcript}");
    assert!(
        !transcript.contains("└  Set output style to Learning"),
        "{transcript}"
    );
}

#[tokio::test]
async fn config_picker_can_open_model_picker() {
    let home_dir = test_temp_path("config-model-home");
    let cwd = test_temp_path("config-model-workspace");
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
        .handle_command(&app_server, "/config", &local_command_tx)
        .await
        .expect("config command succeeds");
    for character in "model".chars() {
        state
            .handle_overlay_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            )
            .await
            .expect("type config search");
    }
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("select model config");

    assert!(matches!(state.overlay, Some(OverlayState::ModelPicker(_))));
    assert_eq!(
        state.status_line,
        "Select model: Enter confirm, ←/→ effort, Esc cancel."
    );
}

#[tokio::test]
async fn theme_slash_command_opens_picker_and_persists_selection() {
    let home_dir = test_temp_path("theme-command-home");
    let cwd = test_temp_path("theme-command-workspace");
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
        .handle_command(&app_server, "/theme", &local_command_tx)
        .await
        .expect("theme command opens picker");
    assert!(matches!(state.overlay, Some(OverlayState::ThemePicker(_))));

    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        )
        .await
        .expect("move theme selection");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("select theme");

    assert!(state.overlay.is_none());
    assert_eq!(
        app_server.app_server().unwrap().theme_setting(),
        orbcode_config::ThemeSetting::Dark
    );
    assert_eq!(state.status_line, "Theme set to dark.");
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(transcript.contains("❯ /theme"), "{transcript}");
    assert!(!transcript.contains("└  Theme set to dark"), "{transcript}");
    let settings: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(home_dir.join("settings.json"))
            .await
            .expect("read settings"),
    )
    .expect("settings json");
    assert_eq!(settings["theme"], "dark");
}

#[tokio::test]
async fn output_style_slash_command_sets_style_directly_when_arg_given() {
    let home_dir = test_temp_path("output-style-direct-home");
    let cwd = test_temp_path("output-style-direct-workspace");
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
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/output-style Explanatory", &local_command_tx)
        .await
        .expect("set output style directly");

    assert!(state.overlay.is_none());
    assert_eq!(
        app_server.app_server().unwrap().active_output_style_name(),
        "Explanatory"
    );
    let settings: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(cwd.join(".claude").join("settings.local.json"))
            .await
            .expect("read settings"),
    )
    .expect("settings json");
    assert_eq!(settings["outputStyle"], "Explanatory");
}

#[tokio::test]
async fn output_style_slash_command_opens_picker_when_no_args() {
    let home_dir = test_temp_path("output-style-picker-home");
    let cwd = test_temp_path("output-style-picker-workspace");
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
        .handle_command(&app_server, "/output-style", &local_command_tx)
        .await
        .expect("open output style picker");

    assert!(matches!(
        state.overlay,
        Some(OverlayState::OutputStylePicker(_))
    ));
}

#[tokio::test]
async fn config_picker_can_open_theme_picker() {
    let home_dir = test_temp_path("config-theme-home");
    let cwd = test_temp_path("config-theme-workspace");
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
        .handle_command(&app_server, "/config", &local_command_tx)
        .await
        .expect("config command succeeds");
    for character in "theme".chars() {
        state
            .handle_overlay_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            )
            .await
            .expect("type config search");
    }
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("select theme config");

    assert!(matches!(state.overlay, Some(OverlayState::ThemePicker(_))));
    assert_eq!(state.status_line, "Theme: Enter to select, Esc to cancel.");
}

#[tokio::test]
async fn model_picker_selection_pushes_slash_feedback() {
    let home_dir = test_temp_path("model-picker-home");
    let cwd = test_temp_path("model-picker-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(
            home_dir.join("settings.json"),
            r#"{"env":{"ANTHROPIC_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_HAIKU_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_SONNET_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_OPUS_MODEL":"glm-4.7"}}"#,
        )
        .await
        .expect("write settings");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir.clone()),
            env_overrides: orbcode_app_server::sealed_provider_env_overrides(),
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
        .handle_command(&app_server, "/model", &local_command_tx)
        .await
        .expect("model command opens picker");
    assert!(matches!(state.overlay, Some(OverlayState::ModelPicker(_))));

    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("select model");

    assert!(state.overlay.is_none());
    assert_eq!(state.status_line, "Set model to glm-4.7.");
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(transcript.contains("❯ /model"), "{transcript}");
    assert!(
        !transcript.contains("└  Set model to glm-4.7"),
        "{transcript}"
    );
    let settings: serde_json::Value = serde_json::from_str(
        &tokio::fs::read_to_string(home_dir.join("settings.json"))
            .await
            .expect("read settings"),
    )
    .expect("settings json");
    assert!(settings.get("model").is_none());
    assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "glm-4.7");

    let controls = app_server
        .session_control_state(&state.session_id)
        .await
        .expect("session model state");
    assert_eq!(
        controls.model_selection.runtime_override,
        orbcode_app_server_client::RuntimeModelOverride::Model("glm-4.7".to_string())
    );
    assert_eq!(controls.model_selection.resolution.request_model, "glm-4.7");
}

#[tokio::test]
async fn model_picker_can_adjust_effort_with_arrow_keys() {
    let home_dir = test_temp_path("model-picker-effort-home");
    let cwd = test_temp_path("model-picker-effort-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(
            home_dir.join("settings.json"),
            r#"{"env":{"ANTHROPIC_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_HAIKU_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_SONNET_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_OPUS_MODEL":"glm-4.7"}}"#,
        )
        .await
        .expect("write settings");

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
        .handle_command(&app_server, "/model", &local_command_tx)
        .await
        .expect("model command opens picker");
    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        )
        .await
        .expect("adjust effort");
    let Some(OverlayState::ModelPicker(picker)) = state.overlay.as_ref() else {
        panic!("expected model picker");
    };
    let rendered = plain_text_lines(&model_picker_lines(picker, 100)).join("\n");
    assert!(
        rendered.contains("◉ Low effort ← → to adjust"),
        "{rendered}"
    );

    state
        .handle_overlay_key(
            &app_server,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .await
        .expect("select model");

    assert_eq!(
        app_server.app_server().unwrap().effort_level(),
        Some(EffortLevel::Low)
    );
    assert!(state.overlay.is_none());
    assert!(
        state
            .status_line
            .contains("Set effort level to low: Quick, straightforward")
    );
}
