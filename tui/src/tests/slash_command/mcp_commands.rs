use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[tokio::test]
async fn mcp_capabilities_slash_command_uses_local_output_registry() {
    let home_dir = test_temp_path("mcp-capabilities-home");
    let cwd = test_temp_path("mcp-capabilities-workspace");
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
        .handle_command(&app_server, "/mcp capabilities", &local_command_tx)
        .await
        .expect("mcp capabilities command succeeds");

    assert_eq!(state.status_line, "Listed MCP transport capabilities.");
    let output = state
        .messages
        .iter()
        .find(|message| {
            message
                .content
                .contains("Listed MCP transport capabilities.")
        })
        .expect("MCP capabilities output message")
        .content
        .as_str();
    assert!(output.contains("stdio enabled=true"), "{output}");
    assert!(output.contains("streamable_http enabled=true"), "{output}");
    assert!(output.contains("web_socket enabled=true"), "{output}");
    assert!(
        !output.lines().any(|line| {
            let line = line.trim();
            line.starts_with("http enabled=") || line.starts_with("https enabled=")
        }),
        "legacy HTTP aliases must not be listed as capabilities: {output}"
    );
}

#[tokio::test]
async fn mcp_read_slash_command_echoes_command_before_result() {
    let home_dir = test_temp_path("mcp-read-order-home");
    let cwd = test_temp_path("mcp-read-order-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    seed_fake_app_server_mcp(app_server.app_server().unwrap()).await;

    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(
            &app_server,
            "/mcp read fake mcp://fake/info",
            &local_command_tx,
        )
        .await
        .expect("mcp read command succeeds");

    let command_index = state
        .messages
        .iter()
        .position(|message| message.content.contains("/mcp read fake mcp://fake/info"))
        .expect("command output message");
    let result_index = state
        .messages
        .iter()
        .position(|message| {
            message
                .content
                .contains("MCP Resource: fake mcp://fake/info")
        })
        .expect("mcp resource result message");
    assert!(
        command_index < result_index,
        "command output should precede MCP result"
    );
}

#[tokio::test]
async fn mcp_call_slash_command_echoes_command_before_result() {
    let home_dir = test_temp_path("mcp-call-order-home");
    let cwd = test_temp_path("mcp-call-order-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    seed_fake_app_server_mcp(app_server.app_server().unwrap()).await;

    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/mcp call fake echo hello", &local_command_tx)
        .await
        .expect("mcp call command succeeds");

    let command_index = state
        .messages
        .iter()
        .position(|message| message.content.contains("/mcp call fake echo hello"))
        .expect("command output message");
    let result_index = state
        .messages
        .iter()
        .position(|message| message.content.contains("MCP Tool: fake::echo"))
        .expect("mcp tool result message");
    assert!(
        command_index < result_index,
        "command output should precede MCP result"
    );
}

#[tokio::test]
async fn mcp_list_and_status_slash_commands_show_trust_column() {
    let home_dir = test_temp_path("mcp-list-home");
    let cwd = test_temp_path("mcp-list-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    seed_fake_app_server_mcp(app_server.app_server().unwrap()).await;

    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    state
        .handle_command(&app_server, "/mcp list", &local_command_tx)
        .await
        .expect("mcp list command succeeds");
    assert_eq!(state.status_line, "Listed MCP servers.");
    assert!(
        state.messages.iter().any(|message| {
            message.content.contains("fake") && message.content.contains("trust=trusted")
        }),
        "/mcp list output should include the trust column"
    );

    state
        .handle_command(&app_server, "/mcp status", &local_command_tx)
        .await
        .expect("mcp status command succeeds");
    assert_eq!(state.status_line, "Showed MCP server status.");
    assert!(
        state.messages.iter().any(|message| {
            message.content.contains("fake") && message.content.contains("trust=trusted")
        }),
        "/mcp status output should include the trust column"
    );
}

#[tokio::test]
async fn mcp_add_trust_remove_slash_commands_mutate_registry() {
    let home_dir = test_temp_path("mcp-add-home");
    let cwd = test_temp_path("mcp-add-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
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
            "/mcp add docs websocket modeled://docs.local",
            &local_command_tx,
        )
        .await
        .expect("mcp add command succeeds");
    assert_eq!(
        state.status_line,
        "Added MCP server `docs` (untrusted; run `/mcp trust docs` to enable)."
    );
    assert!(
        app_server
            .app_server()
            .unwrap()
            .list_mcp_servers()
            .await
            .iter()
            .any(|server| server.id == "docs"),
        "added server should appear in the registry"
    );
    assert_eq!(
        app_server
            .app_server()
            .unwrap()
            .mcp_server_trust("docs")
            .await,
        orbcode_mcp::McpServerTrust::Unknown,
        "a freshly added server must start untrusted (Unknown), not Trusted"
    );

    state
        .handle_command(&app_server, "/mcp distrust docs", &local_command_tx)
        .await
        .expect("mcp distrust command succeeds");
    assert_eq!(state.status_line, "Denied MCP server `docs`.");
    assert_eq!(
        app_server
            .app_server()
            .unwrap()
            .mcp_server_trust("docs")
            .await,
        orbcode_mcp::McpServerTrust::Denied
    );

    state
        .handle_command(&app_server, "/mcp remove docs", &local_command_tx)
        .await
        .expect("mcp remove command succeeds");
    assert_eq!(state.status_line, "Removed MCP server `docs`.");
    assert!(
        !app_server
            .app_server()
            .unwrap()
            .list_mcp_servers()
            .await
            .iter()
            .any(|server| server.id == "docs"),
        "removed server should be gone from the registry"
    );
}

async fn seed_fake_app_server_mcp(app_server: &AppServer) {
    app_server
        .upsert_mcp_server(orbcode_mcp::McpServerConfig {
            id: "fake".to_string(),
            transport: orbcode_mcp::McpTransport::WebSocket,
            endpoint: "modeled://fake.local".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            cwd: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Ready,
            error: None,
            summary: "Fake stored MCP for tui tests".to_string(),
            auth: orbcode_mcp::McpAuth::None,
            trust: orbcode_mcp::McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("seed fake MCP server for tui");
}
