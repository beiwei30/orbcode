//! ACP (Agent Client Protocol) adapter backed by the official Rust SDK.
//!
//! This module owns the production `orbcode acp` path. It translates ACP SDK
//! requests to the canonical app-server protocol through `AppClient`; it does
//! not call core, tools, providers, MCP, or session-store internals directly.

mod capabilities;
mod mcp_setup;
mod replay;
mod server_requests;
mod sessions;
mod tool_updates;

use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::schema::{
    CancelNotification, CloseSessionRequest, ContentBlock, ContentChunk, DeleteSessionRequest,
    InitializeRequest, ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo, Error, Lines, Result, Role};
use orbcode_app_server::AppServer;
use orbcode_app_server_client::AppClient;
use orbcode_app_server_protocol::{RequestId, ResponseResult, ServerRequestEnvelope, StreamEvent};
use orbcode_protocol::format_tool_title;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use capabilities::initialize_response;
use server_requests::{deny_pending_server_requests, server_request_pump};
use sessions::{
    handle_close_session, handle_delete_session, handle_list_sessions, handle_load_session,
    handle_new_session, handle_prompt, handle_resume_session,
};
use tool_updates::{
    extract_progress_title, send_agent_text, send_session_update, tool_call_started,
    tool_completion_update, tool_progress_update,
};

const ACP_ALLOW_ONCE: &str = "allow_once";
const ACP_DENY_ONCE: &str = "reject_once";
const ACP_TRUST_MCP: &str = "trust_mcp_server";
const ACP_DENY_MCP: &str = "reject_mcp_server";
const ACP_ASK_OPTION_PREFIX: &str = "ask_user_option_";

struct AcpSdkState {
    client: Arc<AppClient>,
    launch_cwd: PathBuf,
    sessions: Mutex<HashMap<String, AcpSessionState>>,
    pending_server_requests: Mutex<HashMap<String, Vec<PendingServerRequest>>>,
    server_request_rx: Mutex<Option<mpsc::Receiver<ServerRequestEnvelope>>>,
}

#[derive(Default)]
struct AcpSessionState {
    active_prompt_generation: Option<u64>,
    next_prompt_generation: u64,
}

struct PendingServerRequest {
    id: RequestId,
    result: ResponseResult,
    cancel_tx: oneshot::Sender<()>,
}

struct EofAwareStdio {
    eof_tx: oneshot::Sender<()>,
}

impl EofAwareStdio {
    fn new() -> (Self, oneshot::Receiver<()>) {
        let (eof_tx, eof_rx) = oneshot::channel();
        (Self { eof_tx }, eof_rx)
    }
}

impl<Counterpart: Role> ConnectTo<Counterpart> for EofAwareStdio {
    async fn connect_to(self, client: impl ConnectTo<Counterpart::Counterpart>) -> Result<()> {
        let (line_tx, line_rx) = futures::channel::mpsc::unbounded::<std::io::Result<String>>();
        let eof_tx = self.eof_tx;
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let should_stop = line.is_err();
                if line_tx.unbounded_send(line).is_err() || should_stop {
                    break;
                }
            }
            let _ = eof_tx.send(());
        });

        let outgoing = futures::sink::unfold(
            tokio::io::stdout(),
            async move |mut stdout, line: String| {
                stdout.write_all(line.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                Ok::<_, std::io::Error>(stdout)
            },
        );

        ConnectTo::<Counterpart>::connect_to(Lines::new(outgoing, line_rx), client).await
    }
}

impl AcpSdkState {
    fn new(
        client: Arc<AppClient>,
        server_request_rx: Option<mpsc::Receiver<ServerRequestEnvelope>>,
    ) -> Self {
        let launch_cwd = std::env::current_dir().map_or_else(
            |_| PathBuf::from("."),
            |cwd| cwd.canonicalize().unwrap_or(cwd),
        );
        Self {
            client,
            launch_cwd,
            sessions: Mutex::new(HashMap::new()),
            pending_server_requests: Mutex::new(HashMap::new()),
            server_request_rx: Mutex::new(server_request_rx),
        }
    }
}

pub(crate) async fn run_acp_adapter(app_server: AppServer) -> anyhow::Result<()> {
    let client = Arc::new(
        AppClient::new(app_server)
            .await
            .map_err(|e| anyhow::anyhow!("protocol init: {e}"))?,
    );
    let server_request_rx = client.take_server_request_receiver().await;
    let state = Arc::new(AcpSdkState::new(Arc::clone(&client), server_request_rx));
    let (stdio, stdin_eof_rx) = EofAwareStdio::new();

    let result = Agent
        .builder()
        .name("orbcode")
        .on_receive_request(
            async move |initialize: InitializeRequest, responder, _connection| {
                responder.respond(initialize_response(initialize))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: NewSessionRequest, responder, _connection| {
                    handle_new_session(Arc::clone(&state), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: ListSessionsRequest, responder, _connection| {
                    handle_list_sessions(Arc::clone(&state), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: LoadSessionRequest, responder, connection| {
                    handle_load_session(Arc::clone(&state), request, responder, connection).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: ResumeSessionRequest, responder, _connection| {
                    handle_resume_session(Arc::clone(&state), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: DeleteSessionRequest, responder, _connection| {
                    handle_delete_session(Arc::clone(&state), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: PromptRequest, responder, connection| {
                    handle_prompt(Arc::clone(&state), request, responder, connection).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: CloseSessionRequest, responder, _connection| {
                    handle_close_session(Arc::clone(&state), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = Arc::clone(&state);
                async move |notification: CancelNotification, _connection| {
                    handle_cancel(Arc::clone(&state), notification).await
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(stdio, {
            let state = Arc::clone(&state);
            async move |_connection| {
                let _ = stdin_eof_rx.await;
                cleanup_all_sessions(&state).await;
                Ok(())
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("ACP SDK transport error: {e}"));
    if result.is_err() {
        cleanup_all_sessions(&state).await;
    }
    result
}

async fn cleanup_all_sessions(state: &AcpSdkState) {
    let session_ids = {
        let mut sessions = state.sessions.lock().await;
        let session_ids = sessions.keys().cloned().collect::<Vec<_>>();
        sessions.clear();
        session_ids
    };

    for session_id in session_ids {
        if let Err(err) = state.client.cancel_turn(&session_id).await {
            tracing::warn!(%session_id, %err, "ACP stdio cleanup cancel failed");
        }
        deny_pending_server_requests(state, &session_id).await;
        if let Err(err) = state.client.acp_close_session(&session_id).await {
            tracing::warn!(%session_id, %err, "ACP stdio cleanup failed");
        }
    }
}

async fn handle_cancel(state: Arc<AcpSdkState>, notification: CancelNotification) -> Result<()> {
    let session_id = notification.session_id.to_string();
    if let Err(err) = state.client.cancel_turn(&session_id).await {
        tracing::warn!(%session_id, %err, "ACP session/cancel failed");
    }
    deny_pending_server_requests(&state, &session_id).await;
    Ok(())
}

async fn prompt_response_loop(
    session_id: &str,
    event_rx: &mut mpsc::UnboundedReceiver<StreamEvent>,
    connection: ConnectionTo<Client>,
) -> std::result::Result<StopReason, Error> {
    let mut last_progress_titles: HashMap<String, String> = HashMap::new();
    while let Some(event) = event_rx.recv().await {
        match event {
            StreamEvent::AssistantDelta { delta, .. } => {
                send_agent_text(&connection, session_id, delta)?;
            }
            StreamEvent::ThinkingDelta { delta, .. } => {
                let update =
                    SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(delta)));
                connection
                    .send_notification(SessionNotification::new(session_id.to_string(), update))?;
            }
            StreamEvent::ToolUseStarted {
                tool_use_id,
                tool_name,
                tool_input,
                ..
            } => {
                last_progress_titles.insert(
                    tool_use_id.clone(),
                    format_tool_title(&tool_name, &tool_input),
                );
                send_session_update(
                    &connection,
                    session_id,
                    SessionUpdate::ToolCall(tool_call_started(
                        &tool_use_id,
                        &tool_name,
                        &tool_input,
                    )),
                )?;
            }
            StreamEvent::ToolProgress {
                tool_use_id,
                tool_name,
                progress,
                ..
            } => {
                let cached = last_progress_titles.get(&tool_use_id).cloned();
                let update = tool_progress_update(
                    &tool_use_id,
                    &tool_name,
                    progress.clone(),
                    cached.as_deref(),
                );
                if let Some(title) = extract_progress_title(&progress) {
                    last_progress_titles.insert(tool_use_id.clone(), title);
                }
                send_session_update(
                    &connection,
                    session_id,
                    SessionUpdate::ToolCallUpdate(update),
                )?;
            }
            StreamEvent::ToolUseCompleted {
                tool_use_id, kind, ..
            } => {
                last_progress_titles.remove(&tool_use_id);
                send_session_update(
                    &connection,
                    session_id,
                    SessionUpdate::ToolCallUpdate(tool_completion_update(&tool_use_id, kind)),
                )?;
            }
            StreamEvent::TurnFinished { .. } => return Ok(StopReason::EndTurn),
            StreamEvent::TurnCancelled { .. } => return Ok(StopReason::Cancelled),
            StreamEvent::Budget { blocked: true, .. } => return Ok(StopReason::MaxTokens),
            StreamEvent::Error { message, .. } => {
                send_agent_text(&connection, session_id, format!("Error: {message}"))?;
                return Ok(StopReason::Refusal);
            }
            _ => {}
        }
    }

    Err(internal_error(
        "session/prompt stream ended before terminal event",
    ))
}

async fn finish_prompt_generation(state: &AcpSdkState, session_id: &str, generation: u64) {
    let mut sessions = state.sessions.lock().await;
    let Some(session) = sessions.get_mut(session_id) else {
        return;
    };
    if session.active_prompt_generation == Some(generation) {
        session.active_prompt_generation = None;
    }
}

async fn start_server_request_pump(state: &Arc<AcpSdkState>, connection: ConnectionTo<Client>) {
    let Some(rx) = state.server_request_rx.lock().await.take() else {
        return;
    };
    // Detached ACP server-request pump; request cancellation is tracked in
    // pending-request state.
    let _server_request_pump_handle =
        tokio::spawn(server_request_pump(rx, Arc::clone(state), connection));
}

fn invalid_params(message: impl ToString) -> Error {
    Error::invalid_params().data(message.to_string())
}

fn internal_error(message: impl ToString) -> Error {
    agent_client_protocol::util::internal_error(message)
}
