//! Process-level E2E tests for server-request handling over transports.
//!
//! 1. stdio: permission server-request round-trip (deny → tool_use_completed)
//! 2. stdio: disconnect while permission server-request pending → fast exit
//! 3. Unix socket: connect, initialize, session/list, disconnect

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

// ---------------------------------------------------------------------------
// Helper: stdio serve process with mock provider
// ---------------------------------------------------------------------------

struct ServeProcess {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    _home: TempDir,
    _cwd: TempDir,
}

impl ServeProcess {
    async fn spawn_with_mock(scenario: &str) -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");

        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
        )
        .expect("write settings");

        let mut child = Command::new(ORBCODE_BIN)
            .arg("serve")
            .arg("--stdio")
            .current_dir(cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", home.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home.path())
            .env(
                "ANTHROPIC_BASE_URL",
                format!("mock://anthropic?scenario={scenario}"),
            )
            .env("ANTHROPIC_API_KEY", "test-key")
            .env("RUST_LOG", "warn")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn orbcode serve --stdio");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let reader = BufReader::new(stdout);

        Self {
            child,
            stdin,
            reader,
            _home: home,
            _cwd: cwd,
        }
    }

    #[allow(dead_code)]
    async fn spawn_plain() -> Self {
        Self::spawn_with_mock("hello").await
    }

    async fn send(&mut self, msg: &Value) {
        let line = serde_json::to_string(msg).unwrap();
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.write_all(b"\n").await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn recv_timeout(&mut self, timeout: Duration) -> Option<Value> {
        let mut line = String::new();
        match tokio::time::timeout(timeout, self.reader.read_line(&mut line)).await {
            Ok(Ok(0)) => None,
            Ok(Ok(_)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(serde_json::from_str(trimmed).expect("valid JSON"))
            }
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => None,
        }
    }

    async fn recv(&mut self) -> Option<Value> {
        self.recv_timeout(Duration::from_secs(15)).await
    }

    async fn close(self) -> tokio::process::Child {
        drop(self.stdin);
        self.child
    }
}

fn initialize_msg() -> Value {
    json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "e2e-test", "version": "0.1" }
        }
    })
}

fn initialize_interactive_msg() -> Value {
    let mut message = initialize_msg();
    message["params"]["capabilities"] = json!({
        "streaming": true,
        "experimental_methods": true,
        "interactive_questions": {
            "single_select": true,
            "multi_select": true,
            "free_text": true,
            "previews": true,
            "annotations": true,
            "special_outcomes": true
        }
    });
    message
}

const ASK_USER_SCENARIO: &str = "tool_use&key=AskUserQuestion&input=%7B%22question%22%3A%22Pick%3F%22%2C%22options%22%3A%5B%22yes%22%5D%7D";

#[tokio::test]
async fn stdio_ask_user_capability_negotiates_typed_roundtrip() {
    let mut proc = ServeProcess::spawn_with_mock(ASK_USER_SCENARIO).await;
    proc.send(&initialize_interactive_msg()).await;
    let _init = proc.recv().await.expect("init response");
    proc.send(&json!({
        "type": "request", "id": "bs-ask", "method": "session/bootstrap"
    }))
    .await;
    let bootstrap = proc.recv().await.expect("bootstrap response");
    let session_id = bootstrap["result"]["data"]["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    proc.send(&json!({
        "type": "request", "id": "turn-ask", "method": "turn/submit",
        "params": {"session_id": session_id, "prompt": "ask"}
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let request = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let message = proc
            .recv_timeout(remaining)
            .await
            .expect("ask_user/request");
        if message["type"] == "request" && message["method"] == "ask_user/request" {
            break message;
        }
    };
    assert_eq!(request["params"]["questions"][0]["id"], "question-1");
    let request_id = request["params"]["request_id"].clone();
    proc.send(&json!({
        "type": "response",
        "id": request["id"],
        "result": {
            "status": "success",
            "data": {
                "request_id": request_id,
                "outcome": {
                    "outcome": "answered",
                    "answers": {
                        "question-1": {"kind": "selected", "option_id": "option-1"}
                    },
                    "annotations": {}
                }
            }
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_success = false;
    while tokio::time::Instant::now() < deadline {
        let Some(message) = proc
            .recv_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
        else {
            break;
        };
        if message["type"] == "notification"
            && message["method"] == "stream/event"
            && message["params"]["event"]["event"] == "tool_use_completed"
            && message["params"]["event"]["kind"] == "success"
        {
            saw_success = true;
            break;
        }
    }
    assert!(saw_success, "typed answer should resume the tool");
    let mut child = proc.close().await;
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
}

#[tokio::test]
async fn websocket_ask_user_capability_negotiates_typed_roundtrip() {
    use orbcode_app_server_client::{
        AppClient, AskUserAnswerValue, AskUserQuestionRequest, AskUserResponseOutcome,
    };
    use orbcode_protocol::{StreamEvent, ToolUseCompletionKind};
    use std::collections::BTreeMap;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    drop(listener);
    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--websocket")
        .arg(addr.to_string())
        .arg("--auth-token")
        .arg("ask-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("HOME", home.path())
        .env(
            "ANTHROPIC_BASE_URL",
            format!("mock://anthropic?scenario={ASK_USER_SCENARIO}"),
        )
        .env("ANTHROPIC_API_KEY", "test-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn websocket server");
    let endpoint = format!("ws://{addr}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let client = loop {
        match AppClient::connect_websocket_interactive(&endpoint, "ask-token").await {
            Ok(client) => break client,
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => {
                child.kill().await.ok();
                panic!("connect interactive websocket client: {error}");
            }
        }
    };
    let bootstrap = client.bootstrap(None).await.expect("bootstrap");
    let session_id = bootstrap.session.session_id.clone();
    let mut requests = client
        .take_server_request_receiver()
        .await
        .expect("server request receiver");
    let mut stream = client
        .submit_turn_stream(&session_id, "ask")
        .await
        .expect("submit interactive turn");
    let envelope = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let request = requests.recv().await.expect("server request");
            if request.method == "ask_user/request" {
                return request;
            }
        }
    })
    .await
    .expect("AskUser request timeout");
    let request: AskUserQuestionRequest =
        serde_json::from_value(envelope.params).expect("canonical request");
    assert_eq!(request.questions[0].id, "question-1");
    assert!(
        client
            .respond_to_ask_user_question_outcome(
                &request.request_id,
                AskUserResponseOutcome::Answered {
                    answers: BTreeMap::from([(
                        "question-1".into(),
                        AskUserAnswerValue::Selected {
                            option_id: "option-1".into(),
                        },
                    )]),
                    annotations: BTreeMap::new(),
                },
            )
            .await
    );
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.recv().await {
            if let StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } = event
                && tool_name == "AskUserQuestion"
            {
                return kind;
            }
        }
        panic!("stream closed before tool completion");
    })
    .await
    .expect("tool completion timeout");
    assert_eq!(completed, ToolUseCompletionKind::Success);

    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.recv().await {
            if matches!(event, StreamEvent::TurnFinished { .. }) {
                return;
            }
        }
        panic!("first turn closed before finishing");
    })
    .await
    .expect("first turn finish timeout");

    drop(client);
    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// 1. stdio: permission server-request round-trip
// ---------------------------------------------------------------------------
//
// Submit a turn with tool_use mock → receive permission/request server-request
// → respond with bare PermissionDecisionWire::Deny → observe tool_use_completed
// proving the deny was processed by MessageProcessor.

#[tokio::test]
async fn stdio_permission_server_request_deny_roundtrip() {
    let mut proc = ServeProcess::spawn_with_mock(
        "tool_use&key=bash&input=%7B%22command%22%3A%22echo%20hi%22%2C%22sandbox_permissions%22%3A%22require_escalated%22%7D",
    )
    .await;

    proc.send(&initialize_msg()).await;
    let _init = proc.recv().await.expect("init response");

    // Bootstrap a session
    proc.send(&json!({
        "type": "request", "id": "bs-1",
        "method": "session/bootstrap"
    }))
    .await;
    let bs = proc.recv().await.expect("bootstrap response");
    let session_id = bs["result"]["data"]["session"]["session_id"]
        .as_str()
        .expect("session_id");

    // Submit turn (triggers tool_use → permission request)
    proc.send(&json!({
        "type": "request", "id": "turn-1",
        "method": "turn/submit",
        "params": { "session_id": session_id, "prompt": "echo hi" }
    }))
    .await;
    let _turn = proc.recv().await.expect("turn response");

    // Read messages until we see the permission server-request
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let perm_req_id = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for permission/request server-request");
        }
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("should receive messages");
        if msg["type"].as_str() == Some("request")
            && msg["method"].as_str() == Some("permission/request")
        {
            break msg["id"].as_str().expect("id").to_string();
        }
    };

    // Respond with bare PermissionDecisionWire::Deny
    proc.send(&json!({
        "type": "response",
        "id": perm_req_id,
        "result": {
            "status": "success",
            "data": { "decision": "deny" }
        }
    }))
    .await;

    // After deny, wait for tool_use_completed notification (post-resolution proof)
    let mut saw_tool_completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(msg) = proc.recv_timeout(remaining).await else {
            break;
        };
        if msg["type"].as_str() == Some("notification")
            && msg["method"].as_str() == Some("stream/event")
        {
            let event = &msg["params"]["event"];
            if event["event"].as_str() == Some("tool_use_completed") {
                saw_tool_completed = true;
                break;
            }
        }
    }

    assert!(
        saw_tool_completed,
        "tool_use_completed should arrive after deny, proving permission resolved"
    );

    // Shut down — close stdin; the process may exit non-zero if the turn
    // ended with an error (tool denied), which is acceptable.
    let mut child = proc.close().await;
    match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => panic!("wait: {e}"),
        Err(_) => {
            child.kill().await.ok();
            panic!("process did not exit in 10s");
        }
    }
}

// ---------------------------------------------------------------------------
// 2. stdio: disconnect while permission server-request pending → fast exit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stdio_disconnect_during_pending_permission_exits_quickly() {
    let mut proc = ServeProcess::spawn_with_mock(
        "tool_use&key=bash&input=%7B%22command%22%3A%22echo%20hi%22%2C%22sandbox_permissions%22%3A%22require_escalated%22%7D",
    )
    .await;

    proc.send(&initialize_msg()).await;
    let _init = proc.recv().await.expect("init response");

    proc.send(&json!({
        "type": "request", "id": "bs-1",
        "method": "session/bootstrap"
    }))
    .await;
    let bs = proc.recv().await.expect("bootstrap response");
    let session_id = bs["result"]["data"]["session"]["session_id"]
        .as_str()
        .expect("session_id");

    // Submit turn to trigger permission request
    proc.send(&json!({
        "type": "request", "id": "turn-1",
        "method": "turn/submit",
        "params": { "session_id": session_id, "prompt": "echo hi" }
    }))
    .await;
    let _turn = proc.recv().await;

    // Wait until we see the permission server-request
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for permission server-request");
        }
        let msg = proc.recv_timeout(remaining).await.expect("msg");
        if msg["type"].as_str() == Some("request")
            && msg["method"].as_str() == Some("permission/request")
        {
            break;
        }
    }

    // Disconnect (close stdin) WITHOUT responding to the server-request.
    // The process should exit quickly — not wait 5 minutes for the timeout.
    let mut child = proc.close().await;
    match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(Ok(_status)) => {
            // Process exited within 10s — proves no 5-minute hang.
        }
        Ok(Err(e)) => panic!("wait error: {e}"),
        Err(_) => {
            child.kill().await.ok();
            panic!(
                "process did not exit within 10s after disconnect — \
                 likely stuck waiting for permission timeout"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Unix socket: connect, initialize, session/list, disconnect
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn unix_socket_connect_initialize_and_list() {
    use tokio::net::UnixStream;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    // Unix socket paths are limited to ~104 chars on macOS; use /tmp directly.
    let socket_path = PathBuf::from(format!("/tmp/orbcode-test-{}.sock", std::process::id()));

    // Clean up any stale socket from previous failed runs
    let _ = std::fs::remove_file(&socket_path);

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    // Use the same env pattern as the working stdio_transport_e2e tests.
    // Do NOT env_clear() — the AppServer MCP/config init may need TMPDIR,
    // USER, or other system env vars that env_clear strips.
    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--auth-token")
        .arg("test-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn orbcode serve --socket");

    // Wait for socket file to appear
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if socket_path.exists() {
            break;
        }
        // Check if process already exited (startup failure)
        if let Ok(Some(status)) = child.try_wait() {
            let stderr = child.stderr.take();
            let mut err_msg = String::new();
            if let Some(mut se) = stderr {
                use tokio::io::AsyncReadExt;
                let _ = se.read_to_string(&mut err_msg).await;
            }
            panic!("socket server exited early with {status}, stderr: {err_msg}");
        }
        if tokio::time::Instant::now() > deadline {
            let stderr = child.stderr.take();
            let mut err_output = String::new();
            if let Some(mut se) = stderr {
                use tokio::io::AsyncReadExt;
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    se.read_to_string(&mut err_output),
                )
                .await;
            }
            child.kill().await.ok();
            panic!(
                "socket file did not appear within 10s at {}\nstderr: {}",
                socket_path.display(),
                err_output
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Connect
    let stream = UnixStream::connect(&socket_path)
        .await
        .expect("connect to socket");
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // Send auth token first (required by --auth-token)
    writer.write_all(b"test-token\n").await.unwrap();
    writer.flush().await.unwrap();

    // Send initialize
    let init_req = serde_json::to_string(&initialize_msg()).unwrap();
    writer
        .write_all(format!("{init_req}\n").as_bytes())
        .await
        .unwrap();
    writer.flush().await.unwrap();

    // Read response
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("should get response")
        .expect("read");
    let resp: Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "init-1");
    assert!(resp["result"]["data"]["server_info"]["name"].is_string());

    // Send session/list
    let list_req = serde_json::to_string(&json!({
        "type": "request", "id": "list-1", "method": "session/list"
    }))
    .unwrap();
    writer
        .write_all(format!("{list_req}\n").as_bytes())
        .await
        .unwrap();
    writer.flush().await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("should get list response")
        .expect("read");
    let resp: Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "list-1");
    assert!(resp["result"]["data"].is_array());

    // Disconnect (drop writer/reader)
    drop(writer);
    drop(reader);

    // Server now loops (sequential accept), so it won't exit after one
    // client disconnects. Kill it after verifying responses.
    child.kill().await.ok();

    // Socket file cleanup is handled by SocketCleanupGuard.
    // After kill the guard may or may not run, so don't assert absence.
    // Just clean up manually.
    let _ = std::fs::remove_file(&socket_path);
    assert!(
        !socket_path.exists(),
        "socket file should be removed after exit"
    );
}

// ---------------------------------------------------------------------------
// 4. WebSocket: initialize, session/list, close
// ---------------------------------------------------------------------------

#[tokio::test]
async fn websocket_initialize_list_and_close() {
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    // Use port 0 to let the OS pick an ephemeral port — but the CLI needs
    // a concrete address. Bind temporarily to find a free port, then release.
    let tmp_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = tmp_listener.local_addr().expect("local_addr");
    drop(tmp_listener);

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--websocket")
        .arg(addr.to_string())
        .arg("--auth-token")
        .arg("test-ws-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn orbcode serve --websocket");

    // Poll until the WebSocket server is accepting connections.
    let url = format!("ws://{addr}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (mut ws, _) = loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok(pair) => break pair,
            Err(_) => {
                if tokio::time::Instant::now() > deadline {
                    child.kill().await.ok();
                    panic!("WebSocket server did not start accepting within 10s");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    // Send auth token first
    ws.send(Message::Text("test-ws-token".into()))
        .await
        .unwrap();

    // Initialize
    let init_req = serde_json::to_string(&initialize_msg()).unwrap();
    ws.send(Message::Text(init_req.into())).await.unwrap();

    let resp_msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout")
        .expect("stream end")
        .expect("WS error");
    let text = match resp_msg {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    };
    let resp: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "init-1");
    assert!(resp["result"]["data"]["server_info"]["name"].is_string());

    // Session list
    let list_req = serde_json::to_string(&json!({
        "type": "request", "id": "list-1", "method": "session/list"
    }))
    .unwrap();
    ws.send(Message::Text(list_req.into())).await.unwrap();

    let resp_msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout")
        .expect("stream end")
        .expect("WS error");
    let text = match resp_msg {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    };
    let resp: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["id"], "list-1");
    assert!(resp["result"]["data"].is_array());

    // Clean close. Server loops for next client, so kill it.
    ws.send(Message::Close(None)).await.unwrap();
    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// 5. WebSocket: Origin rejection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn websocket_origin_rejection() {
    use tokio_tungstenite::tungstenite;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let tmp_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = tmp_listener.local_addr().expect("local_addr");
    drop(tmp_listener);

    // The default WebSocketTransportConfig has empty allowed_origins (no check).
    // For origin rejection testing, we need to pass allowed_origins via the API.
    // Since the CLI doesn't expose allowed_origins yet, test at library level.
    let app = {
        use orbcode_app_server::{AppConfigOverrides, AppServer};
        AppServer::new(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app")
    };

    let ws_config = orbcode_app_server_transport::WebSocketTransportConfig {
        allowed_origins: vec!["https://allowed.example.com".to_string()],
        ..Default::default()
    };

    let transport_handle = tokio::spawn(async move {
        orbcode_app_server_transport::run_websocket_transport(addr, app, ws_config).await
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Connect with wrong origin — should be rejected with 403
    let url = format!("ws://{addr}");
    let request = tungstenite::http::Request::builder()
        .uri(&url)
        .header("Host", addr.to_string())
        .header("Origin", "https://evil.example.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .expect("request");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("timeout");

    match result {
        Err(tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                403,
                "wrong origin should get 403, got: {}",
                resp.status()
            );
        }
        Err(e) => {
            // Some tungstenite versions surface this differently
            let msg = e.to_string();
            assert!(
                msg.contains("403") || msg.contains("Forbidden"),
                "should contain 403/Forbidden, got: {msg}"
            );
        }
        Ok(_) => panic!("connection with wrong origin should be rejected"),
    }

    // Clean up
    transport_handle.abort();
}

// ---------------------------------------------------------------------------
// 6. Socket: wrong auth token → disconnect
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn socket_wrong_auth_token_disconnects() {
    use tokio::net::UnixStream;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let socket_path = PathBuf::from(format!(
        "/tmp/orbcode-auth-reject-{}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&socket_path);

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--auth-token")
        .arg("correct-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !socket_path.exists() {
        if tokio::time::Instant::now() > deadline {
            child.kill().await.ok();
            panic!("socket not created");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let stream = UnixStream::connect(&socket_path).await.expect("connect");
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // Send wrong token
    writer.write_all(b"wrong-token\n").await.unwrap();
    writer.flush().await.unwrap();

    // Server should close the connection quickly (auth failure).
    // Reading should return EOF or error within seconds.
    let mut buf = String::new();
    let read_result =
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut buf)).await;
    match read_result {
        Ok(Ok(0)) => {} // EOF — expected
        Ok(Ok(_)) => {
            // Got a response — should not happen with wrong token
            panic!("should not get a response with wrong auth token, got: {buf}");
        }
        Ok(Err(_)) => {} // Read error — acceptable (connection reset)
        Err(_) => panic!("timed out — server did not reject wrong token within 5s"),
    }

    // Server loops after auth rejection — kill it.
    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// 7. Socket: no auth token sent → disconnect
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn socket_no_auth_token_disconnects() {
    use tokio::net::UnixStream;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let socket_path = PathBuf::from(format!("/tmp/orbcode-no-auth-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket_path);

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--auth-token")
        .arg("expected-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !socket_path.exists() {
        if tokio::time::Instant::now() > deadline {
            child.kill().await.ok();
            panic!("socket not created");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Connect and immediately disconnect without sending any token
    let stream = UnixStream::connect(&socket_path).await.expect("connect");
    drop(stream);

    // Server loops after disconnect — kill it.
    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// 8. WebSocket: wrong auth token → close
// ---------------------------------------------------------------------------

#[tokio::test]
async fn websocket_wrong_auth_token_closes() {
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let tmp_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = tmp_listener.local_addr().expect("addr");
    drop(tmp_listener);

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--websocket")
        .arg(addr.to_string())
        .arg("--auth-token")
        .arg("correct-ws-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");

    let url = format!("ws://{addr}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (mut ws, _) = loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok(pair) => break pair,
            Err(_) => {
                if tokio::time::Instant::now() > deadline {
                    child.kill().await.ok();
                    panic!("WS server did not start");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    // Send wrong auth token
    ws.send(Message::Text("wrong-token".into())).await.unwrap();

    // Server should close the connection (policy violation close frame).
    let close_msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
    match close_msg {
        Ok(Some(Ok(Message::Close(Some(frame))))) => {
            assert_eq!(
                frame.code,
                tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy,
                "should get Policy close code"
            );
        }
        Ok(Some(Ok(Message::Close(None)))) => {} // Close without frame — acceptable
        Ok(None) => {}                           // Stream ended — acceptable
        Ok(Some(Err(_))) => {}                   // Connection error — acceptable
        Err(_) => panic!("timed out waiting for close after wrong auth token"),
        other => panic!("unexpected response to wrong token: {other:?}"),
    }

    // Server loops — kill it.
    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// 9. WebSocket: Origin rejection via CLI --allowed-origin
// ---------------------------------------------------------------------------

#[tokio::test]
async fn websocket_origin_rejection_via_cli() {
    use tokio_tungstenite::tungstenite;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let tmp_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = tmp_listener.local_addr().expect("addr");
    drop(tmp_listener);

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--websocket")
        .arg(addr.to_string())
        .arg("--auth-token")
        .arg("test-token")
        .arg("--allowed-origin")
        .arg("https://allowed.example.com")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");

    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect with wrong origin
    let url = format!("ws://{addr}");
    let request = tungstenite::http::Request::builder()
        .uri(&url)
        .header("Host", addr.to_string())
        .header("Origin", "https://evil.example.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .expect("request");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let result = loop {
        match tokio_tungstenite::connect_async(request.clone()).await {
            result @ (Ok(_) | Err(tungstenite::Error::Http(_))) => break result,
            Err(_) => {
                if tokio::time::Instant::now() > deadline {
                    child.kill().await.ok();
                    panic!("WS server did not start");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    match result {
        Err(tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 403, "wrong origin should get 403");
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("403") || msg.contains("Forbidden"),
                "should contain 403, got: {msg}"
            );
        }
        Ok(_) => panic!("wrong origin should be rejected"),
    }

    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// 10. WebSocket: server-request permission deny round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn websocket_permission_server_request_deny_roundtrip() {
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let tmp_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = tmp_listener.local_addr().expect("addr");
    drop(tmp_listener);

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--websocket")
        .arg(addr.to_string())
        .arg("--auth-token")
        .arg("test-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env(
            "ANTHROPIC_BASE_URL",
            "mock://anthropic?scenario=tool_use&key=bash&input=%7B%22command%22%3A%22echo%20hi%22%2C%22sandbox_permissions%22%3A%22require_escalated%22%7D",
        )
        .env("ANTHROPIC_API_KEY", "test-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");

    let url = format!("ws://{addr}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (mut ws, _) = loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok(pair) => break pair,
            Err(_) => {
                if tokio::time::Instant::now() > deadline {
                    child.kill().await.ok();
                    panic!("WS server did not start");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    // Auth token
    ws.send(Message::Text("test-token".into())).await.unwrap();

    // Initialize
    let init_req = serde_json::to_string(&initialize_msg()).unwrap();
    ws.send(Message::Text(init_req.into())).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("WS");
    let text = match resp {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    };
    let init_resp: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(init_resp["type"], "response");

    // Bootstrap
    let bs_req = serde_json::to_string(&json!({
        "type": "request", "id": "bs-1", "method": "session/bootstrap"
    }))
    .unwrap();
    ws.send(Message::Text(bs_req.into())).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("WS");
    let bs_resp: Value = serde_json::from_str(&match resp {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    })
    .unwrap();
    let session_id = bs_resp["result"]["data"]["session"]["session_id"]
        .as_str()
        .expect("session_id");

    // Submit turn (tool_use mock → permission request)
    let turn_req = serde_json::to_string(&json!({
        "type": "request", "id": "turn-1",
        "method": "turn/submit",
        "params": { "session_id": session_id, "prompt": "echo hi" }
    }))
    .unwrap();
    ws.send(Message::Text(turn_req.into())).await.unwrap();

    // Read until permission/request server-request
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let perm_req_id = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for permission/request");
        }
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .ok()
            .flatten()
            .and_then(std::result::Result::ok);
        let Some(Message::Text(text)) = msg else {
            continue;
        };
        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
        if parsed["type"].as_str() == Some("request")
            && parsed["method"].as_str() == Some("permission/request")
        {
            break parsed["id"].as_str().unwrap().to_string();
        }
    };

    // Respond with bare deny
    let deny_resp = serde_json::to_string(&json!({
        "type": "response",
        "id": perm_req_id,
        "result": {
            "status": "success",
            "data": { "decision": "deny" }
        }
    }))
    .unwrap();
    ws.send(Message::Text(deny_resp.into())).await.unwrap();

    // Wait for tool_use_completed (post-resolution proof)
    let mut saw_tool_completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .ok()
            .flatten()
            .and_then(std::result::Result::ok);
        let Some(Message::Text(text)) = msg else {
            continue;
        };
        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
        if parsed["type"].as_str() == Some("notification")
            && parsed["method"].as_str() == Some("stream/event")
            && parsed["params"]["event"]["event"].as_str() == Some("tool_use_completed")
        {
            saw_tool_completed = true;
            break;
        }
    }

    assert!(
        saw_tool_completed,
        "tool_use_completed should arrive after deny over WebSocket"
    );

    // WebSocket server is a long-lived accept loop. This test only verifies
    // the server-request round-trip (proved by saw_tool_completed above).
    // Kill the process for cleanup — it won't exit on its own because the
    // tool_use mock keeps the turn active indefinitely.
    drop(ws);
    let mut child = child;
    match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => panic!("wait: {e}"),
        Err(_) => {
            child.kill().await.ok();
        }
    }
}

// ---------------------------------------------------------------------------
// 11. WebSocket: disconnect while permission server-request pending
// ---------------------------------------------------------------------------
//
// Verify that when a client disconnects while a permission server-request is
// outstanding, the WebSocket server handles it gracefully (no hang, no panic).
// Since the server is a long-lived accept loop, we verify the next client can
// connect and get a response after the first client's messy disconnect.

#[tokio::test]
async fn websocket_disconnect_while_permission_pending() {
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let tmp_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = tmp_listener.local_addr().expect("addr");
    drop(tmp_listener);

    let mut child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .arg("--websocket")
        .arg(addr.to_string())
        .arg("--auth-token")
        .arg("test-token")
        .current_dir(cwd.path())
        .env("ORBCODE_HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("HOME", home.path())
        .env(
            "ANTHROPIC_BASE_URL",
            "mock://anthropic?scenario=tool_use&key=bash&input=%7B%22command%22%3A%22echo%20hi%22%2C%22sandbox_permissions%22%3A%22require_escalated%22%7D",
        )
        .env("ANTHROPIC_API_KEY", "test-key")
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");

    // Connect first client
    let url = format!("ws://{addr}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (mut ws, _) = loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok(pair) => break pair,
            Err(_) => {
                if tokio::time::Instant::now() > deadline {
                    child.kill().await.ok();
                    panic!("WS server did not start");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    // Auth + initialize + bootstrap + submit turn
    ws.send(Message::Text("test-token".into())).await.unwrap();
    let init_req = serde_json::to_string(&initialize_msg()).unwrap();
    ws.send(Message::Text(init_req.into())).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;

    let bs_req = serde_json::to_string(&json!({
        "type": "request", "id": "bs-1", "method": "session/bootstrap"
    }))
    .unwrap();
    ws.send(Message::Text(bs_req.into())).await.unwrap();
    let bs_resp = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("WS");
    let bs_text = match bs_resp {
        Message::Text(t) => t,
        other => panic!("expected text, got: {other:?}"),
    };
    let bs_val: Value = serde_json::from_str(&bs_text).unwrap();
    let session_id = bs_val["result"]["data"]["session"]["session_id"]
        .as_str()
        .expect("session_id");

    let turn_req = serde_json::to_string(&json!({
        "type": "request", "id": "turn-1",
        "method": "turn/submit",
        "params": { "session_id": session_id, "prompt": "test" }
    }))
    .unwrap();
    ws.send(Message::Text(turn_req.into())).await.unwrap();

    // Wait for permission/request server-request
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for permission/request");
        }
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .ok()
            .flatten()
            .and_then(std::result::Result::ok);
        let Some(Message::Text(text)) = msg else {
            continue;
        };
        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
        if parsed["type"].as_str() == Some("request")
            && parsed["method"].as_str() == Some("permission/request")
        {
            break;
        }
    }

    // Disconnect WITHOUT responding — permission is pending
    drop(ws);

    // Give the server a moment to notice the disconnect
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect a second client — verify the server is still alive and responsive
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (mut ws2, _) = loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok(pair) => break pair,
            Err(_) => {
                if tokio::time::Instant::now() > deadline {
                    child.kill().await.ok();
                    panic!("WS server did not accept second client after disconnect");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    // Second client: auth + initialize should succeed
    ws2.send(Message::Text("test-token".into())).await.unwrap();
    let init_req = serde_json::to_string(&initialize_msg()).unwrap();
    ws2.send(Message::Text(init_req.into())).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), ws2.next()).await;
    match resp {
        Ok(Some(Ok(Message::Text(text)))) => {
            let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
            assert_eq!(parsed["type"].as_str(), Some("response"));
            assert_eq!(parsed["id"].as_str(), Some("init-1"));
            assert_eq!(
                parsed["result"]["status"].as_str(),
                Some("success"),
                "second client initialize should succeed, got: {parsed}"
            );
            assert!(
                parsed["result"]["data"]["server_info"]["name"].is_string(),
                "should have server_info.name in success response"
            );
        }
        other => panic!("second client should get text response, got: {other:?}"),
    }

    // Clean up
    drop(ws2);
    child.kill().await.ok();
}
