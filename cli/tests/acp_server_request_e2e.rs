//! Process-level smoke tests for the production `orbcode acp` SDK adapter.
//!
//! These tests verify the ACP v1 wire shape used by IDE clients. The old
//! legacy `approval/request` path is no longer production ACP.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::{
    io::{BufRead, BufReader as StdBufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    thread::JoinHandle,
};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::{
    CancelNotification, CloseSessionRequest, ContentBlock, DeleteSessionRequest, Implementation,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionRequest, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason,
};
use agent_client_protocol::{AcpAgent, Client, Dispatch, SessionMessage};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");
const ASK_USER_MOCK_BASE_URL: &str = "mock://anthropic?scenario=tool_use&key=AskUserQuestion&input=%7B%22question%22%3A%22Pick%20a%20color%22%2C%22options%22%3A%5B%22red%22%2C%22blue%22%5D%7D";
const ASK_USER_FREE_TEXT_MOCK_BASE_URL: &str = "mock://anthropic?scenario=tool_use&key=AskUserQuestion&input=%7B%22question%22%3A%22Say%20anything%22%7D";
const BASH_TOOL_MOCK_BASE_URL: &str = "mock://anthropic?scenario=tool_use&key=bash&command=echo+hi";
const HTTP_MCP_TOOL_MOCK_BASE_URL: &str = "mock://anthropic?scenario=tool_use&key=mcp__docs-http__echo&input=%7B%22text%22%3A%22from%20acp%20http%22%7D";
const HANG_MOCK_BASE_URL: &str = "mock://anthropic?scenario=hang";

struct AcpProcess {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    _home: TempDir,
    cwd: TempDir,
}

impl AcpProcess {
    async fn spawn() -> Self {
        Self::spawn_with_base_url("mock://anthropic?scenario=success").await
    }

    async fn spawn_with_base_url(base_url: &str) -> Self {
        Self::spawn_with_base_url_and_allow_tools(base_url, false).await
    }

    async fn spawn_with_base_url_and_allow_tools(base_url: &str, allow_tools: bool) -> Self {
        Self::spawn_with_options(base_url, allow_tools, None).await
    }

    async fn spawn_with_managed_settings(managed_settings: &str) -> Self {
        Self::spawn_with_options(
            "mock://anthropic?scenario=success",
            false,
            Some(managed_settings),
        )
        .await
    }

    async fn spawn_with_options(
        base_url: &str,
        allow_tools: bool,
        managed_settings: Option<&str>,
    ) -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");

        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
        )
        .expect("write settings");
        if let Some(managed_settings) = managed_settings {
            std::fs::write(home.path().join("managed-settings.json"), managed_settings)
                .expect("write managed settings");
        }

        let mut command = Command::new(ORBCODE_BIN);
        if allow_tools {
            command.arg("--allow-tools").arg("true");
        }

        let command = command
            .arg("acp")
            .current_dir(cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", home.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home.path())
            .env("ANTHROPIC_BASE_URL", base_url)
            .env("ANTHROPIC_API_KEY", "test-key")
            .env("RUST_LOG", "warn");
        if managed_settings.is_some() {
            command.env("CLAUDE_CODE_MANAGED_SETTINGS_PATH", home.path());
        }
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn orbcode acp");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let reader = BufReader::new(stdout);

        Self {
            child,
            stdin,
            reader,
            _home: home,
            cwd,
        }
    }

    fn cwd(&self) -> &Path {
        self.cwd.path()
    }

    fn home(&self) -> &Path {
        self._home.path()
    }

    async fn send(&mut self, msg: &Value) {
        let line = serde_json::to_string(msg).expect("serialize JSON-RPC message");
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

    async fn recv_response(&mut self, id: i64) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for JSON-RPC response id {id}"
            );
            let msg = self
                .recv_timeout(remaining)
                .await
                .expect("process should produce JSON-RPC");
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return msg;
            }
        }
    }

    async fn close(mut self) {
        drop(self.stdin);
        match tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => {
                let mut stderr_msg = String::new();
                if let Some(mut stderr) = self.child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = stderr.read_to_string(&mut stderr_msg).await;
                }
                panic!("ACP process exited with {status}, stderr: {stderr_msg}");
            }
            Ok(Err(e)) => panic!("wait error: {e}"),
            Err(_) => {
                self.child.kill().await.ok();
            }
        }
    }

    async fn close_expect_exit(mut self) {
        drop(self.stdin);
        match tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => {
                let mut stderr_msg = String::new();
                if let Some(mut stderr) = self.child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = stderr.read_to_string(&mut stderr_msg).await;
                }
                panic!("ACP process exited with {status}, stderr: {stderr_msg}");
            }
            Ok(Err(e)) => panic!("wait error: {e}"),
            Err(_) => {
                self.child.kill().await.ok();
                panic!("ACP process did not exit after stdin EOF");
            }
        }
    }
}

fn sdk_acp_agent(base_url: &str, home: &Path) -> AcpAgent {
    std::fs::write(
        home.join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    AcpAgent::from_args(vec![
        format!("ORBCODE_HOME={}", home.display()),
        format!("CLAUDE_CONFIG_DIR={}", home.display()),
        format!("HOME={}", home.display()),
        format!("ANTHROPIC_BASE_URL={base_url}"),
        "ANTHROPIC_API_KEY=test-key".to_string(),
        "RUST_LOG=warn".to_string(),
        ORBCODE_BIN.to_string(),
        "acp".to_string(),
    ])
    .expect("build SDK ACP agent")
}

async fn seed_acp_session_transcript(home: &Path, cwd: &Path, session_id: &str, prompt: &str) {
    let cwd = cwd.canonicalize().expect("canonical cwd");
    let payload = json!({
        "type": "user",
        "uuid": format!("{session_id}-user"),
        "timestamp": "2026-06-01T00:00:00.000Z",
        "message": { "role": "user", "content": prompt },
        "cwd": cwd.display().to_string(),
        "sessionId": session_id,
    });
    seed_acp_session_transcript_lines(home, &cwd, session_id, &[payload]).await;
}

async fn seed_acp_session_transcript_lines(
    home: &Path,
    cwd: &Path,
    session_id: &str,
    lines: &[Value],
) {
    let cwd = cwd.canonicalize().expect("canonical cwd");
    let project_dir = home
        .join("projects")
        .join(sanitize_path(&cwd.display().to_string()));
    tokio::fs::create_dir_all(&project_dir)
        .await
        .expect("project dir");
    let mut body = String::new();
    for line in lines {
        body.push_str(&serde_json::to_string(line).expect("serialize"));
        body.push('\n');
    }
    tokio::fs::write(project_dir.join(format!("{session_id}.jsonl")), body)
        .await
        .expect("write transcript");
}

async fn seed_corrupt_acp_session_transcript(home: &Path, cwd: &Path, session_id: &str) {
    let cwd = cwd.canonicalize().expect("canonical cwd");
    let project_dir = home
        .join("projects")
        .join(sanitize_path(&cwd.display().to_string()));
    tokio::fs::create_dir_all(&project_dir)
        .await
        .expect("project dir");
    tokio::fs::write(
        project_dir.join(format!("{session_id}.jsonl")),
        "not-json\n",
    )
    .await
    .expect("write corrupt transcript");
}

/// Mirror of `orbcode_config::claude_home::sanitize_path` for paths short
/// enough to skip the hash suffix. These tempdir paths stay below that cap.
fn sanitize_path(name: &str) -> String {
    const MAX: usize = 200;
    let sanitized: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    assert!(
        sanitized.len() <= MAX,
        "test fixture path exceeds sanitize length cap ({} > {MAX}); shorten the tempdir prefix",
        sanitized.len(),
    );
    sanitized
}

async fn initialize_acp(proc: &mut AcpProcess) {
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
}

async fn new_session_with_fake_mcp(proc: &mut AcpProcess) -> String {
    new_session_with_fake_mcp_id(proc, 2).await
}

async fn new_session_with_fake_mcp_id(proc: &mut AcpProcess, id: i64) -> String {
    let cwd = proc.cwd().to_string_lossy().to_string();
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": [{
                "name": "Docs Server",
                "command": fake_stdio_server_binary(),
                "args": [],
                "env": []
            }]
        }
    }))
    .await;
    let new_session = proc.recv_response(id).await;
    assert!(new_session.get("error").is_none(), "{new_session:?}");
    let new_session: NewSessionResponse = serde_json::from_value(new_session["result"].clone())
        .expect("valid ACP session/new response");
    new_session.session_id.to_string()
}

async fn new_session_with_fake_http_mcp(proc: &mut AcpProcess, id: i64, endpoint: &str) -> String {
    let cwd = proc.cwd().to_string_lossy().to_string();
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": [{
                "type": "http",
                "name": "Docs HTTP",
                "url": endpoint,
                "headers": [{
                    "name": "X-Test",
                    "value": "acp-http"
                }]
            }]
        }
    }))
    .await;
    let new_session = proc.recv_response(id).await;
    assert!(new_session.get("error").is_none(), "{new_session:?}");
    let new_session: NewSessionResponse = serde_json::from_value(new_session["result"].clone())
        .expect("valid ACP session/new response");
    new_session.session_id.to_string()
}

async fn new_session_without_mcp_id(proc: &mut AcpProcess, id: i64) -> String {
    let cwd = proc.cwd().to_string_lossy().to_string();
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": []
        }
    }))
    .await;
    let new_session = proc.recv_response(id).await;
    assert!(new_session.get("error").is_none(), "{new_session:?}");
    let new_session: NewSessionResponse = serde_json::from_value(new_session["result"].clone())
        .expect("valid ACP session/new response");
    new_session.session_id.to_string()
}

async fn prompt_for_mcp_trust_and_respond(
    proc: &mut AcpProcess,
    session_id: &str,
    prompt_text: &str,
    option_id: &str,
) -> PromptResponse {
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": prompt_text}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let permission_request_id = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for ACP MCP trust session/request_permission"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            let request: RequestPermissionRequest = serde_json::from_value(msg["params"].clone())
                .expect("valid ACP requestPermission request");
            assert_eq!(request.session_id.to_string(), session_id);
            assert!(
                request
                    .options
                    .iter()
                    .any(|option| option.option_id.to_string() == option_id),
                "expected MCP trust option {option_id}: {:?}",
                request.options
            );
            break msg["id"].clone();
        }

        assert_ne!(
            msg.get("id").and_then(Value::as_i64),
            Some(3),
            "session/prompt completed before MCP trust request: {msg:?}"
        );
    };

    let response = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new(option_id.to_string()),
    ));
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": permission_request_id,
        "result": serde_json::to_value(response).expect("permission response JSON")
    }))
    .await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;

    let prompt_response = proc.recv_response(3).await;
    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    serde_json::from_value(prompt_response["result"].clone()).expect("valid ACP prompt response")
}

async fn prompt_for_ask_user_and_respond(
    proc: &mut AcpProcess,
    session_id: &str,
    outcome: RequestPermissionOutcome,
) -> (PromptResponse, RequestPermissionRequest, String) {
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{
                "type": "text",
                "text": r#"#tool:AskUserQuestion {"question":"Pick a color","options":["red","blue"]}"#
            }]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut early_agent_text = String::new();
    let (permission_request_id, permission_request) = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for ACP AskUser session/request_permission"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            let request: RequestPermissionRequest = serde_json::from_value(msg["params"].clone())
                .expect("valid ACP requestPermission request");
            assert!(
                !msg["id"].is_null(),
                "ACP request must carry a JSON-RPC id: {msg:?}"
            );
            assert_eq!(request.session_id.to_string(), session_id);
            assert_eq!(
                request
                    .options
                    .iter()
                    .map(|option| (option.option_id.to_string(), option.name.clone()))
                    .collect::<Vec<_>>(),
                vec![
                    ("ask_user_option_0".to_string(), "red".to_string()),
                    ("ask_user_option_1".to_string(), "blue".to_string()),
                ]
            );
            assert_eq!(
                request.tool_call.fields.title.as_deref(),
                Some("Pick a color")
            );
            assert_eq!(
                request.tool_call.fields.raw_input,
                Some(json!({"question":"Pick a color","options":["red","blue"]}))
            );
            break (msg["id"].clone(), request);
        }

        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            let update = serde_json::to_value(notification.update).expect("serialize update");
            collect_text_fields(&update, &mut early_agent_text);
        }

        assert_ne!(
            msg.get("id").and_then(Value::as_i64),
            Some(3),
            "session/prompt completed before AskUser request: {msg:?}; agent text: {early_agent_text:?}"
        );
    };

    let response = RequestPermissionResponse::new(outcome);
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": permission_request_id,
        "result": serde_json::to_value(response).expect("permission response JSON")
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut agent_text = String::new();
    let prompt_response = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for AskUser session/prompt response"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            let update = serde_json::to_value(notification.update).expect("serialize update");
            collect_text_fields(&update, &mut agent_text);
        }

        if msg.get("id").and_then(Value::as_i64) == Some(3) {
            break msg;
        }
    };

    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    (parsed, permission_request, agent_text)
}

async fn send_ask_user_prompt(proc: &mut AcpProcess, id: i64, session_id: &str) {
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{
                "type": "text",
                "text": r#"#tool:AskUserQuestion {"question":"Pick a color","options":["red","blue"]}"#
            }]
        }
    }))
    .await;
}

async fn send_permission_response(
    proc: &mut AcpProcess,
    request_id: Value,
    outcome: RequestPermissionOutcome,
) {
    let response = RequestPermissionResponse::new(outcome);
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": serde_json::to_value(response).expect("permission response JSON")
    }))
    .await;
}

async fn wait_for_permission_request(
    proc: &mut AcpProcess,
    prompt_response_id: i64,
    session_id: &str,
    expected_option_id: &str,
    description: &str,
) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for ACP {description} session/request_permission"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            let request: RequestPermissionRequest = serde_json::from_value(msg["params"].clone())
                .expect("valid ACP requestPermission request");
            assert_eq!(request.session_id.to_string(), session_id);
            assert!(
                request
                    .options
                    .iter()
                    .any(|option| option.option_id.to_string() == expected_option_id),
                "expected option {expected_option_id}: {:?}",
                request.options
            );
            return msg["id"].clone();
        }

        assert_ne!(
            msg.get("id").and_then(Value::as_i64),
            Some(prompt_response_id),
            "session/prompt completed before {description} request: {msg:?}"
        );
    }
}

async fn wait_for_prompt_response_and_text(
    proc: &mut AcpProcess,
    prompt_response_id: i64,
    description: &str,
) -> (PromptResponse, String) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut agent_text = String::new();
    let prompt_response = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {description} session/prompt response"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            let update = serde_json::to_value(notification.update).expect("serialize update");
            collect_text_fields(&update, &mut agent_text);
        }

        if msg.get("id").and_then(Value::as_i64) == Some(prompt_response_id) {
            break msg;
        }
    };

    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    (parsed, agent_text)
}

fn prompt_stop_reason(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response:?}");
    let parsed: PromptResponse =
        serde_json::from_value(response["result"].clone()).expect("valid ACP prompt response");
    serde_json::to_value(parsed.stop_reason).expect("stop reason JSON")
}

async fn assert_no_response_for(proc: &mut AcpProcess, id: i64, duration: Duration) {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(msg) = proc.recv_timeout(remaining).await else {
            return;
        };
        assert_ne!(
            msg.get("id").and_then(Value::as_i64),
            Some(id),
            "unexpected response id {id} before test action: {msg:?}"
        );
    }
}

fn collect_text_fields(value: &Value, text: &mut String) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "text"
                    && let Some(chunk) = value.as_str()
                {
                    text.push_str(chunk);
                }
                collect_text_fields(value, text);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_text_fields(item, text);
            }
        }
        _ => {}
    }
}

fn fake_stdio_server_binary() -> &'static Path {
    static SERVER_BINARY: OnceLock<PathBuf> = OnceLock::new();
    SERVER_BINARY
        .get_or_init(compile_fake_stdio_server)
        .as_path()
}

fn compile_fake_stdio_server() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orbcode-acp-fake-stdio-server-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create fake MCP server temp dir");

    let source = dir.join("fake_stdio_server.rs");
    let binary = dir.join(if cfg!(windows) {
        "fake_stdio_server.exe"
    } else {
        "fake_stdio_server"
    });
    std::fs::write(&source, FAKE_STDIO_SERVER_SOURCE).expect("write fake MCP server source");

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = StdCommand::new(rustc)
        .arg("--edition=2021")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("compile fake MCP stdio server");

    assert!(
        output.status.success(),
        "compile fake MCP stdio server\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    binary
}

const FAKE_STDIO_SERVER_SOURCE: &str = r##"
use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }

        let Some(response) = fake_stdio_response(&line) else {
            continue;
        };
        writeln!(stdout, "{response}").expect("write fake MCP response");
        stdout.flush().expect("flush fake MCP response");
    }
}

fn fake_stdio_response(request: &str) -> Option<String> {
    let id = extract_id(request);

    if request.contains(r#""method":"notifications/initialized""#) {
        return None;
    }

    if request.contains(r#""method":"initialize""#) {
        return Some(success_response(
            &id,
            r#"{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"orbcode-acp-fake-stdio","version":"0.1.0"}}"#,
        ));
    }

    if request.contains(r#""method":"tools/list""#) {
        return Some(success_response(
            &id,
            r#"{"tools":[{"name":"echo","description":"Echo test input.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}"#,
        ));
    }

    if request.contains(r#""method":"tools/call""#) && request.contains(r#""name":"echo""#) {
        let text = extract_text_argument(request);
        let echoed = escape_json_string(&format!("echo: {text}"));
        return Some(success_response(
            &id,
            &format!(r#"{{"content":[{{"type":"text","text":"{echoed}"}}],"isError":false}}"#),
        ));
    }

    Some(error_response(&id, -32602, "unknown fake MCP request"))
}

fn extract_id(request: &str) -> String {
    let Some(index) = request.find(r#""id":"#) else {
        return "null".to_string();
    };
    let rest = request[index + r#""id":"#.len()..].trim_start();
    let id: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect();
    if id.is_empty() { "null".to_string() } else { id }
}

fn extract_text_argument(request: &str) -> String {
    let pattern = "\"text\":\"";
    let Some(index) = request.find(pattern) else {
        return String::new();
    };
    let rest = &request[index + pattern.len()..];
    read_json_string_value(rest).unwrap_or_default()
}

fn read_json_string_value(input: &str) -> Option<String> {
    let mut value = String::new();
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                '/' => value.push('/'),
                'b' => value.push('\u{0008}'),
                'f' => value.push('\u{000c}'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            },
            other => value.push(other),
        }
    }

    None
}

fn success_response(id: &str, result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#)
}

fn error_response(id: &str, code: i64, message: &str) -> String {
    let message = escape_json_string(message);
    format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":"{message}"}}}}"#)
}

fn escape_json_string(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '"' => output.push_str(r#"\""#),
            '\\' => output.push_str(r#"\\"#),
            '\n' => output.push_str(r#"\n"#),
            '\r' => output.push_str(r#"\r"#),
            '\t' => output.push_str(r#"\t"#),
            other => output.push(other),
        }
    }
    output
}
"##;

struct FakeStreamableHttpMcpServer {
    endpoint: String,
    addr: SocketAddr,
    saw_tool_call: Arc<AtomicBool>,
    saw_delete: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
    handle: Option<JoinHandle<()>>,
}

impl FakeStreamableHttpMcpServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake HTTP MCP server");
        let addr = listener.local_addr().expect("fake HTTP MCP local addr");
        let endpoint = format!("http://{addr}/mcp");
        let saw_tool_call = Arc::new(AtomicBool::new(false));
        let saw_delete = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(Vec::new()));

        let thread_saw_tool_call = Arc::clone(&saw_tool_call);
        let thread_saw_delete = Arc::clone(&saw_delete);
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_errors = Arc::clone(&errors);
        let handle = std::thread::spawn(move || {
            run_fake_streamable_http_mcp_server(
                listener,
                thread_saw_tool_call,
                thread_saw_delete,
                thread_shutdown,
                thread_errors,
            );
        });

        Self {
            endpoint,
            addr,
            saw_tool_call,
            saw_delete,
            shutdown,
            errors,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn finish(mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.saw_delete.load(Ordering::SeqCst) && Instant::now() < deadline {
            let has_errors = !self.errors.lock().expect("fake HTTP MCP errors").is_empty();
            if has_errors {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("join fake HTTP MCP server");
        }

        let errors = self.errors.lock().expect("fake HTTP MCP errors");
        assert!(errors.is_empty(), "fake HTTP MCP errors: {errors:?}");
        assert!(
            self.saw_tool_call.load(Ordering::SeqCst),
            "expected Streamable HTTP tools/call"
        );
        assert!(
            self.saw_delete.load(Ordering::SeqCst),
            "expected Streamable HTTP DELETE shutdown"
        );
    }
}

fn run_fake_streamable_http_mcp_server(
    listener: TcpListener,
    saw_tool_call: Arc<AtomicBool>,
    saw_delete: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
) {
    listener
        .set_nonblocking(true)
        .expect("set fake HTTP MCP listener nonblocking");
    let deadline = Instant::now() + Duration::from_secs(30);

    while !shutdown.load(Ordering::SeqCst) && Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = stream.set_nonblocking(false) {
                    errors
                        .lock()
                        .expect("fake HTTP MCP errors")
                        .push(format!("set accepted stream blocking failed: {error}"));
                    continue;
                }
                if let Err(error) =
                    handle_fake_streamable_http_mcp_request(stream, &saw_tool_call, &saw_delete)
                {
                    errors.lock().expect("fake HTTP MCP errors").push(error);
                }
                if saw_delete.load(Ordering::SeqCst) {
                    shutdown.store(true, Ordering::SeqCst);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                errors
                    .lock()
                    .expect("fake HTTP MCP errors")
                    .push(format!("accept failed: {error}"));
                break;
            }
        }
    }
}

fn handle_fake_streamable_http_mcp_request(
    stream: TcpStream,
    saw_tool_call: &AtomicBool,
    saw_delete: &AtomicBool,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let mut reader = StdBufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|error| error.to_string())?;
    if request_line.trim().is_empty() {
        return Ok(());
    }
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if name == "content-length" {
            content_length = value.parse().unwrap_or(0);
        }
        headers.push((name, value));
    }

    let mut body = vec![0_u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    let body = String::from_utf8(body).map_err(|error| error.to_string())?;
    let mut stream = stream;

    if path != "/mcp" {
        write_http_response(
            &mut stream,
            "404 Not Found",
            &[("Content-Type", "text/plain")],
            "not found",
        )?;
        return Ok(());
    }

    if method == "DELETE" {
        require_header(&headers, "x-test", "acp-http")?;
        require_header(&headers, "mcp-session-id", "acp-http-session")?;
        require_header(&headers, "mcp-protocol-version", "2024-11-05")?;
        saw_delete.store(true, Ordering::SeqCst);
        write_http_response(&mut stream, "204 No Content", &[], "")?;
        return Ok(());
    }

    if method != "POST" {
        write_http_response(
            &mut stream,
            "405 Method Not Allowed",
            &[("Content-Type", "text/plain")],
            "method not allowed",
        )?;
        return Ok(());
    }

    require_header(&headers, "x-test", "acp-http")?;
    require_header(&headers, "content-type", "application/json")?;
    let accept = header_value(&headers, "accept").unwrap_or_default();
    if !accept.contains("application/json") || !accept.contains("text/event-stream") {
        return Err(format!("unexpected Accept header: {accept:?}"));
    }

    let request: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let id = request
        .get("id")
        .cloned()
        .ok_or_else(|| format!("missing JSON-RPC id in {request:?}"))?;
    let rpc_method = request.get("method").and_then(Value::as_str).unwrap_or("");

    match rpc_method {
        "initialize" => {
            if header_value(&headers, "mcp-session-id").is_some() {
                return Err("initialize must not send an MCP session id".to_string());
            }
            let body = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "orbcode-acp-fake-http", "version": "0.1.0" }
                }
            })
            .to_string();
            write_http_response(
                &mut stream,
                "200 OK",
                &[
                    ("Content-Type", "application/json"),
                    ("Mcp-Session-Id", "acp-http-session"),
                ],
                &body,
            )?;
        }
        "tools/list" => {
            require_header(&headers, "mcp-session-id", "acp-http-session")?;
            require_header(&headers, "mcp-protocol-version", "2024-11-05")?;
            let body = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echo test input over HTTP.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "text": { "type": "string" } }
                        }
                    }]
                }
            })
            .to_string();
            write_http_response(
                &mut stream,
                "200 OK",
                &[("Content-Type", "application/json")],
                &body,
            )?;
        }
        "tools/call" => {
            require_header(&headers, "mcp-session-id", "acp-http-session")?;
            require_header(&headers, "mcp-protocol-version", "2024-11-05")?;
            let params = request.get("params").unwrap_or(&Value::Null);
            if params.get("name").and_then(Value::as_str) != Some("echo") {
                return Err(format!("unexpected tool call params: {params:?}"));
            }
            let text = params
                .get("arguments")
                .and_then(|value| value.get("text"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("missing text argument: {params:?}"))?;
            if text != "from acp http" {
                return Err(format!("unexpected text argument: {text:?}"));
            }
            saw_tool_call.store(true, Ordering::SeqCst);
            let body = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": format!("http echo: {text}") }],
                    "isError": false
                }
            })
            .to_string();
            write_http_response(
                &mut stream,
                "200 OK",
                &[("Content-Type", "application/json; charset=utf-8")],
                &body,
            )?;
        }
        other => {
            return Err(format!("unexpected JSON-RPC method: {other}"));
        }
    }

    Ok(())
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header_name, _)| header_name == name)
        .map(|(_, value)| value.clone())
}

fn require_header(headers: &[(String, String)], name: &str, expected: &str) -> Result<(), String> {
    let actual = header_value(headers, name)
        .ok_or_else(|| format!("missing required header {name}; headers={headers:?}"))?;
    if actual != expected {
        return Err(format!(
            "unexpected header {name}: got {actual:?}, expected {expected:?}"
        ));
    }
    Ok(())
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<(), String> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .map_err(|error| error.to_string())?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").map_err(|error| error.to_string())?;
    }
    write!(stream, "\r\n{body}").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

#[tokio::test]
async fn acp_sdk_client_conformance_harness_smoke() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let permission_requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let permission_requests_for_handler = Arc::clone(&permission_requests);
    let agent = sdk_acp_agent(BASH_TOOL_MOCK_BASE_URL, home.path());

    let run = Client
        .builder()
        .name("orbcode-sdk-client-conformance")
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                permission_requests_for_handler.lock().await.push(request);
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        "reject_once",
                    )),
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |connection| {
            let initialized = connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_info(Implementation::new("orbcode-sdk-test", "0.0.0")),
                )
                .block_task()
                .await?;
            assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
            assert_eq!(
                initialized.agent_info.as_ref().expect("agent info").name,
                "orbcode"
            );
            assert!(initialized.auth_methods.is_empty());
            assert!(initialized.agent_capabilities.load_session);
            assert!(initialized.agent_capabilities.mcp_capabilities.http);
            assert!(!initialized.agent_capabilities.mcp_capabilities.sse);
            assert!(initialized.agent_capabilities.auth.logout.is_none());
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .additional_directories
                    .is_some()
            );
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .close
                    .is_some()
            );
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .list
                    .is_some()
            );
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .resume
                    .is_some()
            );
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .delete
                    .is_some()
            );
            assert!(!initialized.agent_capabilities.prompt_capabilities.image);
            assert!(!initialized.agent_capabilities.prompt_capabilities.audio);
            assert!(
                initialized
                    .agent_capabilities
                    .prompt_capabilities
                    .embedded_context
            );

            let mut session = connection
                .build_session(cwd.path())
                .block_task()
                .start_session()
                .await?;
            let session_id = session.session_id().clone();

            session.send_prompt("run echo hi")?;
            let mut saw_session_update = false;
            let stop_reason = loop {
                let message = tokio::time::timeout(Duration::from_secs(20), session.read_update())
                    .await
                    .expect("timed out waiting for SDK session update")
                    .expect("read SDK session update");

                match message {
                    SessionMessage::SessionMessage(Dispatch::Notification(untyped)) => {
                        assert_eq!(untyped.method, "session/update");
                        let _notification: SessionNotification =
                            serde_json::from_value(untyped.params)
                                .expect("valid SDK session/update notification");
                        saw_session_update = true;
                    }
                    SessionMessage::StopReason(reason) => break reason,
                    other => panic!("unexpected SDK session message: {other:?}"),
                }
            };

            assert_eq!(stop_reason, StopReason::EndTurn);
            assert!(saw_session_update, "SDK client should parse session/update");

            let seen_permission_requests = permission_requests.lock().await;
            assert_eq!(seen_permission_requests.len(), 1);
            assert_eq!(
                seen_permission_requests[0].session_id.to_string(),
                session_id.to_string()
            );
            assert!(
                seen_permission_requests[0]
                    .options
                    .iter()
                    .any(|option| option.option_id.to_string() == "reject_once"),
                "SDK request_permission should include reject_once: {:?}",
                seen_permission_requests[0].options
            );
            drop(seen_permission_requests);

            let connection = session.connection();
            drop(session);
            connection
                .send_request(CloseSessionRequest::new(session_id))
                .block_task()
                .await?;

            Ok(())
        });

    tokio::time::timeout(Duration::from_secs(45), run)
        .await
        .expect("SDK ACP conformance harness timed out")
        .expect("SDK ACP conformance harness failed");
}

#[tokio::test]
async fn acp_sdk_client_session_list_conformance_smoke() {
    let home = tempfile::tempdir().expect("home");
    let cwd = std::env::current_dir()
        .expect("current dir")
        .canonicalize()
        .expect("canonical current dir");
    let session_id = "sdk-list-session";
    seed_acp_session_transcript(home.path(), &cwd, session_id, "remember this").await;
    let agent = sdk_acp_agent("mock://anthropic?scenario=success", home.path());

    let run = Client
        .builder()
        .name("orbcode-sdk-client-session-list")
        .connect_with(agent, async move |connection| {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .list
                    .is_some(),
                "session/list should be advertised"
            );
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .resume
                    .is_some(),
                "session/resume should be advertised"
            );

            let listed = connection
                .send_request(ListSessionsRequest::new().cwd(cwd.clone()))
                .block_task()
                .await?;
            let session = listed
                .sessions
                .iter()
                .find(|session| session.session_id.to_string() == session_id)
                .expect("seeded session listed");
            assert_eq!(session.cwd, cwd);
            assert_eq!(session.title.as_deref(), Some("remember this"));
            assert!(listed.next_cursor.is_none());

            Ok(())
        });

    tokio::time::timeout(Duration::from_secs(45), run)
        .await
        .expect("SDK ACP session/list harness timed out")
        .expect("SDK ACP session/list harness failed");
}

#[tokio::test]
async fn acp_sdk_client_session_load_conformance_smoke() {
    let home = tempfile::tempdir().expect("home");
    let cwd = std::env::current_dir()
        .expect("current dir")
        .canonicalize()
        .expect("canonical current dir");
    let session_id = "sdk-load-session";
    seed_acp_session_transcript_lines(
        home.path(),
        &cwd,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "sdk-load-user",
                "timestamp": "2026-06-01T00:00:00.000Z",
                "message": { "role": "user", "content": "remember this" },
                "cwd": cwd.display().to_string(),
                "sessionId": session_id,
            }),
            json!({
                "type": "assistant",
                "uuid": "sdk-load-assistant",
                "timestamp": "2026-06-01T00:00:01.000Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "loaded answer" }] },
                "cwd": cwd.display().to_string(),
                "sessionId": session_id,
            }),
        ],
    )
    .await;
    let updates = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let updates_for_handler = Arc::clone(&updates);
    let agent = sdk_acp_agent("mock://anthropic?scenario=success", home.path());

    let run = Client
        .builder()
        .name("orbcode-sdk-client-session-load")
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                updates_for_handler.lock().await.push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, async move |connection| {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert!(
                initialized.agent_capabilities.load_session,
                "session/load should be advertised"
            );

            let loaded = connection
                .send_request(LoadSessionRequest::new(session_id, cwd.clone()))
                .block_task()
                .await?;
            assert_eq!(
                loaded
                    .modes
                    .as_ref()
                    .expect("load modes")
                    .current_mode_id
                    .to_string(),
                "default"
            );
            assert!(loaded.config_options.as_ref().is_some_and(|options| {
                options
                    .iter()
                    .any(|option| option.id.to_string() == "model")
                    && options
                        .iter()
                        .any(|option| option.id.to_string() == "thought_level")
            }));

            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                if updates.lock().await.len() >= 2 {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "timed out waiting for SDK load replay notifications"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            let seen = updates.lock().await;
            assert_eq!(seen[0].session_id.to_string(), session_id);
            assert!(matches!(seen[0].update, SessionUpdate::UserMessageChunk(_)));
            assert!(matches!(
                seen[1].update,
                SessionUpdate::AgentMessageChunk(_)
            ));

            Ok(())
        });

    tokio::time::timeout(Duration::from_secs(45), run)
        .await
        .expect("SDK ACP session/load harness timed out")
        .expect("SDK ACP session/load harness failed");
}

#[tokio::test]
async fn acp_sdk_client_session_resume_conformance_smoke() {
    let home = tempfile::tempdir().expect("home");
    let cwd = std::env::current_dir()
        .expect("current dir")
        .canonicalize()
        .expect("canonical current dir");
    let session_id = "sdk-resume-session";
    seed_acp_session_transcript_lines(
        home.path(),
        &cwd,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "sdk-resume-user",
                "timestamp": "2026-06-01T00:00:00.000Z",
                "message": { "role": "user", "content": "remember this" },
                "cwd": cwd.display().to_string(),
                "sessionId": session_id,
            }),
            json!({
                "type": "assistant",
                "uuid": "sdk-resume-assistant",
                "timestamp": "2026-06-01T00:00:01.000Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "remembered" }] },
                "cwd": cwd.display().to_string(),
                "sessionId": session_id,
            }),
        ],
    )
    .await;
    let updates = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let updates_for_handler = Arc::clone(&updates);
    let agent = sdk_acp_agent("mock://anthropic?scenario=success", home.path());

    let run = Client
        .builder()
        .name("orbcode-sdk-client-session-resume")
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                updates_for_handler.lock().await.push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, async move |connection| {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .resume
                    .is_some(),
                "session/resume should be advertised"
            );

            let resumed = connection
                .send_request(ResumeSessionRequest::new(session_id, cwd.clone()))
                .block_task()
                .await?;
            assert_eq!(
                resumed
                    .modes
                    .as_ref()
                    .expect("resume modes")
                    .current_mode_id
                    .to_string(),
                "default"
            );
            assert!(resumed.config_options.as_ref().is_some_and(|options| {
                options
                    .iter()
                    .any(|option| option.id.to_string() == "model")
                    && options
                        .iter()
                        .any(|option| option.id.to_string() == "thought_level")
            }));
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(
                updates.lock().await.is_empty(),
                "session/resume must not replay history updates"
            );

            let prompt = PromptRequest::new(
                session_id,
                vec![ContentBlock::from("continue after resume".to_string())],
            );
            let response = connection.send_request(prompt).block_task().await?;
            assert_eq!(response.stop_reason, StopReason::EndTurn);

            Ok(())
        });

    tokio::time::timeout(Duration::from_secs(45), run)
        .await
        .expect("SDK ACP session/resume harness timed out")
        .expect("SDK ACP session/resume harness failed");
}

#[tokio::test]
async fn acp_sdk_client_session_delete_conformance_smoke() {
    let home = tempfile::tempdir().expect("home");
    let cwd = std::env::current_dir()
        .expect("current dir")
        .canonicalize()
        .expect("canonical current dir");
    let session_id = "sdk-delete-session";
    seed_acp_session_transcript(home.path(), &cwd, session_id, "delete this").await;
    let agent = sdk_acp_agent("mock://anthropic?scenario=success", home.path());

    let run = Client
        .builder()
        .name("orbcode-sdk-client-session-delete")
        .connect_with(agent, async move |connection| {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert!(
                initialized
                    .agent_capabilities
                    .session_capabilities
                    .delete
                    .is_some(),
                "session/delete should be advertised"
            );

            let before = connection
                .send_request(ListSessionsRequest::new().cwd(cwd.clone()))
                .block_task()
                .await?;
            assert!(
                before
                    .sessions
                    .iter()
                    .any(|session| session.session_id.to_string() == session_id),
                "seeded session should be listed before delete"
            );

            connection
                .send_request(DeleteSessionRequest::new(session_id))
                .block_task()
                .await?;

            let after = connection
                .send_request(ListSessionsRequest::new().cwd(cwd.clone()))
                .block_task()
                .await?;
            assert!(
                !after
                    .sessions
                    .iter()
                    .any(|session| session.session_id.to_string() == session_id),
                "deleted session must not be listed"
            );

            Ok(())
        });

    tokio::time::timeout(Duration::from_secs(45), run)
        .await
        .expect("SDK ACP session/delete harness timed out")
        .expect("SDK ACP session/delete harness failed");
}

#[tokio::test]
async fn acp_sdk_client_cancel_conformance_smoke() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let agent = sdk_acp_agent(HANG_MOCK_BASE_URL, home.path());

    let run = Client
        .builder()
        .name("orbcode-sdk-client-cancel")
        .connect_with(agent, async move |connection| {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let mut session = connection
                .build_session(cwd.path())
                .block_task()
                .start_session()
                .await?;
            let session_id = session.session_id().clone();
            let connection = session.connection();

            session.send_prompt("hang until cancelled")?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            connection.send_notification(CancelNotification::new(session_id.clone()))?;

            let stop_reason = loop {
                let message = tokio::time::timeout(Duration::from_secs(20), session.read_update())
                    .await
                    .expect("timed out waiting for SDK cancel response")
                    .expect("read SDK cancel response");
                match message {
                    SessionMessage::StopReason(reason) => break reason,
                    SessionMessage::SessionMessage(Dispatch::Notification(untyped)) => {
                        assert_eq!(untyped.method, "session/update");
                    }
                    other => panic!("unexpected SDK cancel message: {other:?}"),
                }
            };

            assert_eq!(stop_reason, StopReason::Cancelled);
            drop(session);
            connection
                .send_request(CloseSessionRequest::new(session_id))
                .block_task()
                .await?;

            Ok(())
        });

    tokio::time::timeout(Duration::from_secs(45), run)
        .await
        .expect("SDK ACP cancel harness timed out")
        .expect("SDK ACP cancel harness failed");
}

#[tokio::test]
async fn acp_initialize_uses_official_sdk_shape() {
    let mut proc = AcpProcess::spawn().await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;

    let response = proc.recv_response(1).await;
    assert!(response.get("error").is_none(), "{response:?}");
    assert!(response["result"].get("protocolVersion").is_some());
    assert!(response["result"].get("protocol_version").is_none());
    assert!(response["result"].get("agentCapabilities").is_some());
    assert!(
        response["result"]["agentCapabilities"]["sessionCapabilities"]["close"].is_object(),
        "{response:?}"
    );
    assert!(
        response["result"]["agentCapabilities"]["sessionCapabilities"]["additionalDirectories"]
            .is_object(),
        "{response:?}"
    );
    assert!(
        response["result"]["agentCapabilities"]["sessionCapabilities"]["list"].is_object(),
        "{response:?}"
    );
    assert!(response["result"].get("agentInfo").is_some());
    assert_eq!(response["result"]["authMethods"], json!([]));

    let raw_capabilities = &response["result"]["agentCapabilities"];
    assert!(raw_capabilities["auth"].is_object());
    assert!(raw_capabilities["auth"].get("logout").is_none());
    assert_eq!(raw_capabilities["loadSession"], json!(true));
    assert_eq!(raw_capabilities["mcpCapabilities"]["http"], json!(true));
    assert_eq!(raw_capabilities["mcpCapabilities"]["sse"], json!(false));
    assert!(raw_capabilities["mcpCapabilities"].get("acp").is_none());
    assert!(raw_capabilities["sessionCapabilities"]["resume"].is_object());
    assert!(raw_capabilities["sessionCapabilities"]["delete"].is_object());
    assert!(
        raw_capabilities["sessionCapabilities"]
            .get("fork")
            .is_none()
    );
    assert!(raw_capabilities.get("elicitation").is_none());
    assert!(raw_capabilities.get("providers").is_none());
    assert!(raw_capabilities.get("nes").is_none());
    assert!(raw_capabilities.get("positionEncoding").is_none());
    assert_eq!(raw_capabilities["promptCapabilities"]["image"], false);
    assert_eq!(raw_capabilities["promptCapabilities"]["audio"], false);
    assert_eq!(
        raw_capabilities["promptCapabilities"]["embeddedContext"],
        true
    );

    let parsed: InitializeResponse =
        serde_json::from_value(response["result"].clone()).expect("valid ACP initialize response");
    assert_eq!(parsed.agent_info.expect("agent info").name, "orbcode");
    assert!(parsed.auth_methods.is_empty());
    assert!(parsed.agent_capabilities.auth.logout.is_none());
    assert!(
        parsed
            .agent_capabilities
            .session_capabilities
            .list
            .is_some()
    );
    assert!(
        parsed
            .agent_capabilities
            .session_capabilities
            .resume
            .is_some()
    );
    assert!(
        parsed
            .agent_capabilities
            .session_capabilities
            .delete
            .is_some()
    );
    assert!(parsed.agent_capabilities.load_session);

    proc.close().await;
}

#[tokio::test]
async fn acp_session_list_returns_persisted_sessions() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-list-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript(proc.home(), &session_cwd, session_id, "list me").await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
    let capabilities = &init["result"]["agentCapabilities"];
    assert!(capabilities["sessionCapabilities"]["list"].is_object());
    assert!(capabilities["sessionCapabilities"]["resume"].is_object());
    assert!(capabilities["sessionCapabilities"]["delete"].is_object());
    assert_eq!(capabilities["loadSession"], json!(true));

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/list",
        "params": {
            "cwd": session_cwd
        }
    }))
    .await;
    let response = proc.recv_response(2).await;
    assert!(response.get("error").is_none(), "{response:?}");
    assert!(response["result"].get("nextCursor").is_none());
    let listed: ListSessionsResponse =
        serde_json::from_value(response["result"].clone()).expect("valid session/list response");
    let session = listed
        .sessions
        .iter()
        .find(|session| session.session_id.to_string() == session_id)
        .expect("seeded session listed");
    assert_eq!(session.cwd, session_cwd);
    assert_eq!(session.title.as_deref(), Some("list me"));
    assert!(listed.next_cursor.is_none());

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/list",
        "params": {}
    }))
    .await;
    let response = proc.recv_response(3).await;
    assert!(response.get("error").is_none(), "{response:?}");
    let listed_without_cwd: ListSessionsResponse =
        serde_json::from_value(response["result"].clone()).expect("valid session/list response");
    assert!(
        listed_without_cwd
            .sessions
            .iter()
            .any(|session| session.session_id.to_string() == session_id),
        "launch-cwd session should be listed without an explicit cwd: {listed_without_cwd:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_session_list_rejects_non_launch_cwd_filter() {
    let mut proc = AcpProcess::spawn().await;
    let other_cwd = tempfile::tempdir().expect("other cwd");
    let other_cwd = other_cwd
        .path()
        .canonicalize()
        .expect("canonical other cwd");
    seed_acp_session_transcript(proc.home(), &other_cwd, "other-cwd-session", "not listed").await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/list",
        "params": {
            "cwd": other_cwd
        }
    }))
    .await;
    let response = proc.recv_response(2).await;
    assert!(response.get("error").is_some(), "{response:?}");
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("must match the orbcode acp launch cwd")),
        "{response:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_load_replays_history_before_response() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-load-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript_lines(
        proc.home(),
        &session_cwd,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "raw-load-user",
                "timestamp": "2026-06-01T00:00:00.000Z",
                "message": { "role": "user", "content": "load me" },
                "cwd": session_cwd.display().to_string(),
                "sessionId": session_id,
            }),
            json!({
                "type": "assistant",
                "uuid": "raw-load-assistant",
                "timestamp": "2026-06-01T00:00:01.000Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "loaded answer" }] },
                "cwd": session_cwd.display().to_string(),
                "sessionId": session_id,
            }),
        ],
    )
    .await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
    let capabilities = &init["result"]["agentCapabilities"];
    assert_eq!(capabilities["loadSession"], json!(true));
    assert!(capabilities["sessionCapabilities"]["list"].is_object());
    assert!(capabilities["sessionCapabilities"]["resume"].is_object());

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/load",
        "params": {
            "sessionId": session_id,
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;

    let mut updates = Vec::new();
    let response = loop {
        let msg = proc
            .recv_timeout(Duration::from_secs(15))
            .await
            .expect("process should produce load replay or response");
        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            updates.push(notification);
            continue;
        }
        if msg.get("id").and_then(Value::as_i64) == Some(2) {
            break msg;
        }
    };
    assert!(response.get("error").is_none(), "{response:?}");
    assert_eq!(response["result"]["modes"]["currentModeId"], "default");
    assert!(config_option(&response["result"]["configOptions"], "model").is_some());
    assert!(config_option(&response["result"]["configOptions"], "thought_level").is_some());
    assert_eq!(updates.len(), 2, "expected user and assistant replay");
    assert!(matches!(
        updates[0].update,
        SessionUpdate::UserMessageChunk(_)
    ));
    assert!(matches!(
        updates[1].update,
        SessionUpdate::AgentMessageChunk(_)
    ));

    proc.close().await;
}

#[tokio::test]
async fn acp_load_rejects_blocked_transcript_without_partial_replay() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-load-blocked-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript_lines(
        proc.home(),
        &session_cwd,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "raw-load-blocked-user",
                "timestamp": "2026-06-01T00:00:00.000Z",
                "message": { "role": "user", "content": "load me" },
                "cwd": session_cwd.display().to_string(),
                "sessionId": session_id,
            }),
            json!({
                "type": "system",
                "subtype": "api_error",
                "uuid": "raw-load-blocked-system",
                "timestamp": "2026-06-01T00:00:01.000Z",
                "content": "provider failed",
                "cwd": session_cwd.display().to_string(),
                "sessionId": session_id,
            }),
        ],
    )
    .await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/load",
        "params": {
            "sessionId": session_id,
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;

    let response = proc.recv_response(2).await;
    assert!(response.get("error").is_some(), "{response:?}");
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("system/API-error provenance")),
        "{response:?}"
    );
    assert!(
        proc.recv_timeout(Duration::from_millis(200))
            .await
            .is_none(),
        "blocked load must not send partial replay updates"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_resume_does_not_replay_history_and_accepts_prompt() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-resume-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript_lines(
        proc.home(),
        &session_cwd,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "raw-resume-user",
                "timestamp": "2026-06-01T00:00:00.000Z",
                "message": { "role": "user", "content": "remember me" },
                "cwd": session_cwd.display().to_string(),
                "sessionId": session_id,
            }),
            json!({
                "type": "assistant",
                "uuid": "raw-resume-assistant",
                "timestamp": "2026-06-01T00:00:01.000Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "remembered" }] },
                "cwd": session_cwd.display().to_string(),
                "sessionId": session_id,
            }),
        ],
    )
    .await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
    let capabilities = &init["result"]["agentCapabilities"];
    assert_eq!(capabilities["loadSession"], json!(true));
    assert!(capabilities["sessionCapabilities"]["resume"].is_object());

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/resume",
        "params": {
            "sessionId": session_id,
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;

    let mut resume_updates = Vec::new();
    let response = loop {
        let msg = proc
            .recv_timeout(Duration::from_secs(15))
            .await
            .expect("process should produce resume response");
        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            resume_updates.push(msg);
            continue;
        }
        if msg.get("id").and_then(Value::as_i64) == Some(2) {
            break msg;
        }
    };
    assert!(response.get("error").is_none(), "{response:?}");
    assert_eq!(response["result"]["modes"]["currentModeId"], "default");
    assert!(config_option(&response["result"]["configOptions"], "model").is_some());
    assert!(config_option(&response["result"]["configOptions"], "thought_level").is_some());
    assert!(
        resume_updates.is_empty(),
        "session/resume must not replay persisted history: {resume_updates:?}"
    );
    assert!(
        proc.recv_timeout(Duration::from_millis(200))
            .await
            .is_none(),
        "session/resume must not send delayed replay updates"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "continue"}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut saw_agent_update = false;
    let prompt_response = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for resumed session/prompt response"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            if serde_json::to_value(notification.update)
                .expect("serialize update")
                .get("sessionUpdate")
                .and_then(Value::as_str)
                == Some("agent_message_chunk")
            {
                saw_agent_update = true;
            }
        }

        if msg.get("id").and_then(Value::as_i64) == Some(3) {
            break msg;
        }
    };
    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    assert!(
        saw_agent_update,
        "resumed session/prompt should stream an ACP agent_message_chunk"
    );
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("end_turn")
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_resume_rejects_missing_session() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/resume",
        "params": {
            "sessionId": "missing-resume-session",
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;
    let response = proc.recv_response(2).await;
    assert!(response.get("error").is_some(), "{response:?}");
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("session/resume session not found")),
        "{response:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_resume_cancel_cleans_active_prompt() {
    let mut proc = AcpProcess::spawn_with_base_url(HANG_MOCK_BASE_URL).await;
    let session_id = "raw-resume-cancel-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript(proc.home(), &session_cwd, session_id, "resume then cancel").await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/resume",
        "params": {
            "sessionId": session_id,
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;
    let resume = proc.recv_response(2).await;
    assert!(resume.get("error").is_none(), "{resume:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "hang until cancelled"}]
        }
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;
    assert_eq!(
        prompt_stop_reason(&proc.recv_response(3).await),
        json!("cancelled")
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_resume_close_cleans_session() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-resume-close-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript(proc.home(), &session_cwd, session_id, "resume then close").await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/resume",
        "params": {
            "sessionId": session_id,
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;
    let resume = proc.recv_response(2).await;
    assert!(resume.get("error").is_none(), "{resume:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/close",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;
    let close = proc.recv_response(3).await;
    assert!(close.get("error").is_none(), "{close:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "should be closed"}]
        }
    }))
    .await;
    let prompt = proc.recv_response(4).await;
    assert!(
        prompt.get("error").is_some(),
        "closed resumed session must reject later prompt: {prompt:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_resume_stdin_eof_exits_with_active_prompt() {
    let mut proc = AcpProcess::spawn_with_base_url(HANG_MOCK_BASE_URL).await;
    let session_id = "raw-resume-eof-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript(proc.home(), &session_cwd, session_id, "resume then eof").await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/resume",
        "params": {
            "sessionId": session_id,
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;
    let resume = proc.recv_response(2).await;
    assert!(resume.get("error").is_none(), "{resume:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "hang until stdin closes"}]
        }
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    proc.close_expect_exit().await;
}

#[tokio::test]
async fn acp_resume_rejects_cwd_mismatch() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-resume-cwd-mismatch";
    let persisted_cwd = tempfile::tempdir().expect("persisted cwd");
    let persisted_cwd = persisted_cwd
        .path()
        .canonicalize()
        .expect("canonical persisted cwd");
    let requested_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript(proc.home(), &persisted_cwd, session_id, "not here").await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/resume",
        "params": {
            "sessionId": session_id,
            "cwd": requested_cwd,
            "mcpServers": []
        }
    }))
    .await;
    let response = proc.recv_response(2).await;
    assert!(response.get("error").is_some(), "{response:?}");
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("session/resume cwd mismatch")),
        "{response:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_delete_removes_listed_session_and_list_after_delete() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-delete-session";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_acp_session_transcript(proc.home(), &session_cwd, session_id, "delete me").await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
    let capabilities = &init["result"]["agentCapabilities"];
    assert!(capabilities["sessionCapabilities"]["delete"].is_object());

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/list",
        "params": {
            "cwd": session_cwd
        }
    }))
    .await;
    let listed = proc.recv_response(2).await;
    assert!(listed.get("error").is_none(), "{listed:?}");
    let listed: ListSessionsResponse =
        serde_json::from_value(listed["result"].clone()).expect("valid list response");
    assert!(
        listed
            .sessions
            .iter()
            .any(|session| session.session_id.to_string() == session_id),
        "seeded session should be listed before delete"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/delete",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;
    let response = proc.recv_response(3).await;
    assert!(response.get("error").is_none(), "{response:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/list",
        "params": {
            "cwd": session_cwd
        }
    }))
    .await;
    let listed = proc.recv_response(4).await;
    assert!(listed.get("error").is_none(), "{listed:?}");
    let listed: ListSessionsResponse =
        serde_json::from_value(listed["result"].clone()).expect("valid list response");
    assert!(
        !listed
            .sessions
            .iter()
            .any(|session| session.session_id.to_string() == session_id),
        "deleted session must not be listed"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_delete_rejects_active_session() {
    let mut proc = AcpProcess::spawn_with_base_url(HANG_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "hang before delete"}]
        }
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/delete",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;
    let response = proc.recv_response(4).await;
    assert!(response.get("error").is_some(), "{response:?}");
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("cannot delete active session")),
        "{response:?}"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;
    assert_eq!(
        prompt_stop_reason(&proc.recv_response(3).await),
        json!("cancelled")
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_delete_rejects_out_of_scope_session() {
    let mut proc = AcpProcess::spawn().await;
    let other_cwd = tempfile::tempdir().expect("other cwd");
    let other_cwd = other_cwd
        .path()
        .canonicalize()
        .expect("canonical other cwd");
    let session_id = "raw-delete-out-of-scope";
    seed_acp_session_transcript(proc.home(), &other_cwd, session_id, "not visible").await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/delete",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;
    let response = proc.recv_response(2).await;
    assert!(response.get("error").is_some(), "{response:?}");
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("session not found")),
        "{response:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_delete_rejects_corrupt_transcript() {
    let mut proc = AcpProcess::spawn().await;
    let session_id = "raw-delete-corrupt";
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    seed_corrupt_acp_session_transcript(proc.home(), &session_cwd, session_id).await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/delete",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;
    let response = proc.recv_response(2).await;
    assert!(response.get("error").is_some(), "{response:?}");
    assert_eq!(response["error"]["code"], json!(-32602));
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("cannot delete corrupt transcript")),
        "{response:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_fork_and_auth_stay_unadvertised_and_unimplemented() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
    let result = &init["result"];
    assert_eq!(result["authMethods"], json!([]));
    assert!(
        result["agentCapabilities"]["auth"].get("logout").is_none(),
        "{init:?}"
    );
    assert!(
        result["agentCapabilities"]["sessionCapabilities"]
            .get("fork")
            .is_none(),
        "{init:?}"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "authenticate",
        "params": {
            "methodId": "api-key"
        }
    }))
    .await;
    let authenticate = proc.recv_response(2).await;
    assert!(authenticate.get("error").is_some(), "{authenticate:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "logout",
        "params": {}
    }))
    .await;
    let logout = proc.recv_response(3).await;
    assert!(logout.get("error").is_some(), "{logout:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/fork",
        "params": {
            "sessionId": "fork-is-not-advertised",
            "cwd": session_cwd,
            "mcpServers": []
        }
    }))
    .await;
    let fork = proc.recv_response(4).await;
    assert!(fork.get("error").is_some(), "{fork:?}");

    proc.close().await;
}

#[tokio::test]
async fn acp_session_new_accepts_setup_gate_params_without_persisting_mcp() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = tempfile::tempdir().expect("session cwd");
    let extra = tempfile::tempdir().expect("additional dir");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": session_cwd.path(),
            "additionalDirectories": [extra.path()],
            "mcpServers": [{
                "name": "Docs Server",
                "command": "cat",
                "args": [],
                "env": []
            }]
        }
    }))
    .await;
    let new_session = proc.recv_response(2).await;
    assert!(new_session.get("error").is_none(), "{new_session:?}");
    let new_session: NewSessionResponse = serde_json::from_value(new_session["result"].clone())
        .expect("valid ACP session/new response");
    let session_id = new_session.session_id.to_string();
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "ACP session MCP overlay must not persist to servers.json"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/close",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;
    let close = proc.recv_response(3).await;
    assert!(close.get("error").is_none(), "{close:?}");
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "closing ACP session must not persist MCP overlay"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_session_mcp_http_server_is_accepted_without_persisting() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = tempfile::tempdir().expect("session cwd");

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": session_cwd.path(),
            "mcpServers": [{
                "type": "http",
                "name": "Docs HTTP",
                "url": "https://docs.test/mcp",
                "headers": [{
                    "name": "Authorization",
                    "value": "Bearer test"
                }]
            }]
        }
    }))
    .await;
    let new_session = proc.recv_response(2).await;
    assert!(new_session.get("error").is_none(), "{new_session:?}");
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "ACP HTTP MCP overlay must not persist to servers.json"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_session_mcp_http_server_invokes_streamable_http_tool() {
    let server = FakeStreamableHttpMcpServer::start();
    let mut proc =
        AcpProcess::spawn_with_base_url_and_allow_tools(HTTP_MCP_TOOL_MOCK_BASE_URL, true).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_with_fake_http_mcp(&mut proc, 2, server.endpoint()).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "trigger HTTP MCP tool"}]
        }
    }))
    .await;
    let request_id =
        wait_for_permission_request(&mut proc, 3, &session_id, "trust_mcp_server", "MCP trust")
            .await;
    send_permission_response(
        &mut proc,
        request_id,
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("trust_mcp_server")),
    )
    .await;

    let (parsed, agent_text) =
        wait_for_prompt_response_and_text(&mut proc, 3, "HTTP MCP prompt").await;
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("end_turn")
    );
    assert!(
        agent_text.contains("http echo: from acp http"),
        "HTTP MCP tool result should reach the completed prompt text: {agent_text:?}"
    );
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "ACP HTTP MCP overlay must not persist to servers.json"
    );
    assert!(
        !proc.home().join("mcp").join("trust.json").exists(),
        "session HTTP MCP trust approval must not persist to trust.json"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/close",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;
    let close = proc.recv_response(4).await;
    assert!(close.get("error").is_none(), "{close:?}");
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "closing ACP HTTP MCP session must not persist overlay"
    );

    proc.close().await;
    server.finish();
}

#[tokio::test]
async fn acp_session_mcp_http_server_is_accepted_by_load_and_resume_without_persisting() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");
    let load_session_id = "mcp-http-load-session";
    let resume_session_id = "mcp-http-resume-session";
    seed_acp_session_transcript(proc.home(), &session_cwd, load_session_id, "load mcp").await;
    seed_acp_session_transcript(proc.home(), &session_cwd, resume_session_id, "resume mcp").await;

    initialize_acp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/load",
        "params": {
            "sessionId": load_session_id,
            "cwd": session_cwd,
            "mcpServers": [{
                "type": "http",
                "name": "Docs HTTP Load",
                "url": "https://docs.test/load-mcp",
                "headers": [{
                    "name": "Authorization",
                    "value": "Bearer load"
                }]
            }]
        }
    }))
    .await;
    let load_response = loop {
        let msg = proc
            .recv_timeout(Duration::from_secs(15))
            .await
            .expect("process should produce load replay or response");
        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            continue;
        }
        if msg.get("id").and_then(Value::as_i64) == Some(2) {
            break msg;
        }
    };
    assert!(load_response.get("error").is_none(), "{load_response:?}");
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "ACP session/load MCP overlay must not persist to servers.json"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/resume",
        "params": {
            "sessionId": resume_session_id,
            "cwd": session_cwd,
            "mcpServers": [{
                "type": "http",
                "name": "Docs HTTP Resume",
                "url": "http://docs.test/resume-mcp",
                "headers": []
            }]
        }
    }))
    .await;
    let resume_response = proc.recv_response(3).await;
    assert!(
        resume_response.get("error").is_none(),
        "{resume_response:?}"
    );
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "ACP session/resume MCP overlay must not persist to servers.json"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_session_mcp_sse_server_is_rejected() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = tempfile::tempdir().expect("session cwd");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
    assert_eq!(
        init["result"]["agentCapabilities"]["mcpCapabilities"]["sse"],
        json!(false)
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": session_cwd.path(),
            "mcpServers": [{
                "type": "sse",
                "name": "Docs SSE",
                "url": "https://docs.test/events",
                "headers": []
            }]
        }
    }))
    .await;
    let rejected = proc.recv_response(2).await;
    assert!(rejected.get("error").is_some(), "{rejected:?}");

    proc.close().await;
}

#[tokio::test]
async fn acp_session_mcp_sse_server_is_rejected_by_load_and_resume() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");

    initialize_acp(&mut proc).await;

    for (id, method) in [(2, "session/load"), (3, "session/resume")] {
        proc.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {
                "sessionId": format!("sse-{id}"),
                "cwd": session_cwd,
                "mcpServers": [{
                    "type": "sse",
                    "name": "Docs SSE",
                    "url": "https://docs.test/events",
                    "headers": []
                }]
            }
        }))
        .await;
        let rejected = proc.recv_response(id).await;
        assert_eq!(rejected["error"]["code"], json!(-32602), "{rejected:?}");
        assert!(
            rejected["error"]["data"]
                .as_str()
                .is_some_and(|message| message.contains("SSE transport is not supported")),
            "{rejected:?}"
        );
    }

    proc.close().await;
}

#[tokio::test]
async fn acp_session_mcp_acp_transport_is_rejected() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = tempfile::tempdir().expect("session cwd");

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");
    assert!(
        init["result"]["agentCapabilities"]["mcpCapabilities"]
            .get("acp")
            .is_none(),
        "{init:?}"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": session_cwd.path(),
            "mcpServers": [{
                "type": "acp",
                "name": "ACP Tools",
                "id": "acp-tools"
            }]
        }
    }))
    .await;
    let rejected = proc.recv_response(2).await;
    assert!(rejected.get("error").is_some(), "{rejected:?}");

    proc.close().await;
}

#[tokio::test]
async fn acp_session_mcp_acp_transport_is_rejected_by_load_and_resume() {
    let mut proc = AcpProcess::spawn().await;
    let session_cwd = proc.cwd().canonicalize().expect("canonical session cwd");

    initialize_acp(&mut proc).await;

    for (id, method) in [(2, "session/load"), (3, "session/resume")] {
        proc.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {
                "sessionId": format!("acp-{id}"),
                "cwd": session_cwd,
                "mcpServers": [{
                    "type": "acp",
                    "name": "ACP Tools",
                    "id": "acp-tools"
                }]
            }
        }))
        .await;
        let rejected = proc.recv_response(id).await;
        assert!(rejected.get("error").is_some(), "{rejected:?}");
    }

    proc.close().await;
}

#[tokio::test]
async fn acp_ask_user_option_selected_via_request_permission() {
    let mut proc = AcpProcess::spawn_with_base_url(ASK_USER_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    let (parsed, request, agent_text) = prompt_for_ask_user_and_respond(
        &mut proc,
        &session_id,
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("ask_user_option_1")),
    )
    .await;

    assert!(!request.tool_call.tool_call_id.to_string().is_empty());
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("end_turn")
    );
    assert!(
        agent_text.contains("\"Pick a color\" = \"blue\""),
        "AskUser selected option should reach the completed prompt text: {agent_text:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_ask_user_cancelled_via_request_permission() {
    let mut proc = AcpProcess::spawn_with_base_url(ASK_USER_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    let (parsed, _request, agent_text) = prompt_for_ask_user_and_respond(
        &mut proc,
        &session_id,
        RequestPermissionOutcome::Cancelled,
    )
    .await;

    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("cancelled")
    );
    assert!(
        agent_text.is_empty(),
        "cancelled AskUser must not be reported as a successful answer: {agent_text:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_ask_user_free_text_is_cancelled_without_acp_request_permission() {
    let mut proc = AcpProcess::spawn_with_base_url(ASK_USER_FREE_TEXT_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{
                "type": "text",
                "text": r#"#tool:AskUserQuestion {"question":"Say anything"}"#
            }]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut agent_text = String::new();
    let prompt_response = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for free-text AskUser prompt response"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        assert_ne!(
            msg.get("method").and_then(Value::as_str),
            Some("session/request_permission"),
            "option-free AskUser must remain unsupported, not mapped to request_permission: {msg:?}"
        );
        assert_ne!(
            msg.get("method").and_then(Value::as_str),
            Some("elicitation/create"),
            "option-free AskUser must not advertise or use ACP elicitation while disabled: {msg:?}"
        );

        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            let update = serde_json::to_value(notification.update).expect("serialize update");
            collect_text_fields(&update, &mut agent_text);
        }

        if msg.get("id").and_then(Value::as_i64) == Some(3) {
            break msg;
        }
    };

    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("cancelled")
    );
    assert!(
        agent_text.is_empty(),
        "unsupported free-text AskUser must cancel without placeholder success: {agent_text:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_cancel_denies_pending_ask_user_request() {
    let mut proc = AcpProcess::spawn_with_base_url(ASK_USER_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{
                "type": "text",
                "text": r#"#tool:AskUserQuestion {"question":"Pick a color","options":["red","blue"]}"#
            }]
        }
    }))
    .await;

    wait_for_permission_request(&mut proc, 3, &session_id, "ask_user_option_0", "AskUser").await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;

    let stop_reason = prompt_stop_reason(&proc.recv_response(3).await);
    assert!(
        stop_reason == json!("cancelled") || stop_reason == json!("end_turn"),
        "unexpected stop reason after pending AskUser cancel: {stop_reason}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_cancelled_ask_user_does_not_block_later_ask_user() {
    let mut proc = AcpProcess::spawn_with_base_url(ASK_USER_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    send_ask_user_prompt(&mut proc, 3, &session_id).await;
    wait_for_permission_request(&mut proc, 3, &session_id, "ask_user_option_0", "AskUser").await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;

    let stop_reason = prompt_stop_reason(&proc.recv_response(3).await);
    assert!(
        stop_reason == json!("cancelled") || stop_reason == json!("end_turn"),
        "unexpected stop reason after pending AskUser cancel: {stop_reason}"
    );

    let later_session_id = new_session_without_mcp_id(&mut proc, 4).await;
    send_ask_user_prompt(&mut proc, 5, &later_session_id).await;
    let permission_request_id = wait_for_permission_request(
        &mut proc,
        5,
        &later_session_id,
        "ask_user_option_1",
        "AskUser",
    )
    .await;
    send_permission_response(
        &mut proc,
        permission_request_id,
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("ask_user_option_1")),
    )
    .await;

    let (parsed, agent_text) =
        wait_for_prompt_response_and_text(&mut proc, 5, "second AskUser prompt").await;
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("end_turn")
    );
    assert!(
        agent_text.contains("\"Pick a color\" = \"blue\""),
        "later AskUser should complete after prior cancel: {agent_text:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_ask_user_routes_to_second_active_session() {
    let mut proc = AcpProcess::spawn_with_base_url(ASK_USER_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let first_session_id = new_session_without_mcp_id(&mut proc, 2).await;
    let second_session_id = new_session_without_mcp_id(&mut proc, 3).await;

    send_ask_user_prompt(&mut proc, 4, &first_session_id).await;
    wait_for_permission_request(
        &mut proc,
        4,
        &first_session_id,
        "ask_user_option_0",
        "first AskUser",
    )
    .await;

    send_ask_user_prompt(&mut proc, 5, &second_session_id).await;
    let permission_request_id = wait_for_permission_request(
        &mut proc,
        5,
        &second_session_id,
        "ask_user_option_1",
        "second AskUser",
    )
    .await;
    send_permission_response(
        &mut proc,
        permission_request_id,
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("ask_user_option_1")),
    )
    .await;

    let (parsed, agent_text) =
        wait_for_prompt_response_and_text(&mut proc, 5, "second session AskUser prompt").await;
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("end_turn")
    );
    assert!(
        agent_text.contains("\"Pick a color\" = \"blue\""),
        "second session AskUser should complete independently: {agent_text:?}"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": first_session_id }
    }))
    .await;
    let stop_reason = prompt_stop_reason(&proc.recv_response(4).await);
    assert!(
        stop_reason == json!("cancelled") || stop_reason == json!("end_turn"),
        "unexpected stop reason after cancelling first session AskUser: {stop_reason}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_tool_lifecycle_emits_tool_updates() {
    let mut proc =
        AcpProcess::spawn_with_base_url_and_allow_tools(BASH_TOOL_MOCK_BASE_URL, true).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "run a bash tool"}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut saw_tool_call = false;
    let mut saw_tool_call_update = false;
    let mut statuses = Vec::new();
    let mut tool_call_titles = Vec::new();
    let mut tool_call_update_titles = Vec::new();
    let prompt_response = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for tool lifecycle prompt response"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            let update = serde_json::to_value(notification.update).expect("serialize update");
            match update.get("sessionUpdate").and_then(Value::as_str) {
                Some("tool_call") => {
                    saw_tool_call = true;
                    if let Some(status) = update.get("status").and_then(Value::as_str) {
                        statuses.push(status.to_string());
                    }
                    if let Some(title) = update.get("title").and_then(Value::as_str) {
                        tool_call_titles.push(title.to_string());
                    }
                }
                Some("tool_call_update") => {
                    saw_tool_call_update = true;
                    if let Some(status) = update.get("status").and_then(Value::as_str) {
                        statuses.push(status.to_string());
                    }
                    if let Some(title) = update.get("title").and_then(Value::as_str) {
                        tool_call_update_titles.push(title.to_string());
                    }
                }
                _ => {}
            }
        }

        if msg.get("id").and_then(Value::as_i64) == Some(3) {
            break msg;
        }
    };

    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("end_turn")
    );
    assert!(saw_tool_call, "expected ACP tool_call update");
    assert!(saw_tool_call_update, "expected ACP tool_call_update update");
    assert!(
        statuses.iter().any(|status| status == "in_progress"),
        "expected in_progress status in {statuses:?}"
    );
    assert!(
        statuses.iter().any(|status| status == "completed"),
        "expected completed status in {statuses:?}"
    );
    assert!(
        tool_call_titles.iter().any(|title| {
            let lowered = title.to_ascii_lowercase();
            lowered.starts_with("bash(") && lowered.contains("echo")
        }),
        "expected descriptive bash(<command>) tool_call title in {tool_call_titles:?}"
    );
    assert!(
        tool_call_update_titles
            .iter()
            .any(|title| !title.is_empty()),
        "expected non-empty tool_call_update titles in {tool_call_update_titles:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_mcp_trust_approve_via_request_permission() {
    let mut proc = AcpProcess::spawn_with_base_url_and_allow_tools(
        "mock://anthropic?scenario=tool_use&key=mcp__docs-server__echo",
        true,
    )
    .await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_with_fake_mcp(&mut proc).await;

    let parsed = prompt_for_mcp_trust_and_respond(
        &mut proc,
        &session_id,
        "trigger MCP tool trust",
        "trust_mcp_server",
    )
    .await;
    let stop_reason = serde_json::to_value(parsed.stop_reason).expect("stop reason JSON");
    assert!(
        stop_reason == json!("end_turn") || stop_reason == json!("cancelled"),
        "unexpected stop reason after MCP trust approval: {stop_reason}"
    );
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "session MCP overlay must not persist to servers.json"
    );
    assert!(
        !proc.home().join("mcp").join("trust.json").exists(),
        "session MCP trust approval must not persist to trust.json"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_mcp_trust_deny_via_request_permission() {
    let mut proc = AcpProcess::spawn_with_base_url_and_allow_tools(
        "mock://anthropic?scenario=tool_use&key=mcp__docs-server__echo",
        true,
    )
    .await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_with_fake_mcp(&mut proc).await;

    let parsed = prompt_for_mcp_trust_and_respond(
        &mut proc,
        &session_id,
        "trigger MCP tool trust",
        "reject_mcp_server",
    )
    .await;
    let stop_reason = serde_json::to_value(parsed.stop_reason).expect("stop reason JSON");
    assert!(
        stop_reason == json!("end_turn") || stop_reason == json!("cancelled"),
        "unexpected stop reason after MCP trust denial: {stop_reason}"
    );
    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "session MCP overlay must not persist to servers.json"
    );
    assert!(
        !proc.home().join("mcp").join("trust.json").exists(),
        "session MCP trust denial must not persist to trust.json"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_close_denies_pending_mcp_trust_request() {
    let mut proc = AcpProcess::spawn_with_base_url_and_allow_tools(
        "mock://anthropic?scenario=tool_use&key=mcp__docs-server__echo",
        true,
    )
    .await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_with_fake_mcp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "trigger MCP tool trust"}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for ACP MCP trust session/request_permission"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            let request: RequestPermissionRequest = serde_json::from_value(msg["params"].clone())
                .expect("valid ACP requestPermission request");
            assert_eq!(request.session_id.to_string(), session_id);
            assert!(
                request
                    .options
                    .iter()
                    .any(|option| option.option_id.to_string() == "reject_mcp_server"),
                "expected MCP reject option: {:?}",
                request.options
            );
            break;
        }

        assert_ne!(
            msg.get("id").and_then(Value::as_i64),
            Some(3),
            "session/prompt completed before MCP trust request: {msg:?}"
        );
    }

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/close",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut saw_close = false;
    let mut prompt_response = None;
    while !saw_close || prompt_response.is_none() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for close and prompt cleanup responses"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        match msg.get("id").and_then(Value::as_i64) {
            Some(4) => {
                assert!(msg.get("error").is_none(), "{msg:?}");
                saw_close = true;
            }
            Some(3) => {
                assert!(msg.get("error").is_none(), "{msg:?}");
                prompt_response = Some(msg);
            }
            _ => {}
        }
    }
    let prompt_response = prompt_response.expect("prompt response captured");
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("cancelled")
    );

    assert!(
        !proc.home().join("mcp").join("servers.json").exists(),
        "closing ACP session must not persist MCP overlay"
    );
    assert!(
        !proc.home().join("mcp").join("trust.json").exists(),
        "close-time MCP trust denial must not persist to trust.json"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_cancel_denies_pending_mcp_trust_request() {
    let mut proc = AcpProcess::spawn_with_base_url_and_allow_tools(
        "mock://anthropic?scenario=tool_use&key=mcp__docs-server__echo",
        true,
    )
    .await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_with_fake_mcp(&mut proc).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "trigger MCP tool trust"}]
        }
    }))
    .await;

    wait_for_permission_request(&mut proc, 3, &session_id, "reject_mcp_server", "MCP trust").await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;

    let stop_reason = prompt_stop_reason(&proc.recv_response(3).await);
    assert!(
        stop_reason == json!("cancelled") || stop_reason == json!("end_turn"),
        "unexpected stop reason after pending MCP trust cancel: {stop_reason}"
    );

    assert!(
        !proc.home().join("mcp").join("trust.json").exists(),
        "cancel-time MCP trust denial must not persist to trust.json"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_close_cancels_active_non_mcp_turn() {
    let mut proc = AcpProcess::spawn_with_base_url(HANG_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "hang until closed"}]
        }
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/close",
        "params": {
            "sessionId": session_id
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut saw_close = false;
    let mut captured_stop_reason = None;
    while !saw_close || captured_stop_reason.is_none() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for active non-MCP close cleanup"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        match msg.get("id").and_then(Value::as_i64) {
            Some(4) => {
                assert!(msg.get("error").is_none(), "{msg:?}");
                saw_close = true;
            }
            Some(3) => {
                captured_stop_reason = Some(prompt_stop_reason(&msg));
            }
            _ => {}
        }
    }

    assert_eq!(captured_stop_reason, Some(json!("cancelled")));

    proc.close().await;
}

#[tokio::test]
async fn acp_cancelled_prompt_does_not_leak_into_next_prompt() {
    let mut proc = AcpProcess::spawn_with_base_url(HANG_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "first hanging prompt"}]
        }
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "second prompt while first is active"}]
        }
    }))
    .await;
    let second_while_active = proc.recv_response(4).await;
    assert!(
        second_while_active.get("error").is_some(),
        "second prompt should be rejected while first is active: {second_while_active:?}"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;
    assert_eq!(
        prompt_stop_reason(&proc.recv_response(3).await),
        json!("cancelled")
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "next hanging prompt"}]
        }
    }))
    .await;
    assert_no_response_for(&mut proc, 5, Duration::from_millis(250)).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;
    assert_eq!(
        prompt_stop_reason(&proc.recv_response(5).await),
        json!("cancelled")
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_stdin_eof_exits_with_active_turn() {
    let mut proc = AcpProcess::spawn_with_base_url(HANG_MOCK_BASE_URL).await;

    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "hang until stdin closes"}]
        }
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    proc.close_expect_exit().await;
}

#[tokio::test]
async fn acp_mcp_trust_request_keeps_session_id_with_multiple_sessions() {
    let mut proc = AcpProcess::spawn_with_base_url_and_allow_tools(
        "mock://anthropic?scenario=tool_use&key=mcp__docs-server-2__echo",
        true,
    )
    .await;

    initialize_acp(&mut proc).await;
    let first_session_id = new_session_with_fake_mcp_id(&mut proc, 2).await;
    let second_session_id = new_session_with_fake_mcp_id(&mut proc, 3).await;
    assert_ne!(first_session_id, second_session_id);

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": second_session_id,
            "prompt": [{"type": "text", "text": "trigger second-session MCP trust"}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for ACP MCP trust session/request_permission"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            let request: RequestPermissionRequest = serde_json::from_value(msg["params"].clone())
                .expect("valid ACP requestPermission request");
            assert_eq!(request.session_id.to_string(), second_session_id);
            assert_ne!(request.session_id.to_string(), first_session_id);
            break;
        }

        assert_ne!(
            msg.get("id").and_then(Value::as_i64),
            Some(4),
            "session/prompt completed before MCP trust request: {msg:?}"
        );
    }

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/close",
        "params": {
            "sessionId": second_session_id
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut saw_close = false;
    let mut saw_prompt_cancelled = false;
    while !saw_close || !saw_prompt_cancelled {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for second session close and prompt cleanup responses"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        match msg.get("id").and_then(Value::as_i64) {
            Some(5) => {
                assert!(msg.get("error").is_none(), "{msg:?}");
                saw_close = true;
            }
            Some(4) => {
                assert!(msg.get("error").is_none(), "{msg:?}");
                let parsed: PromptResponse =
                    serde_json::from_value(msg["result"].clone()).expect("valid prompt response");
                assert_eq!(
                    serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
                    json!("cancelled")
                );
                saw_prompt_cancelled = true;
            }
            _ => {}
        }
    }

    proc.close().await;
}

#[tokio::test]
async fn acp_session_mcp_server_is_not_trustable_from_another_session() {
    let mut proc = AcpProcess::spawn_with_base_url_and_allow_tools(
        "mock://anthropic?scenario=tool_use&key=mcp__docs-server__echo",
        true,
    )
    .await;

    initialize_acp(&mut proc).await;
    let first_session_id = new_session_with_fake_mcp_id(&mut proc, 2).await;
    let second_session_id = new_session_without_mcp_id(&mut proc, 3).await;
    assert_ne!(first_session_id, second_session_id);

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": second_session_id,
            "prompt": [{"type": "text", "text": "try another session MCP server"}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut prompt_response = None;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(msg) = proc.recv_timeout(remaining).await else {
            break;
        };
        assert_ne!(
            msg.get("method").and_then(Value::as_str),
            Some("session/request_permission"),
            "second session must not receive a trust request for first session MCP server: {msg:?}"
        );
        if msg.get("id").and_then(Value::as_i64) == Some(4) {
            prompt_response = Some(msg);
            break;
        }
    }

    if prompt_response.is_none() {
        proc.send(&json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": second_session_id }
        }))
        .await;
        prompt_response = Some(proc.recv_response(4).await);
    }

    let prompt_response = prompt_response.expect("prompt response captured");
    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_session_prompt_basic_turn() {
    let mut proc = AcpProcess::spawn().await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");

    let cwd = proc.cwd().to_string_lossy().to_string();
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": []
        }
    }))
    .await;
    let new_session = proc.recv_response(2).await;
    assert!(new_session.get("error").is_none(), "{new_session:?}");
    let new_session: NewSessionResponse = serde_json::from_value(new_session["result"].clone())
        .expect("valid ACP session/new response");
    let session_id = new_session.session_id.to_string();

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "say hello"}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut saw_agent_update = false;
    let prompt_response = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for session/prompt response"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/update") {
            let notification: SessionNotification =
                serde_json::from_value(msg["params"].clone()).expect("valid session/update");
            if serde_json::to_value(notification.update)
                .expect("serialize update")
                .get("sessionUpdate")
                .and_then(Value::as_str)
                == Some("agent_message_chunk")
            {
                saw_agent_update = true;
            }
        }

        if msg.get("id").and_then(Value::as_i64) == Some(3) {
            break msg;
        }
    };

    assert!(
        saw_agent_update,
        "session/prompt should stream at least one ACP agent_message_chunk"
    );
    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    assert_eq!(
        serde_json::to_value(parsed.stop_reason).expect("stop reason JSON"),
        json!("end_turn")
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_permission_deny_via_request_permission() {
    let mut proc = AcpProcess::spawn_with_base_url(
        "mock://anthropic?scenario=tool_use&key=bash&command=echo+hi",
    )
    .await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "zed-test", "version": "0.0.0"}
        }
    }))
    .await;
    let init = proc.recv_response(1).await;
    assert!(init.get("error").is_none(), "{init:?}");

    let cwd = proc.cwd().to_string_lossy().to_string();
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": []
        }
    }))
    .await;
    let new_session = proc.recv_response(2).await;
    assert!(new_session.get("error").is_none(), "{new_session:?}");
    let new_session: NewSessionResponse = serde_json::from_value(new_session["result"].clone())
        .expect("valid ACP session/new response");
    let session_id = new_session.session_id.to_string();

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "run echo hi"}]
        }
    }))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let permission_request_id = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for ACP session/request_permission"
        );
        let msg = proc
            .recv_timeout(remaining)
            .await
            .expect("process should produce JSON-RPC");

        if msg.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            let request: RequestPermissionRequest = serde_json::from_value(msg["params"].clone())
                .expect("valid ACP requestPermission request");
            assert_eq!(request.session_id.to_string(), session_id);
            assert!(
                request
                    .options
                    .iter()
                    .any(|option| option.option_id.to_string() == "reject_once"),
                "deny option should be present: {:?}",
                request.options
            );
            break msg["id"].clone();
        }
    };

    let response = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new("reject_once"),
    ));
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": permission_request_id,
        "result": serde_json::to_value(response).expect("permission response JSON")
    }))
    .await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;

    let prompt_response = proc.recv_response(3).await;
    assert!(
        prompt_response.get("error").is_none(),
        "{prompt_response:?}"
    );
    let parsed: PromptResponse = serde_json::from_value(prompt_response["result"].clone())
        .expect("valid ACP prompt response");
    let stop_reason = serde_json::to_value(parsed.stop_reason).expect("stop reason JSON");
    assert!(
        stop_reason == json!("end_turn") || stop_reason == json!("cancelled"),
        "unexpected stop reason after permission deny: {stop_reason}"
    );

    proc.close().await;
}

#[tokio::test]
async fn acp_session_controls_have_stable_shape_and_session_isolation() {
    let mut proc = AcpProcess::spawn().await;
    initialize_acp(&mut proc).await;
    let cwd = proc.cwd().to_string_lossy().to_string();

    let mut new_results = Vec::new();
    for id in [2, 3] {
        proc.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/new",
            "params": {"cwd": cwd, "mcpServers": []}
        }))
        .await;
        let response = proc.recv_response(id).await;
        assert!(response.get("error").is_none(), "{response:?}");
        new_results.push(response["result"].clone());
    }
    let first_session = new_results[0]["sessionId"]
        .as_str()
        .expect("first session id")
        .to_string();
    let second_session = new_results[1]["sessionId"]
        .as_str()
        .expect("second session id")
        .to_string();
    assert_ne!(first_session, second_session);

    for result in &new_results {
        assert_eq!(result["modes"]["currentModeId"], "default");
        let mode_ids = result["modes"]["availableModes"]
            .as_array()
            .expect("available modes")
            .iter()
            .map(|mode| mode["id"].as_str().expect("mode id"))
            .collect::<Vec<_>>();
        assert_eq!(
            mode_ids,
            vec!["default", "accept_edits", "plan", "dont_ask"]
        );
        assert!(!mode_ids.contains(&"bypass_permissions"));
        assert!(config_option(&result["configOptions"], "model").is_some());
        assert!(config_option(&result["configOptions"], "thought_level").is_some());
    }

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/set_mode",
        "params": {"sessionId": first_session, "modeId": "plan"}
    }))
    .await;
    let mode_response = proc.recv_response(4).await;
    assert!(mode_response.get("error").is_none(), "{mode_response:?}");
    assert_eq!(mode_response["result"], json!({}));

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/set_config_option",
        "params": {"sessionId": first_session, "configId": "model", "value": "sonnet"}
    }))
    .await;
    let model_response = proc.recv_response(5).await;
    assert!(model_response.get("error").is_none(), "{model_response:?}");
    assert_eq!(
        config_option(&model_response["result"]["configOptions"], "model").expect("model option")["currentValue"],
        "sonnet"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "session/set_config_option",
        "params": {"sessionId": first_session, "configId": "thought_level", "value": "high"}
    }))
    .await;
    let effort_response = proc.recv_response(6).await;
    assert!(
        effort_response.get("error").is_none(),
        "{effort_response:?}"
    );
    assert_eq!(
        config_option(&effort_response["result"]["configOptions"], "thought_level")
            .expect("thought option")["currentValue"],
        "high"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "session/set_config_option",
        "params": {"sessionId": second_session, "configId": "model", "value": "default"}
    }))
    .await;
    let second_response = proc.recv_response(7).await;
    assert!(
        second_response.get("error").is_none(),
        "{second_response:?}"
    );
    assert_eq!(
        config_option(&second_response["result"]["configOptions"], "model")
            .expect("second model option")["currentValue"],
        "default",
        "changing the first session must not alter the second session"
    );

    for (id, params) in [
        (
            8,
            json!({"sessionId": first_session, "modeId": "bypass_permissions"}),
        ),
        (9, json!({"sessionId": "missing-session", "modeId": "plan"})),
    ] {
        proc.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/set_mode",
            "params": params
        }))
        .await;
        let response = proc.recv_response(id).await;
        assert_eq!(response["error"]["code"], json!(-32602), "{response:?}");
    }

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "session/set_config_option",
        "params": {"sessionId": first_session, "configId": "model", "value": true}
    }))
    .await;
    let wrong_kind = proc.recv_response(10).await;
    assert_eq!(wrong_kind["error"]["code"], json!(-32602), "{wrong_kind:?}");

    proc.close().await;
}

#[tokio::test]
async fn acp_session_control_change_during_prompt_is_rejected_until_next_prompt_boundary() {
    let mut proc = AcpProcess::spawn_with_base_url(HANG_MOCK_BASE_URL).await;
    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "keep this turn active"}]
        }
    }))
    .await;
    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/set_mode",
        "params": {"sessionId": session_id, "modeId": "plan"}
    }))
    .await;
    let response = proc.recv_response(4).await;
    assert_eq!(response["error"]["code"], json!(-32602), "{response:?}");
    assert!(
        response["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("active")),
        "{response:?}"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": {"sessionId": session_id}
    }))
    .await;
    let prompt = proc.recv_response(3).await;
    assert!(prompt.get("error").is_none(), "{prompt:?}");
    proc.close().await;
}

#[tokio::test]
async fn acp_session_controls_surface_managed_setting_locks() {
    let mut proc = AcpProcess::spawn_with_managed_settings(
        r#"{"model":"sonnet","effortLevel":"high","permissions":{"defaultMode":"default"}}"#,
    )
    .await;
    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;

    for (id, method, params) in [
        (
            3,
            "session/set_config_option",
            json!({"sessionId": session_id, "configId": "model", "value": "opus"}),
        ),
        (
            4,
            "session/set_config_option",
            json!({"sessionId": session_id, "configId": "thought_level", "value": "low"}),
        ),
        (
            5,
            "session/set_mode",
            json!({"sessionId": session_id, "modeId": "plan"}),
        ),
    ] {
        proc.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await;
        let response = proc.recv_response(id).await;
        assert_eq!(response["error"]["code"], json!(-32602), "{response:?}");
        assert!(
            response["error"]["data"]
                .as_str()
                .is_some_and(|message| message.contains("locked by managed policy")),
            "{response:?}"
        );
    }

    proc.close().await;
}

#[tokio::test]
async fn acp_unsupported_prompt_content_submits_zero_turns_and_context_is_attributed() {
    let mut proc = AcpProcess::spawn().await;
    initialize_acp(&mut proc).await;
    let session_id = new_session_without_mcp_id(&mut proc, 2).await;
    let transcript = proc
        .home()
        .join("projects")
        .join(sanitize_path(
            &proc.cwd().canonicalize().unwrap().display().to_string(),
        ))
        .join(format!("{session_id}.jsonl"));

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "image", "data": "AA==", "mimeType": "image/png"}]
        }
    }))
    .await;
    let rejected = proc.recv_response(3).await;
    assert_eq!(rejected["error"]["code"], json!(-32602), "{rejected:?}");
    assert!(
        rejected["error"]["data"]
            .as_str()
            .is_some_and(|message| message.contains("image input is unsupported")),
        "{rejected:?}"
    );
    assert!(
        !transcript.exists(),
        "unsupported prompt content must be rejected before turn persistence"
    );

    proc.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [
                {"type": "text", "text": "alpha"},
                {
                    "type": "resource_link",
                    "name": "Guide",
                    "uri": "file:///guide.md",
                    "description": "Project guide",
                    "mimeType": "text/markdown"
                },
                {
                    "type": "resource",
                    "resource": {
                        "uri": "file:///context.txt",
                        "mimeType": "text/plain",
                        "text": "embedded context"
                    }
                },
                {"type": "text", "text": "omega"}
            ]
        }
    }))
    .await;
    let accepted = proc.recv_response(4).await;
    assert!(accepted.get("error").is_none(), "{accepted:?}");

    let body = std::fs::read_to_string(&transcript).expect("accepted turn transcript");
    assert_eq!(body.matches(r#""type":"user""#).count(), 1);
    assert!(!body.contains("Unsupported image content"));
    let alpha = body.find("alpha").expect("leading text");
    let link = body.find("file:///guide.md").expect("link attribution");
    let embedded = body
        .find("file:///context.txt")
        .expect("embedded attribution");
    let omega = body.find("omega").expect("trailing text");
    assert!(alpha < link && link < embedded && embedded < omega);
    assert!(body.contains("metadata only, not fetched"));

    proc.close().await;
}

#[tokio::test]
async fn acp_sdk_client_session_control_conformance() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let agent = sdk_acp_agent("mock://anthropic?scenario=success", home.path());
    let cwd_path = cwd.path().to_path_buf();

    let run =
        Client
            .builder()
            .name("orbcode-sdk-client-session-controls")
            .connect_with(agent, async move |connection| {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let created = connection
                    .send_request(NewSessionRequest::new(cwd_path))
                    .block_task()
                    .await?;
                let modes = created.modes.expect("SDK parses session modes");
                assert_eq!(modes.current_mode_id.to_string(), "default");
                assert_eq!(modes.available_modes.len(), 4);
                let options = created.config_options.expect("SDK parses config options");
                assert!(
                    options
                        .iter()
                        .any(|option| option.id.to_string() == "model")
                );
                assert!(
                    options
                        .iter()
                        .any(|option| option.id.to_string() == "thought_level")
                );

                connection
                    .send_request(SetSessionModeRequest::new(
                        created.session_id.clone(),
                        "plan",
                    ))
                    .block_task()
                    .await?;
                let changed = connection
                    .send_request(SetSessionConfigOptionRequest::new(
                        created.session_id.clone(),
                        "model",
                        "sonnet",
                    ))
                    .block_task()
                    .await?;
                let changed_json = serde_json::to_value(changed).expect("config response JSON");
                assert_eq!(
                    config_option(&changed_json["configOptions"], "model").expect("changed model")
                        ["currentValue"],
                    "sonnet"
                );
                connection
                    .send_request(CloseSessionRequest::new(created.session_id))
                    .block_task()
                    .await?;
                Ok(())
            });

    tokio::time::timeout(Duration::from_secs(45), run)
        .await
        .expect("SDK ACP session-control harness timed out")
        .expect("SDK ACP session-control harness failed");
}

fn config_option<'a>(options: &'a Value, id: &str) -> Option<&'a Value> {
    options
        .as_array()?
        .iter()
        .find(|option| option["id"].as_str() == Some(id))
}
