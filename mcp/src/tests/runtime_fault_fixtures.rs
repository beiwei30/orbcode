use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::json;

use super::{
    FakeHttpResponse, fake_websocket_mcp_response, json_rpc_response,
    read_blocking_websocket_frame, read_blocking_websocket_text, read_http_request,
    spawn_fake_http_mcp_server, temp_paths, websocket_server_text_frame,
};
use crate::cancel::McpCancellationToken;
use crate::transport::websocket::{WEBSOCKET_ACCEPT, WebSocketMcpClient};
use crate::*;

fn server_config(id: &str, transport: McpTransport, endpoint: String) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        transport,
        endpoint,
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: format!("Test: {id}"),
        auth: McpAuth::None,
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn websocket_close_frame() -> Vec<u8> {
    vec![0x88, 0x02, 0x03, 0xe8]
}

fn init_response_json() -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake", "version": "0.1.0"}
        }
    })
    .to_string()
}

fn tools_call_response_json(text: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "result": {
            "content": [{"type": "text", "text": text}],
            "isError": false
        }
    })
    .to_string()
}

// ─── Unreachable endpoint tests ───

#[tokio::test]
async fn unreachable_http_endpoint_invoke_tool_sets_failed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("http://{}/mcp", listener.local_addr().expect("addr"));
    drop(listener);

    let (home, cwd) = temp_paths("fault-http-unreachable-invoke");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("unreachable", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("unreachable", "echo", r#"{"text":"hi"}"#)
        .await;
    assert!(
        result.is_err(),
        "invoke_tool should fail on unreachable endpoint"
    );

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "unreachable")
        .expect("find server");
    assert_eq!(server.status, McpServerStatus::Failed);
    assert!(server.error.is_some(), "error message should be set");
}

#[tokio::test]
async fn unreachable_websocket_endpoint_list_tools_sets_failed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("addr"));
    drop(listener);

    let (home, cwd) = temp_paths("fault-ws-unreachable-list");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("ws-dead", McpTransport::WebSocket, endpoint))
        .await
        .expect("upsert");

    let result = registry.list_tools("ws-dead").await;
    assert!(result.is_err());

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "ws-dead")
        .expect("find server");
    assert_eq!(server.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn unreachable_websocket_endpoint_invoke_tool_sets_failed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("addr"));
    drop(listener);

    let (home, cwd) = temp_paths("fault-ws-unreachable-invoke");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("ws-dead", McpTransport::WebSocket, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("ws-dead", "echo", r#"{"text":"test"}"#)
        .await;
    assert!(result.is_err());

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "ws-dead")
        .expect("find server");
    assert_eq!(server.status, McpServerStatus::Failed);
}

// ─── HTTP connection drop tests ───

#[tokio::test]
async fn http_invoke_tool_call_connection_drop_is_not_retried() {
    // A `tools/call` (non-idempotent) whose connection drops mid-send must NOT be
    // transparently retried: the client cannot prove the server didn't already
    // execute it (only the response was lost), so replaying could double-execute.
    // `initialize` (idempotent) succeeds first; the dropped `tools/call` then
    // surfaces an error instead of silently recovering.
    use std::io::Write;
    use std::net::TcpListener as StdTcpListener;

    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let endpoint = format!("http://{addr}/mcp");

    std::thread::spawn(move || {
        // 1) initialize handshake succeeds.
        let (mut stream, _) = listener.accept().expect("accept init");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let _ = read_http_request(&mut stream);
        let body = init_response_json();
        let payload = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(payload.as_bytes()).expect("write init");

        // 2) the tools/call connection is dropped without a response. If the
        // client (wrongly) retried a non-idempotent call, it would open a
        // further connection; accept once more so a spurious retry would be
        // observable rather than hanging, then drop it too.
        for _ in 0..2 {
            if let Ok((stream, _)) = listener.accept() {
                drop(stream);
            }
        }
    });

    let (home, cwd) = temp_paths("fault-http-call-drop-no-retry");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"test"}"#)
        .await;
    assert!(
        result.is_err(),
        "a dropped tools/call must surface an error, not silently retry: {result:?}"
    );
}

#[tokio::test]
async fn http_invoke_tool_double_connection_drop_exhausts_retry() {
    use std::net::TcpListener as StdTcpListener;

    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let endpoint = format!("http://{addr}/mcp");

    std::thread::spawn(move || {
        for _ in 0..2 {
            let (stream, _) = listener.accept().expect("accept");
            drop(stream);
        }
    });

    let (home, cwd) = temp_paths("fault-http-double-drop");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"test"}"#)
        .await;
    assert!(result.is_err(), "should fail after retry exhaustion");

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "remote")
        .expect("find server");
    assert_eq!(server.status, McpServerStatus::Failed);
}

// ─── HTTP malformed response tests ───

#[tokio::test]
async fn http_malformed_json_response_sets_failed() {
    let endpoint = spawn_fake_http_mcp_server(2, |_index, request| {
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(init_response_json());
        }
        FakeHttpResponse::ok("{not valid json at all!!!}".to_string())
    });

    let (home, cwd) = temp_paths("fault-http-malformed-json");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"test"}"#)
        .await;
    assert!(result.is_err(), "malformed JSON should produce an error");

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "remote")
        .expect("find server");
    assert_eq!(server.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn http_sse_without_data_lines_returns_protocol_error() {
    let endpoint = spawn_fake_http_mcp_server(2, |_index, request| {
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(init_response_json());
        }
        FakeHttpResponse {
            status: "200 OK",
            content_type: "text/event-stream",
            headers: Vec::new(),
            body: "event: message\nid: 1\n\n".to_string(),
            delay: None,
        }
    });

    let (home, cwd) = temp_paths("fault-http-sse-no-data");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"test"}"#)
        .await;
    let error = result.expect_err("SSE without data lines should fail");
    let message = error.to_string();
    assert!(
        message.contains("did not contain a matching JSON-RPC response"),
        "expected missing matching response error, got: {message}"
    );
}

#[tokio::test]
async fn http_503_during_invoke_tool_sets_failed() {
    let endpoint = spawn_fake_http_mcp_server(2, |_index, request| {
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(init_response_json());
        }
        FakeHttpResponse::status("503 Service Unavailable", "service down")
    });

    let (home, cwd) = temp_paths("fault-http-503-invoke");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"test"}"#)
        .await;
    assert!(result.is_err());

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "remote")
        .expect("find server");
    assert_eq!(server.status, McpServerStatus::Failed);
}

// ─── WebSocket close frame tests ───

#[tokio::test]
async fn websocket_close_frame_during_list_tools_returns_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("addr"));

    std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream.write_all(handshake.as_bytes()).expect("handshake");

        let msg = read_blocking_websocket_text(&mut stream);
        let resp = fake_websocket_mcp_response(&msg);
        stream
            .write_all(&websocket_server_text_frame(resp.as_bytes()))
            .expect("init");

        let _ = read_blocking_websocket_text(&mut stream);
        stream
            .write_all(&websocket_close_frame())
            .expect("close frame");
    });

    let (home, cwd) = temp_paths("fault-ws-close-list");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("ws-close", McpTransport::WebSocket, endpoint))
        .await
        .expect("upsert");

    let result = registry.list_tools("ws-close").await;
    let error = result.expect_err("close frame should produce error");
    assert!(
        error.to_string().contains("closed"),
        "expected 'closed' in error: {error}"
    );
}

#[tokio::test]
async fn websocket_close_frame_during_invoke_tool_returns_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("addr"));

    std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream.write_all(handshake.as_bytes()).expect("handshake");

        let msg = read_blocking_websocket_text(&mut stream);
        let resp = fake_websocket_mcp_response(&msg);
        stream
            .write_all(&websocket_server_text_frame(resp.as_bytes()))
            .expect("init");

        let _ = read_blocking_websocket_text(&mut stream);
        stream
            .write_all(&websocket_close_frame())
            .expect("close frame");
    });

    let (home, cwd) = temp_paths("fault-ws-close-invoke");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("ws-close", McpTransport::WebSocket, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("ws-close", "echo", r#"{"text":"test"}"#)
        .await;
    let error = result.expect_err("close frame should produce error");
    assert!(
        error.to_string().contains("closed"),
        "expected 'closed' in error: {error}"
    );
}

// ─── OAuth concurrent refresh tests ───

#[tokio::test]
async fn concurrent_invoke_tool_expired_oauth_single_refresh() {
    let refresh_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refresh_count_inner = refresh_count.clone();

    let endpoint = spawn_fake_http_mcp_server(7, move |_index, request| {
        if request.starts_with("POST /token ") {
            refresh_count_inner.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return FakeHttpResponse::ok(
                json!({
                    "access_token": "refreshed-token",
                    "refresh_token": "new-refresh",
                    "expires_in": 3600,
                    "scope": "tools.read"
                })
                .to_string(),
            );
        }
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(init_response_json());
        }
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                "content": [{"type": "text", "text": "ok"}],
                "isError": false
            }),
        ))
    });

    let token_endpoint = format!("{}/token", endpoint.trim_end_matches("/mcp"));
    let (home, cwd) = temp_paths("fault-concurrent-invoke-refresh");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");
    registry
        .store_mcp_oauth_token(
            "remote",
            McpOAuthTokenInput {
                access_token: "expired".to_string(),
                refresh_token: Some("stale".to_string()),
                token_endpoint: Some(token_endpoint),
                client_id: Some("orbcode-test".to_string()),
                expires_at: Some(unix_now() - 1),
                scopes: vec!["tools.read".to_string()],
            },
        )
        .await
        .expect("store expired token");

    let (a, b, c) = tokio::join!(
        registry.invoke_tool("remote", "echo", r#"{"text":"1"}"#),
        registry.invoke_tool("remote", "echo", r#"{"text":"2"}"#),
        registry.invoke_tool("remote", "echo", r#"{"text":"3"}"#),
    );
    a.expect("invoke a");
    b.expect("invoke b");
    c.expect("invoke c");
    assert_eq!(
        refresh_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "concurrent invoke_tool should trigger exactly one refresh"
    );
}

#[tokio::test]
async fn oauth_refresh_failure_propagates_to_invoke_tool() {
    let endpoint = spawn_fake_http_mcp_server(1, |_index, _request| {
        FakeHttpResponse::status("400 Bad Request", r#"{"error":"invalid_grant"}"#)
    });

    let token_endpoint = format!("{}/token", endpoint.trim_end_matches("/mcp"));
    let (home, cwd) = temp_paths("fault-oauth-refresh-fail");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");
    registry
        .store_mcp_oauth_token(
            "remote",
            McpOAuthTokenInput {
                access_token: "expired".to_string(),
                refresh_token: Some("stale".to_string()),
                token_endpoint: Some(token_endpoint),
                client_id: Some("orbcode-test".to_string()),
                expires_at: Some(unix_now() - 1),
                scopes: vec!["tools.read".to_string()],
            },
        )
        .await
        .expect("store expired token");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"test"}"#)
        .await;
    assert!(
        result.is_err(),
        "invoke should fail when refresh endpoint returns error"
    );
}

// ─── WebSocket liveness tests ───

#[tokio::test]
async fn websocket_request_blocked_while_pong_pending() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("addr"));

    std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream.write_all(handshake.as_bytes()).expect("handshake");

        let msg = read_blocking_websocket_text(&mut stream);
        let resp = fake_websocket_mcp_response(&msg);
        stream
            .write_all(&websocket_server_text_frame(resp.as_bytes()))
            .expect("init");

        let (opcode, _) = read_blocking_websocket_frame(&mut stream);
        assert_eq!(opcode, 0x9, "expected ping frame");
        std::thread::sleep(Duration::from_secs(3));
    });

    let mut client =
        WebSocketMcpClient::connect(&endpoint, HeaderMap::new(), Duration::from_secs(10))
            .await
            .expect("connect");
    client.initialize().await.expect("init");
    client.send_ping().await.expect("ping");

    let result = client.list_tools().await;
    let error = result.expect_err("request should be blocked while pong pending");
    assert!(
        error.to_string().contains("ping timeout"),
        "expected ping timeout error, got: {error}"
    );
}

// ─── Cancellation propagation tests ───

#[tokio::test]
async fn cancel_token_aborts_http_invoke_tool() {
    let endpoint = spawn_fake_http_mcp_server(2, |index, _request| {
        if index == 0 {
            FakeHttpResponse::ok(init_response_json())
        } else {
            FakeHttpResponse::ok(tools_call_response_json("should-not-arrive"))
                .with_delay(Duration::from_secs(30))
        }
    });

    let (home, cwd) = temp_paths("cancel-http-invoke");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let flag = Arc::new(AtomicBool::new(false));
    let token = McpCancellationToken::from_flag(flag.clone());

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        flag.store(true, Ordering::Relaxed);
    });

    let start = std::time::Instant::now();
    let result = registry
        .invoke_tool_cancellable("remote", "echo", r#"{"text":"test"}"#, Some(token))
        .await;
    let elapsed = start.elapsed();

    assert!(matches!(result, Err(McpError::Cancelled)));
    assert!(
        elapsed < Duration::from_secs(1),
        "cancelled invoke should return quickly, took {elapsed:?}"
    );
}

#[tokio::test]
async fn cancel_token_aborts_websocket_invoke_tool() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("addr"));

    std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream.write_all(handshake.as_bytes()).expect("handshake");

        let msg = read_blocking_websocket_text(&mut stream);
        let resp = fake_websocket_mcp_response(&msg);
        stream
            .write_all(&websocket_server_text_frame(resp.as_bytes()))
            .expect("init resp");

        std::thread::sleep(Duration::from_secs(30));
    });

    let (home, cwd) = temp_paths("cancel-ws-invoke");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config(
            "ws-cancel",
            McpTransport::WebSocket,
            endpoint,
        ))
        .await
        .expect("upsert");

    let flag = Arc::new(AtomicBool::new(false));
    let token = McpCancellationToken::from_flag(flag.clone());

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        flag.store(true, Ordering::Relaxed);
    });

    let start = std::time::Instant::now();
    let result = registry
        .invoke_tool_cancellable("ws-cancel", "echo", r#"{"text":"test"}"#, Some(token))
        .await;
    let elapsed = start.elapsed();

    assert!(matches!(result, Err(McpError::Cancelled)));
    assert!(
        elapsed < Duration::from_secs(1),
        "cancelled invoke should return quickly, took {elapsed:?}"
    );
}

#[tokio::test]
async fn never_responds_http_server_cancelled_within_one_second() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("http://{}/mcp", listener.local_addr().expect("addr"));

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        let _ = read_http_request(&mut stream);
        std::thread::sleep(Duration::from_secs(60));
    });

    let (home, cwd) = temp_paths("never-responds-http");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("stalled", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let flag = Arc::new(AtomicBool::new(false));
    let token = McpCancellationToken::from_flag(flag.clone());

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        flag.store(true, Ordering::Relaxed);
    });

    let start = std::time::Instant::now();
    let result = registry
        .invoke_tool_cancellable("stalled", "echo", r#"{"text":"hello"}"#, Some(token))
        .await;
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(McpError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "never-responds server should be cancelled within 1s, took {elapsed:?}"
    );
}

#[tokio::test]
async fn never_responds_websocket_server_cancelled_within_one_second() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("addr"));

    std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();

        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream.write_all(handshake.as_bytes()).expect("handshake");

        std::thread::sleep(Duration::from_secs(60));
    });

    let (home, cwd) = temp_paths("never-responds-ws");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config(
            "ws-stalled",
            McpTransport::WebSocket,
            endpoint,
        ))
        .await
        .expect("upsert");

    let flag = Arc::new(AtomicBool::new(false));
    let token = McpCancellationToken::from_flag(flag.clone());

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        flag.store(true, Ordering::Relaxed);
    });

    let start = std::time::Instant::now();
    let result = registry
        .invoke_tool_cancellable("ws-stalled", "echo", r#"{"text":"hello"}"#, Some(token))
        .await;
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(McpError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "never-responds WS server should be cancelled within 1s, took {elapsed:?}"
    );
}

#[tokio::test]
async fn invoke_tool_without_cancel_token_still_works() {
    let endpoint = spawn_fake_http_mcp_server(2, |index, _request| {
        if index == 0 {
            FakeHttpResponse::ok(init_response_json())
        } else {
            FakeHttpResponse::ok(tools_call_response_json("no-cancel"))
        }
    });

    let (home, cwd) = temp_paths("no-cancel-backwards-compat");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(server_config("remote", McpTransport::Http, endpoint))
        .await
        .expect("upsert");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"compat"}"#)
        .await
        .expect("invoke_tool without cancel should work");
    assert_eq!(result.output, "no-cancel");
}
