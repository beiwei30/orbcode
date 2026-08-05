//! Split-core smoke test: proves AppClient can drive the full protocol
//! lifecycle over a real Unix socket transport. Exercises the same protocol
//! path that a remote TUI uses, verifying that sessions, turns, and
//! server-requests work correctly over IPC.

use std::collections::BTreeMap;
use std::time::Duration;

use orbcode_app_server_client::{
    AppClient, McpAuth, McpListServersResult, McpServerInput, McpServerStatus, McpServerTrust,
    McpTransport,
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

fn secret_bearing_mcp_input() -> McpServerInput {
    McpServerInput {
        id: "transport-redaction-server".to_string(),
        transport: McpTransport::StreamableHttp,
        endpoint: "https://url-user-canary:url-password-canary@example.com/mcp?token=url-query-canary#url-fragment-canary".to_string(),
        args: vec!["arg-canary".to_string()],
        env: BTreeMap::from([("TOKEN".to_string(), "env-canary".to_string())]),
        cwd: None,
        headers: BTreeMap::from([(
            "Authorization".to_string(),
            "header-canary".to_string(),
        )]),
        enabled: false,
        status: McpServerStatus::Failed,
        error: Some("error-canary".to_string()),
        summary: "transport redaction fixture".to_string(),
        auth: McpAuth::Header {
            name: "X-Auth".to_string(),
            value: "auth-header-canary".to_string(),
        },
        trust: McpServerTrust::Unknown,
        transport_type_hint: None,
        source: None,
    }
}

async fn upsert_and_list_redacted(client: &AppClient) -> McpListServersResult {
    client
        .upsert_mcp_server(secret_bearing_mcp_input())
        .await
        .expect("upsert secret-bearing MCP fixture");
    let result = client.list_mcp_servers().await.expect("list MCP servers");
    let serialized = serde_json::to_string(&result).expect("serialize typed MCP list result");
    for canary in [
        "url-user-canary",
        "url-password-canary",
        "url-query-canary",
        "url-fragment-canary",
        "arg-canary",
        "env-canary",
        "header-canary",
        "auth-header-canary",
        "error-canary",
    ] {
        assert!(
            !serialized.contains(canary),
            "leaked {canary}: {serialized}"
        );
    }
    result
}

async fn spawn_serve_socket() -> (
    tokio::process::Child,
    String,
    String,
    TempDir,
    TempDir,
    TempDir,
) {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let sock_dir = tempfile::tempdir().expect("sock");
    let sock_path = sock_dir.path().join("test.sock");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key","ANTHROPIC_BASE_URL":"mock://anthropic?scenario=tool_use&key=bash&command=echo+hi"}}"#,
    )
    .expect("write settings");

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--socket")
        .arg(sock_path.to_str().unwrap())
        .current_dir(cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", home.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn orbcode serve --socket");

    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    let info: Value = tokio::time::timeout(Duration::from_secs(15), async {
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await.expect("read");
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v.get("transport").is_some()
            {
                return v;
            }
        }
    })
    .await
    .expect("connection info");

    let path = info["path"].as_str().unwrap().to_string();
    let token = info["auth_token"].as_str().unwrap().to_string();

    (child, path, token, home, cwd, sock_dir)
}

async fn spawn_serve_websocket() -> (tokio::process::Child, String, String, TempDir, TempDir) {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key","ANTHROPIC_BASE_URL":"mock://anthropic?scenario=tool_use&key=bash&command=echo+hi"}}"#,
    )
    .expect("write settings");

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--websocket")
        .arg("127.0.0.1:0")
        .current_dir(cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", home.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn orbcode serve --websocket");

    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    let info: Value = tokio::time::timeout(Duration::from_secs(15), async {
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await.expect("read");
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v.get("transport").and_then(|v| v.as_str()) == Some("websocket")
            {
                return v;
            }
        }
    })
    .await
    .expect("connection info");

    let addr = info["addr"].as_str().unwrap().to_string();
    let token = info["auth_token"].as_str().unwrap().to_string();

    (child, addr, token, home, cwd)
}

#[tokio::test]
async fn split_core_bootstrap_and_session_list() {
    let (mut child, path, token, _home, _cwd, _sock) = spawn_serve_socket().await;

    let client = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match AppClient::connect_socket(std::path::Path::new(&path), &token).await {
                Ok(c) => return c,
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .expect("connect within 5s");

    let sessions = client.list_sessions().await.expect("list_sessions");
    assert!(sessions.is_empty());

    let bootstrap = client.bootstrap(None).await.expect("bootstrap");
    assert!(!bootstrap.session.session_id.is_empty());

    drop(client);
    child.kill().await.ok();
}

#[tokio::test]
async fn mcp_list_redaction_is_equivalent_in_process_ndjson_and_websocket() {
    let in_process_home = tempfile::tempdir().expect("in-process home");
    let in_process_cwd = tempfile::tempdir().expect("in-process cwd");
    let app = orbcode_app_server::AppServer::new(
        in_process_cwd.path(),
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(in_process_home.path().to_path_buf()),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("in-process app server");
    let in_process_client = AppClient::new(app).await.expect("in-process client");
    let in_process_result = upsert_and_list_redacted(&in_process_client).await;

    let (mut socket_child, path, socket_token, _socket_home, _socket_cwd, _sock_dir) =
        spawn_serve_socket().await;
    let socket_client = AppClient::connect_socket(std::path::Path::new(&path), &socket_token)
        .await
        .expect("socket client");
    let socket_result = upsert_and_list_redacted(&socket_client).await;

    let (mut websocket_child, addr, websocket_token, _websocket_home, _websocket_cwd) =
        spawn_serve_websocket().await;
    let websocket_client = AppClient::connect_websocket(&format!("ws://{addr}"), &websocket_token)
        .await
        .expect("websocket client");
    let websocket_result = upsert_and_list_redacted(&websocket_client).await;

    assert_eq!(in_process_result, socket_result);
    assert_eq!(in_process_result, websocket_result);

    let in_process_tools = in_process_client
        .list_tools()
        .await
        .expect("in-process tools");
    let socket_tools = socket_client.list_tools().await.expect("socket tools");
    let websocket_tools = websocket_client
        .list_tools()
        .await
        .expect("websocket tools");
    assert_eq!(in_process_tools, socket_tools);
    assert_eq!(in_process_tools, websocket_tools);

    let in_process_capabilities = in_process_client
        .mcp_capabilities()
        .await
        .expect("in-process MCP capabilities");
    let socket_capabilities = socket_client
        .mcp_capabilities()
        .await
        .expect("socket MCP capabilities");
    let websocket_capabilities = websocket_client
        .mcp_capabilities()
        .await
        .expect("websocket MCP capabilities");
    assert_eq!(in_process_capabilities, socket_capabilities);
    assert_eq!(in_process_capabilities, websocket_capabilities);

    drop(socket_client);
    drop(websocket_client);
    socket_child.kill().await.ok();
    websocket_child.kill().await.ok();
}

#[tokio::test]
async fn split_core_submit_turn_receives_events() {
    let (mut child, path, token, _home, _cwd, _sock) = spawn_serve_socket().await;

    let client = AppClient::connect_socket(std::path::Path::new(&path), &token)
        .await
        .expect("connect");

    let bootstrap = client.bootstrap(None).await.expect("bootstrap");
    let session_id = bootstrap.session.session_id;

    let subscription = client
        .submit_turn(&session_id, "test")
        .await
        .expect("submit_turn");
    assert!(!subscription.subscription_id.is_empty());

    // Read notifications until we see turn_finished or timeout
    let mut notif_rx = client
        .take_notification_receiver()
        .await
        .expect("notification_rx");

    let mut saw_stream_event = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, notif_rx.recv()).await {
            Ok(Some(notif)) => {
                if notif.method == "stream/event" {
                    saw_stream_event = true;
                    break;
                }
            }
            _ => break,
        }
    }

    assert!(
        saw_stream_event,
        "should receive at least one stream/event notification"
    );

    drop(client);
    child.kill().await.ok();
}

#[tokio::test]
async fn split_core_websocket_submit_turn_stream_receives_events() {
    let (mut child, addr, token, _home, _cwd) = spawn_serve_websocket().await;

    let client = AppClient::connect_websocket(&format!("ws://{addr}"), &token)
        .await
        .expect("connect websocket");

    let bootstrap = client.bootstrap(None).await.expect("bootstrap");
    let session_id = bootstrap.session.session_id;

    let mut events = client
        .submit_turn_stream(&session_id, "test")
        .await
        .expect("submit turn stream");
    let _first = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("first stream event")
        .expect("stream event");

    drop(client);
    child.kill().await.ok();
}

#[tokio::test]
async fn split_core_permission_deny_via_server_request() {
    let (mut child, path, token, _home, _cwd, _sock) = spawn_serve_socket().await;

    let client = AppClient::connect_socket(std::path::Path::new(&path), &token)
        .await
        .expect("connect");

    let bootstrap = client.bootstrap(None).await.expect("bootstrap");
    let session_id = bootstrap.session.session_id;

    let _subscription = client
        .submit_turn(&session_id, "test")
        .await
        .expect("submit_turn");

    let mut srv_req_rx = client
        .take_server_request_receiver()
        .await
        .expect("server_request_rx");
    let mut notif_rx = client
        .take_notification_receiver()
        .await
        .expect("notification_rx");

    // Wait for permission/request server-request and deny it
    let mut permission_denied = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        if remaining.is_zero() {
            break;
        }
        tokio::select! {
            req = srv_req_rx.recv() => {
                if let Some(req) = req
                    && req.method == "permission/request"
                {
                    client
                        .respond_to_server_request(
                            req.id,
                            orbcode_app_server_protocol::ResponseResult::Success {
                                data: Some(serde_json::json!({"decision": "deny"})),
                            },
                        )
                        .await
                        .expect("respond");
                    permission_denied = true;
                }
            }
            notif = notif_rx.recv() => {
                if let Some(notif) = notif {
                    let event_type = notif.params["event"]["event"].as_str().unwrap_or("");
                    if event_type == "turn_finished" || event_type == "turn_cancelled" {
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(remaining) => break,
        }
    }

    assert!(
        permission_denied,
        "should have received and denied a permission/request"
    );

    drop(client);
    child.kill().await.ok();
}
