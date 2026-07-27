use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[tokio::test]
async fn login_slash_command_shows_auth_status() {
    let home_dir = test_temp_path("login-status-home");
    let cwd = test_temp_path("login-status-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

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
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/login", &local_command_tx)
        .await
        .expect("login status command succeeds");

    assert_eq!(state.status_line, "Auth status loaded.");
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(transcript.contains("❯ /login"), "{transcript}");
    assert!(!transcript.contains("Auth status loaded."), "{transcript}");
    assert!(transcript.contains("auth store:"), "{transcript}");
    assert!(transcript.contains("auth.json"), "{transcript}");
    assert!(
        transcript.contains("auth: no stored credentials"),
        "{transcript}"
    );
}

#[tokio::test]
async fn login_slash_command_stores_env_var_auth_metadata() {
    let home_dir = test_temp_path("login-env-home");
    let cwd = test_temp_path("login-env-workspace");
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
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(
            &app_server,
            "/login anthropic --env-var ANTHROPIC_API_KEY",
            &local_command_tx,
        )
        .await
        .expect("login command succeeds");

    let overview = app_server
        .app_server()
        .unwrap()
        .auth_overview()
        .await
        .expect("auth overview");
    assert!(overview.has_provider(ProviderId::Anthropic));
    assert_eq!(
        state.status_line,
        "Stored auth metadata for anthropic via env:ANTHROPIC_API_KEY."
    );
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(
        transcript.contains("❯ /login anthropic --env-var ANTHROPIC_API_KEY"),
        "{transcript}"
    );
    assert!(
        transcript.contains("anthropic api_key env:ANTHROPIC_API_KEY (persisted)"),
        "{transcript}"
    );
    assert!(
        tokio::fs::try_exists(home_dir.join("auth.json"))
            .await
            .expect("check auth store")
    );
}

#[tokio::test]
async fn login_slash_command_rejects_direct_tokens() {
    let home_dir = test_temp_path("login-token-home");
    let cwd = test_temp_path("login-token-workspace");
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

    let error = state
        .handle_command(
            &app_server,
            "/login anthropic --token sk-ant-secret",
            &local_command_tx,
        )
        .await
        .expect_err("token login rejected");

    assert!(
        error
            .to_string()
            .contains("does not accept --token because slash commands are recorded"),
        "{error}"
    );
}

#[tokio::test]
async fn submitted_slash_command_is_immediately_browsable_from_history() {
    let home_dir = test_temp_path("slash-history-home");
    let cwd = test_temp_path("slash-history-workspace");
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
    state.input = "/help".to_string();
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
        .expect("submit help command");

    state.overlay = None;
    assert!(state.navigate_prompt_up());
    assert_eq!(state.input, "/help");
    assert_eq!(state.prompt_history_index, Some(0));
}

#[tokio::test]
async fn rejected_token_login_slash_command_is_not_added_to_history() {
    let home_dir = test_temp_path("slash-token-history-home");
    let cwd = test_temp_path("slash-token-history-workspace");
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
    state.input = "/login anthropic --token sk-ant-secret".to_string();
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
        .expect("submit rejected login command");

    assert!(state.status_line.contains("Command failed:"));
    assert!(state.prompt_history.is_empty());
}

#[tokio::test]
async fn logout_slash_command_removes_persisted_auth_metadata() {
    let home_dir = test_temp_path("logout-home");
    let cwd = test_temp_path("logout-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            env_overrides: orbcode_app_server::sealed_provider_env_overrides(),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    app_server
        .app_server()
        .unwrap()
        .auth_login(
            ProviderId::OpenAi,
            AuthMethod::ApiKey,
            None,
            Some("OPENAI_API_KEY".to_string()),
        )
        .await
        .expect("seed auth");
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/logout openai", &local_command_tx)
        .await
        .expect("logout command succeeds");

    assert_eq!(
        state.status_line,
        "Removed 1 persisted auth entry(s) for openai."
    );
    let overview = app_server
        .app_server()
        .unwrap()
        .auth_overview()
        .await
        .expect("auth overview");
    assert!(!overview.has_provider(ProviderId::OpenAi));
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(transcript.contains("❯ /logout openai"), "{transcript}");
    assert!(
        transcript.contains("auth: no stored credentials"),
        "{transcript}"
    );
}

#[tokio::test]
async fn logout_slash_command_removes_anthropic_oauth_credentials() {
    let home_dir = test_temp_path("logout-oauth-home");
    let cwd = test_temp_path("logout-oauth-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(
        home_dir.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"oauth-token","expiresAt":null,"scopes":["user:inference"]}}"#,
    )
    .await
    .expect("credentials");

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
    let bootstrap = app_server
        .bootstrap_typed(None)
        .await
        .expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/logout anthropic", &local_command_tx)
        .await
        .expect("logout command succeeds");

    assert_eq!(
        state.status_line,
        "Removed 1 persisted auth entry(s) for anthropic."
    );
    assert!(
        !tokio::fs::try_exists(home_dir.join(".credentials.json"))
            .await
            .expect("check credentials")
    );
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(transcript.contains("❯ /logout anthropic"), "{transcript}");
    assert!(
        transcript.contains("auth: no stored credentials"),
        "{transcript}"
    );
}

#[tokio::test]
async fn release_notes_slash_command_reads_cached_changelog() {
    let home_dir = test_temp_path("release-notes-home");
    let cwd = test_temp_path("release-notes-workspace");
    tokio::fs::create_dir_all(home_dir.join("cache"))
        .await
        .expect("create cache");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(
            home_dir.join("cache/changelog.md"),
            "# Changelog\n\n## 2.1.0 - 2026-05-05\n- Added one thing\n- Fixed another thing\n\n## 2.0.0\n- Older note\n",
        )
        .await
        .expect("write changelog");

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
    state.ui_version = "2.1.0".to_string();
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/release-notes", &local_command_tx)
        .await
        .expect("release notes command succeeds");

    assert_eq!(
        state.status_line,
        "Release notes loaded from the Claude Code changelog."
    );
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(transcript.contains("❯ /release-notes"), "{transcript}");
    assert!(transcript.contains("Version 2.1.0:"), "{transcript}");
    assert!(transcript.contains("· Added one thing"), "{transcript}");
    assert!(transcript.contains("Version 2.0.0:"), "{transcript}");
}

#[tokio::test]
async fn release_notes_slash_command_falls_back_to_changelog_link() {
    let home_dir = test_temp_path("release-notes-empty-home");
    let cwd = test_temp_path("release-notes-empty-workspace");
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
        .handle_command(&app_server, "/release-notes", &local_command_tx)
        .await
        .expect("release notes command succeeds");

    assert_eq!(state.status_line, "Release notes unavailable locally.");
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(
        transcript.contains("See the full changelog at:"),
        "{transcript}"
    );
    assert!(transcript.contains(CHANGELOG_URL), "{transcript}");
}
