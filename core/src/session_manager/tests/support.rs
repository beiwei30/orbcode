use std::io::{Read, Write};
use std::net::TcpListener;
use std::ops::{Deref, DerefMut};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc as std_mpsc,
};
use std::thread;
use std::time::Duration as StdDuration;

use orbcode_config::{
    AppConfig, AppConfigOverrides, ClaudeSettings, EditorModeSetting, EffectivePolicy,
    SettingsLayers, ThemeSetting,
};
use orbcode_mcp::McpRegistry;
use orbcode_protocol::{
    MessageRole, ProviderId, SandboxMode, StreamEvent, TranscriptBlock, TranscriptMessage,
    TurnCancellationKind,
};
use orbcode_tools::ToolRegistry;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use uuid::Uuid;

use super::super::SessionManager;
use crate::{CoreError, agent_loop::tool_round::ToolRoundResponse, turn_loop::TurnLoopOutcome};

impl SessionManager {
    pub(super) async fn handle_provider_response(
        &self,
        session_id: &str,
        turn_id: Uuid,
        prompt: &str,
        response: orbcode_model_provider::ProviderResponse,
        auto_continue_attempts: usize,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<TurnLoopOutcome, CoreError> {
        let _ = tx.send(StreamEvent::AssistantMessageStarted {
            session_id: session_id.to_string(),
            provider: response.provider,
            fallback_from: response.fallback_from,
        });

        if let Some(thinking) = latest_thinking_text(&response.blocks) {
            let _ = tx.send(StreamEvent::ThinkingStarted {
                session_id: session_id.to_string(),
                provider: response.provider,
            });
            for delta in chunk_response(&thinking) {
                if cancel_flag.load(Ordering::SeqCst) {
                    break;
                }
                let _ = tx.send(StreamEvent::ThinkingDelta {
                    session_id: session_id.to_string(),
                    delta,
                });
                tokio::task::yield_now().await;
            }
            if !cancel_flag.load(Ordering::SeqCst) {
                let _ = tx.send(StreamEvent::ThinkingCompleted {
                    session_id: session_id.to_string(),
                    provider: response.provider,
                });
            }
        }

        let deltas = if response.deltas.is_empty() {
            chunk_response(&response.content)
        } else {
            response.deltas.clone()
        };

        let mut assembled = String::new();
        for delta in deltas {
            if cancel_flag.load(Ordering::SeqCst) {
                break;
            }
            assembled.push_str(&delta);
            let _ = tx.send(StreamEvent::AssistantDelta {
                session_id: session_id.to_string(),
                delta,
            });
            tokio::task::yield_now().await;
        }

        if cancel_flag.load(Ordering::SeqCst) {
            let partial = if assembled.is_empty() {
                None
            } else {
                Some(
                    TranscriptMessage::new(MessageRole::Assistant, assembled.clone())
                        .with_usage(response.usage.clone()),
                )
            };
            if let Some(message) = partial.clone() {
                let _ = self.append_message(session_id, message).await;
            }
            let usage = if assembled.is_empty() {
                None
            } else {
                Some(response.usage.clone())
            };
            if self.active_turns.is_active(session_id, turn_id).await {
                self.append_interruption_message(session_id, false, tx)
                    .await?;
            }
            let _ = tx.send(StreamEvent::TurnCancelled {
                session_id: session_id.to_string(),
                kind: TurnCancellationKind::AssistantStreaming,
                partial,
                usage,
            });
            return Ok(TurnLoopOutcome::Cancelled);
        }

        self.finish_provider_response(
            session_id,
            turn_id,
            prompt,
            ToolRoundResponse::from_response(response),
            assembled,
            auto_continue_attempts,
            false,
            tx,
            cancel_flag,
        )
        .await
    }
}

pub(super) fn chunk_response(text: &str) -> Vec<String> {
    const MAX_CHUNK: usize = 18;

    let mut chunks = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= MAX_CHUNK || ch == '\n' {
            chunks.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

fn latest_thinking_text(blocks: &[TranscriptBlock]) -> Option<String> {
    blocks.iter().rev().find_map(|block| match block {
        TranscriptBlock::Thinking { text, .. } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    })
}

/// Cap for concurrent SessionManager-using tests.
///
/// `cargo test` spawns one OS thread per test up to `--test-threads`
/// (defaults to the CPU count). Each `#[tokio::test]` builds its own
/// current-thread runtime, plus the agent loop tokio-spawns nested tasks
/// and reqwest pulls in its own worker threads. On a 10-core dev box that
/// yields ~30–50 concurrent OS threads, and the per-test runtime ends up
/// CPU-starved long enough that `tokio::time::timeout` budgets fire even
/// when nothing is actually deadlocked. Limiting concurrent
/// `test_manager()` callers keeps the per-test runtime liveness above the
/// flaky threshold without forcing `--test-threads=1`. Other tests
/// (pure-unit, no SessionManager) continue to parallelise freely.
fn test_concurrency_slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(8, std::num::NonZero::get);
        // Roughly one SessionManager test per 3 cores keeps the
        // per-test runtime responsive. Floor at 2 so very small machines
        // still get some parallelism.
        let slots = (cores / 3).max(2);
        Arc::new(Semaphore::new(slots))
    })
}

async fn acquire_test_slot() -> OwnedSemaphorePermit {
    test_concurrency_slots()
        .clone()
        .acquire_owned()
        .await
        .expect("acquire test concurrency slot")
}

/// `SessionManager` plus a concurrency-slot guard. Tests bind this as
/// `let manager = test_manager().await;` and use it transparently —
/// `Deref` / `DerefMut` forward all method and field access to the
/// underlying `SessionManager`. The slot guard releases when the test
/// function ends and the wrapper drops.
pub(super) struct TestSessionManager {
    inner: SessionManager,
    _slot: OwnedSemaphorePermit,
}

impl Deref for TestSessionManager {
    type Target = SessionManager;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for TestSessionManager {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub(super) async fn test_manager() -> TestSessionManager {
    let _slot = acquire_test_slot().await;
    let inner = build_test_session_manager().await;
    TestSessionManager { inner, _slot }
}

async fn build_test_session_manager() -> SessionManager {
    let home_dir = std::env::temp_dir().join(format!("orbcode-test-{}", Uuid::new_v4()));
    let sessions_dir = home_dir.join("sessions");
    let projects_dir = home_dir.join("projects");
    let current_project_dir = projects_dir.join("project");
    tokio::fs::create_dir_all(&sessions_dir)
        .await
        .expect("create session dir");
    tokio::fs::create_dir_all(&current_project_dir)
        .await
        .expect("create project dir");

    let config = AppConfig {
        cwd: home_dir.clone(),
        home_dir: home_dir.clone(),
        sessions_dir,
        projects_dir,
        current_project_dir,
        history_path: home_dir.join("history.jsonl"),
        settings_path: home_dir.join("settings.json"),
        default_provider: ProviderId::Anthropic,
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: 1,
        sandbox_mode: SandboxMode::DangerFullAccess,
        sandbox_allow_network: true,
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        ask_tools: Vec::new(),
        additional_directories: Vec::new(),
        mcp_config_inputs: Vec::new(),
        settings: ClaudeSettings {
            // Stub provider mode lives in `settings.env` so individual
            // tests can still override it via `set_anthropic_server_env`
            // / `set_openai_server_env`. The seal-off lives in
            // `env_overrides` below.
            env: [
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "stub://anthropic".to_string(),
                ),
                ("ANTHROPIC_MODEL".to_string(), "stub-model".to_string()),
                // Collapse the retry backoff so provider retry/fallback tests
                // do not sleep between attempts.
                (
                    "CLAUDE_CODE_RETRY_BASE_DELAY_MS".to_string(),
                    "0".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            model: None,
            theme: ThemeSetting::Auto,
            editor_mode: EditorModeSetting::Normal,
            always_thinking_enabled: None,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            ask_tools: Vec::new(),
            additional_directories: Vec::new(),
            hooks: Default::default(),
            hook_sources: Default::default(),
            max_budget_usd: None,
            max_budget_strict_unknown_pricing: None,
            statusline_command: None,
            statusline_refresh_interval_secs: None,
        },
        settings_layers: SettingsLayers::default(),
        resolved_settings: Default::default(),
        settings_warnings: Vec::new(),
        policy: EffectivePolicy::default(),
        policy_conflicts: Vec::new(),
        runtime_model_override: None,
        // Empty entries seal off matching `std::env::var` reads but let
        // `settings.env` still serve the value, so a developer's real
        // `ANTHROPIC_*` / `OPENAI_*` shell env never bleeds into the
        // stub-backed tests while individual tests can still override
        // through `settings.env`.
        env_overrides: [
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "OPENAI_BASE_URL",
            "OPENAI_API_KEY",
            "CLAUDE_CODE_RETRY_BASE_DELAY_MS",
            "CLAUDE_CODE_RETRY_MAX_DELAY_MS",
        ]
        .into_iter()
        .map(|key| (key.to_string(), String::new()))
        .collect(),
        append_system_prompt: None,
        permission_mode: None,
        trusted_project: true,
    };
    let tools = ToolRegistry::foundation();
    let mcp = McpRegistry::load(home_dir.clone(), home_dir.clone())
        .await
        .expect("create mcp registry");
    SessionManager::new(config, tools, mcp)
}

pub(super) async fn test_manager_with_overrides(
    overrides: AppConfigOverrides,
) -> TestSessionManager {
    let mut manager = test_manager().await;
    manager.config.default_provider = overrides.default_provider.unwrap_or(ProviderId::Anthropic);
    manager.config.fallback_provider = overrides.fallback_provider;
    manager.config.max_retries = overrides.max_retries.unwrap_or(1);
    manager.config.sandbox_mode = overrides.sandbox_mode.unwrap_or_default();
    manager.config.sandbox_allow_network = overrides.sandbox_allow_network.unwrap_or(true);
    manager.config.allow_network = overrides.allow_network.unwrap_or(true);
    manager.config.provider_allow_network = overrides.provider_allow_network.unwrap_or(true);
    manager.config.allow_tools = overrides.allow_tools.unwrap_or(false);
    manager.config.allowed_tools = overrides.allowed_tools;
    manager.config.disallowed_tools = overrides.disallowed_tools;
    manager
}

pub(super) fn start_hanging_anthropic_server()
-> (String, std_mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    listener
        .set_nonblocking(true)
        .expect("set test server nonblocking");
    let address = listener.local_addr().expect("server addr");
    let (shutdown_tx, shutdown_rx) = std_mpsc::channel();

    let handle = thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // On BSD/macOS the accepted socket inherits the listener's
                    // non-blocking flag, which makes `set_read_timeout` a no-op
                    // and lets `read` return `WouldBlock` before a slow client's
                    // request arrives. Force blocking so the timeout governs.
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
                    let request = read_test_http_request(&mut stream);
                    if is_anthropic_count_tokens_request(&request) {
                        write_anthropic_count_tokens_response(&mut stream);
                        continue;
                    }
                    stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                            )
                            .expect("write response headers");
                    let keepalive = b": keepalive\n\n";
                    let chunk_prefix = format!("{:X}\r\n", keepalive.len());
                    stream
                        .write_all(chunk_prefix.as_bytes())
                        .expect("write chunk prefix");
                    stream.write_all(keepalive).expect("write keepalive");
                    stream.write_all(b"\r\n").expect("write chunk suffix");
                    let _ = stream.flush();
                    let _ = shutdown_rx.recv_timeout(StdDuration::from_secs(2));
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    thread::sleep(StdDuration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (format!("http://{address}"), shutdown_tx, handle)
}

pub(super) fn start_partial_text_anthropic_server()
-> (String, std_mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind partial stream server");
    listener
        .set_nonblocking(true)
        .expect("set partial stream server nonblocking");
    let address = listener.local_addr().expect("partial server addr");
    let (shutdown_tx, shutdown_rx) = std_mpsc::channel();

    let handle = thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // On BSD/macOS the accepted socket inherits the listener's
                    // non-blocking flag, which makes `set_read_timeout` a no-op
                    // and lets `read` return `WouldBlock` before a slow client's
                    // request arrives. Force blocking so the timeout governs.
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
                    let request = read_test_http_request(&mut stream);
                    if is_anthropic_count_tokens_request(&request) {
                        write_anthropic_count_tokens_response(&mut stream);
                        continue;
                    }
                    stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                            )
                            .expect("write partial response headers");
                    let body = concat!(
                        "event: message_start\n",
                        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                        "event: content_block_start\n",
                        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                        "event: content_block_delta\n",
                        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial live\"}}\n\n",
                    );
                    let chunk_prefix = format!("{:X}\r\n", body.len());
                    stream
                        .write_all(chunk_prefix.as_bytes())
                        .expect("write partial chunk prefix");
                    stream
                        .write_all(body.as_bytes())
                        .expect("write partial chunk");
                    stream
                        .write_all(b"\r\n")
                        .expect("write partial chunk suffix");
                    let _ = stream.flush();
                    let _ = shutdown_rx.recv_timeout(StdDuration::from_secs(2));
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    thread::sleep(StdDuration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (format!("http://{address}"), shutdown_tx, handle)
}

pub(super) fn start_error_after_content_anthropic_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stream error server");
    let address = listener.local_addr().expect("stream error server addr");

    let handle = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept stream error request");
            let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
            let request = read_test_http_request(&mut stream);
            if is_anthropic_count_tokens_request(&request) {
                write_anthropic_count_tokens_response(&mut stream);
                continue;
            }
            let body = concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"before error\"}}\n\n",
                "event: error\n",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"server overloaded after content\"}}\n\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body,
            );
            stream
                .write_all(response.as_bytes())
                .expect("write stream error response");
            let _ = stream.flush();
            return;
        }
        panic!("stream error server did not receive a streaming request");
    });

    (format!("http://{address}"), handle)
}

pub(super) fn start_error_after_thinking_anthropic_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind thinking stream error server");
    let address = listener
        .local_addr()
        .expect("thinking stream error server addr");

    let handle = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener
                .accept()
                .expect("accept thinking stream error request");
            let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
            let request = read_test_http_request(&mut stream);
            if is_anthropic_count_tokens_request(&request) {
                write_anthropic_count_tokens_response(&mut stream);
                continue;
            }
            let body = concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"considering fallback\"}}\n\n",
                "event: error\n",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"server overloaded after thinking\"}}\n\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body,
            );
            stream
                .write_all(response.as_bytes())
                .expect("write thinking stream error response");
            let _ = stream.flush();
            return;
        }
        panic!("thinking stream error server did not receive a streaming request");
    });

    (format!("http://{address}"), handle)
}

pub(super) fn start_error_after_tool_use_anthropic_server(
    command: String,
    marker_path: std::path::PathBuf,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind tool stream error server");
    let address = listener
        .local_addr()
        .expect("tool stream error server addr");

    let handle = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept tool stream error request");
            let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
            let request = read_test_http_request(&mut stream);
            if is_anthropic_count_tokens_request(&request) {
                write_anthropic_count_tokens_response(&mut stream);
                continue;
            }
            let tool_input = serde_json::to_string(&serde_json::json!({
                "command": command,
            }))
            .expect("serialize streamed tool input");
            let input_delta = serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": tool_input,
                },
            });
            let before_error = format!(
                concat!(
                    "event: message_start\n",
                    "data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":1,\"output_tokens\":0}}}}}}\n\n",
                    "event: content_block_start\n",
                    "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"tool-stream-error\",\"name\":\"bash\",\"input\":{{}}}}}}\n\n",
                    "event: content_block_delta\n",
                    "data: {}\n\n",
                    "event: content_block_stop\n",
                    "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
                ),
                input_delta,
            );
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                )
                .expect("write tool stream response headers");
            stream
                .write_all(before_error.as_bytes())
                .expect("write streamed tool use");
            stream.flush().expect("flush streamed tool use");

            for _ in 0..300 {
                if marker_path.exists() {
                    break;
                }
                thread::sleep(StdDuration::from_millis(10));
            }
            assert!(
                marker_path.exists(),
                "streamed tool must produce its external marker before the provider fails"
            );

            let error = concat!(
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null},\"usage\":{\"output_tokens\":2}}\n\n",
                "event: error\n",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"server overloaded after tool use\"}}\n\n",
            );
            stream
                .write_all(error.as_bytes())
                .expect("write error after streamed tool side effect");
            stream.flush().expect("flush streamed provider error");
            return;
        }
        panic!("tool stream error server did not receive a streaming request");
    });

    (format!("http://{address}"), handle)
}

pub(super) fn start_recording_openai_error_server() -> (
    String,
    Arc<AtomicUsize>,
    std_mpsc::Sender<()>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fallback recorder");
    listener
        .set_nonblocking(true)
        .expect("set fallback recorder nonblocking");
    let address = listener.local_addr().expect("fallback recorder addr");
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = requests.clone();
    let (shutdown_tx, shutdown_rx) = std_mpsc::channel();

    let handle = thread::spawn(move || {
        loop {
            if shutdown_rx.try_recv().is_ok() {
                return;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
                    let _request = read_test_http_request(&mut stream);
                    server_requests.fetch_add(1, Ordering::SeqCst);
                    let body = r#"{"error":{"message":"fallback recorder contacted"}}"#;
                    let response = format!(
                        "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(StdDuration::from_millis(5));
                }
                Err(_) => return,
            }
        }
    });

    (format!("http://{address}"), requests, shutdown_tx, handle)
}

fn read_test_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(_) => break,
        }
        if let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&buf[..header_end]);
            let content_length = header
                .lines()
                .find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let total = header_end + 4 + content_length;
            if buf.len() >= total {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn is_anthropic_count_tokens_request(request: &str) -> bool {
    request.starts_with("POST /v1/messages/count_tokens ")
}

fn write_anthropic_count_tokens_response(stream: &mut std::net::TcpStream) {
    let body = r#"{"input_tokens":42}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("write Anthropic count-tokens response");
    let _ = stream.flush();
}

/// Anthropic mock that answers only count-tokens requests, tallying how many it
/// served so tests can assert the count-tokens cache suppresses duplicate
/// network round-trips. The returned counter increments once per request.
pub(super) fn start_counting_count_tokens_server() -> (
    String,
    Arc<AtomicUsize>,
    std_mpsc::Sender<()>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind counting count-tokens server");
    listener
        .set_nonblocking(true)
        .expect("set counting count-tokens server nonblocking");
    let address = listener.local_addr().expect("counting server addr");
    let (shutdown_tx, shutdown_rx) = std_mpsc::channel();
    let counter = Arc::new(AtomicUsize::new(0));
    let server_counter = Arc::clone(&counter);

    let handle = thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // On BSD/macOS the accepted socket inherits the listener's
                    // non-blocking flag, which makes `set_read_timeout` a no-op
                    // and lets `read` return `WouldBlock` before a slow client's
                    // request arrives. Force blocking so the timeout governs.
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
                    let request = read_test_http_request(&mut stream);
                    if is_anthropic_count_tokens_request(&request) {
                        server_counter.fetch_add(1, Ordering::SeqCst);
                        write_anthropic_count_tokens_response(&mut stream);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    thread::sleep(StdDuration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (format!("http://{address}"), counter, shutdown_tx, handle)
}

pub(super) fn start_reactive_compaction_anthropic_server()
-> (String, std_mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind reactive compaction server");
    let address = listener
        .local_addr()
        .expect("reactive compaction server addr");
    let (request_tx, request_rx) = std_mpsc::channel();

    let handle = thread::spawn(move || {
        let mut non_count_requests = 0_usize;
        loop {
            let (mut stream, _) = listener
                .accept()
                .expect("accept reactive compaction request");
            let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
            let request = read_test_http_request(&mut stream);
            if is_anthropic_count_tokens_request(&request) {
                write_anthropic_count_tokens_response(&mut stream);
                continue;
            }

            non_count_requests += 1;
            request_tx
                .send(request)
                .expect("record reactive compaction request");
            match non_count_requests {
                1 => write_anthropic_http_error_response(
                    &mut stream,
                    413,
                    "prompt is too long: context window exceeded",
                ),
                2 => write_anthropic_text_sse_response(
                    &mut stream,
                    "reactive compact summary marker",
                ),
                3 => {
                    write_anthropic_text_sse_response(
                        &mut stream,
                        "final answer after reactive compaction",
                    );
                    return;
                }
                _ => panic!("unexpected extra reactive compaction request"),
            }
        }
    });

    (format!("http://{address}"), request_rx, handle)
}

/// Anthropic mock for precisely verifying `Retry-After` backoff timing.
///
/// Count-tokens requests are answered normally so the token preflight never
/// pollutes the retry timing (it does not go through the retry loop and would
/// otherwise show up as an extra, immediately-failing request — see the
/// manual-test surprise where a count-tokens 429 looked like a missing
/// backoff). The first `/v1/messages` attempt returns `429` with the given
/// `Retry-After` seconds; the second succeeds. The returned receiver yields the
/// arrival `Instant` of each `/v1/messages` attempt so the test can assert the
/// gap between attempts honors the server directive. Tests run with
/// `CLAUDE_CODE_RETRY_BASE_DELAY_MS=0`, so any measured gap can only come from
/// the honored `Retry-After`.
pub(super) fn start_retry_after_anthropic_server(
    retry_after_secs: u64,
) -> (
    String,
    std_mpsc::Receiver<std::time::Instant>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind retry-after server");
    let address = listener.local_addr().expect("retry-after server addr");
    let (instant_tx, instant_rx) = std_mpsc::channel();

    let handle = thread::spawn(move || {
        let mut messages_requests = 0_usize;
        loop {
            let (mut stream, _) = listener.accept().expect("accept retry-after request");
            let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
            let request = read_test_http_request(&mut stream);
            if is_anthropic_count_tokens_request(&request) {
                write_anthropic_count_tokens_response(&mut stream);
                continue;
            }

            messages_requests += 1;
            instant_tx
                .send(std::time::Instant::now())
                .expect("record messages attempt instant");
            match messages_requests {
                1 => write_anthropic_retry_after_response(&mut stream, retry_after_secs),
                2 => {
                    write_anthropic_text_sse_response(
                        &mut stream,
                        "answer after retry-after backoff",
                    );
                    return;
                }
                _ => panic!("unexpected extra messages request to retry-after server"),
            }
        }
    });

    (format!("http://{address}"), instant_rx, handle)
}

fn write_anthropic_retry_after_response(stream: &mut std::net::TcpStream, retry_after_secs: u64) {
    let body = r#"{"error":{"message":"rate limited"}}"#;
    let response = format!(
        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: {retry_after_secs}\r\nanthropic-ratelimit-unified-status: rejected\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write Anthropic 429 retry-after response");
    let _ = stream.flush();
}

fn write_anthropic_http_error_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    message: &str,
) {
    let body = format!(r#"{{"error":{{"message":"{message}"}}}}"#);
    let response = format!(
        "HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write Anthropic error response");
    let _ = stream.flush();
}

fn write_anthropic_text_sse_response(stream: &mut std::net::TcpStream, text: &str) {
    let escaped_text = serde_json::to_string(text).expect("serialize Anthropic text");
    let body = format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":2,\"output_tokens\":0}}}}}}\n\n",
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        escaped_text,
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write Anthropic text response");
    let _ = stream.flush();
}

fn write_openai_sse_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write OpenAI stream response");
    let _ = stream.flush();
}

pub(super) fn start_openai_text_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind openai text server");
    let address = listener.local_addr().expect("openai text server addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept openai text request");
        let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"openai \"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"live\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\n\n",
            "data: [DONE]\n\n",
        );
        write_openai_sse_response(&mut stream, body);
    });
    (format!("http://{address}"), handle)
}

pub(super) fn start_hanging_openai_server() -> (String, std_mpsc::Sender<()>, thread::JoinHandle<()>)
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind hanging openai server");
    listener
        .set_nonblocking(true)
        .expect("set hanging openai server nonblocking");
    let address = listener.local_addr().expect("hanging openai addr");
    let (shutdown_tx, shutdown_rx) = std_mpsc::channel();

    let handle = thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // See the count-tokens server: force blocking so the
                    // accepted socket does not inherit the listener's
                    // non-blocking flag on BSD/macOS.
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request);
                    stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                            )
                            .expect("write openai hanging response headers");
                    let keepalive = b": keepalive\n\n";
                    let chunk_prefix = format!("{:X}\r\n", keepalive.len());
                    stream
                        .write_all(chunk_prefix.as_bytes())
                        .expect("write openai keepalive chunk prefix");
                    stream.write_all(keepalive).expect("write openai keepalive");
                    stream
                        .write_all(b"\r\n")
                        .expect("write openai keepalive chunk suffix");
                    let _ = stream.flush();
                    let _ = shutdown_rx.recv_timeout(StdDuration::from_secs(2));
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    thread::sleep(StdDuration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (format!("http://{address}"), shutdown_tx, handle)
}

pub(super) fn start_openai_tool_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind openai tool server");
    let address = listener.local_addr().expect("openai tool server addr");
    let handle = thread::spawn(move || {
        let first_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"printf ok\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\n\n",
            "data: [DONE]\n\n",
        );
        let second_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"tool done\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );

        for body in [first_body, second_body] {
            let (mut stream, _) = listener.accept().expect("accept openai tool request");
            let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            write_openai_sse_response(&mut stream, body);
        }
    });
    (format!("http://{address}"), handle)
}

pub(super) fn start_openai_http_error_server(status: u16) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind openai error server");
    let address = listener.local_addr().expect("openai error server addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept openai error request");
        let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = r#"{"error":{"message":"openai overloaded"}}"#;
        let response = format!(
            "HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body,
        );
        stream
            .write_all(response.as_bytes())
            .expect("write openai error response");
        let _ = stream.flush();
    });
    (format!("http://{address}"), handle)
}

pub(super) fn start_openai_error_after_content_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind openai stream error server");
    let address = listener.local_addr().expect("openai stream error addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("accept openai stream error request");
        let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"before error\"},\"finish_reason\":null}]}\n\n",
            "data: {not-json}\n\n",
        );
        write_openai_sse_response(&mut stream, body);
    });
    (format!("http://{address}"), handle)
}
