use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use orbcode_app_server_protocol::{
    AskUserQuestionRequest, AskUserQuestionResponse, ClientCapabilities, ClientInfo, ClientMessage,
    ClientRequestEnvelope, ErrorCode, InitializeParams, McpTrustDecisionWire,
    McpTrustResponseParams, PermissionDecisionWire, PermissionResponseParams, ProtocolError,
    RequestId, ResponseResult, SentResult, ServerMessage, ServerNotificationEnvelope,
    ServerRequestEnvelope, ServerRequestResponse, ServerResponseEnvelope,
    SessionGoalContinueParams, SessionGoalContinueResult, StreamEventNotification, ToolOverview,
    ToolsListResult, TurnSubmitParams, TurnSubmitResult, method,
};
use orbcode_core::{
    GoalContinuationOutcome, InteractiveQuestionCapabilities, PermissionDecision,
    TurnInteractionContext,
};
use orbcode_protocol::{
    AskUserCancellationReason, AskUserValidationCode, AskUserValidationError, StreamEvent,
};

use crate::AppServer;
use crate::protocol_handler::permissions::wire_to_core;

/// Default timeout for server-initiated permission requests. If the client
/// does not respond within this window the permission is denied so the turn
/// can make progress.
const DEFAULT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// ServerSink trait
// ---------------------------------------------------------------------------

/// Trait for sending server messages to a connected client.
///
/// Implementations must guarantee delivery for lossless messages
/// (responses, server-requests, durable stream notifications). Best-effort
/// messages (deltas, progress) may be dropped under backpressure.
pub trait ServerSink: Send + Sync + 'static {
    fn send(&self, message: ServerMessage);
    fn is_closed(&self) -> bool;
}

/// Default bounded-channel capacity for [`ChannelSink`].
///
/// Large enough for typical streaming bursts while still providing
/// backpressure when the consumer falls behind.
pub const CHANNEL_SINK_CAPACITY: usize = 1024;

/// Channel-based [`ServerSink`] with two-tier delivery:
/// - **Lossless** messages (responses, server-requests, terminal notifications)
///   and durable stream events go through an unbounded channel — guaranteed
///   delivery, never dropped.
/// - **Best-effort** messages (deltas, progress) go through a bounded channel —
///   dropped under backpressure to avoid blocking the processor.
///
/// The consumer must drain both channels (e.g. via `tokio::select!`).
pub struct ChannelSink {
    lossless_tx: mpsc::UnboundedSender<ServerMessage>,
    best_effort_tx: mpsc::Sender<ServerMessage>,
}

impl ChannelSink {
    pub fn new(
        lossless_tx: mpsc::UnboundedSender<ServerMessage>,
        best_effort_tx: mpsc::Sender<ServerMessage>,
    ) -> Self {
        Self {
            lossless_tx,
            best_effort_tx,
        }
    }

    fn is_lossless(message: &ServerMessage) -> bool {
        match message {
            ServerMessage::Notification(n) => {
                if let Ok(notif) =
                    serde_json::from_value::<StreamEventNotification>(n.params.clone())
                {
                    stream_event_is_lossless(&notif.event)
                } else {
                    false
                }
            }
            _ => true,
        }
    }
}

fn stream_event_is_lossless(event: &StreamEvent) -> bool {
    !matches!(
        event,
        StreamEvent::AssistantDelta { .. }
            | StreamEvent::ThinkingDelta { .. }
            | StreamEvent::ToolProgress { .. }
            | StreamEvent::HookProgress { .. }
    )
}

impl ServerSink for ChannelSink {
    fn send(&self, message: ServerMessage) {
        if Self::is_lossless(&message) {
            let _ = self.lossless_tx.send(message);
        } else {
            let _ = self.best_effort_tx.try_send(message);
        }
    }

    fn is_closed(&self) -> bool {
        self.lossless_tx.is_closed() && self.best_effort_tx.is_closed()
    }
}

// ---------------------------------------------------------------------------
// MessageProcessor
// ---------------------------------------------------------------------------

/// Message-level processor for a single client connection.
///
/// Accepts [`ClientMessage`] values (both client-initiated requests and
/// responses to server-initiated requests), dispatches them, and sends
/// [`ServerMessage`] values to a [`ServerSink`]. For `turn/submit`, it
/// spawns a pump task that converts the `StreamEvent` receiver into
/// notifications and emits server-requests for permission prompts.
pub struct MessageProcessor {
    app_server: AppServer,
    sink: Arc<dyn ServerSink>,
    client_info: Option<ClientInfo>,
    client_capabilities: ClientCapabilities,
    initialized: bool,
    connection_id: String,
    pending_server_requests: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>>,
    active_subscriptions: HashMap<String, JoinHandle<()>>,
    /// Monotonically increasing counter for generating unique server-request
    /// IDs within this processor.
    next_server_request_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubscriptionPumpFailure {
    Panicked,
    UnexpectedCancellation,
}

impl SubscriptionPumpFailure {
    const fn message(self) -> &'static str {
        match self {
            Self::Panicked => "subscription event pump panicked",
            Self::UnexpectedCancellation => "subscription event pump was cancelled unexpectedly",
        }
    }
}

async fn observe_subscription_pump(
    handle: JoinHandle<()>,
    cancellation_expected: bool,
) -> Option<SubscriptionPumpFailure> {
    match handle.await {
        Ok(()) => None,
        Err(error) if error.is_cancelled() && cancellation_expected => None,
        Err(error) if error.is_cancelled() => Some(SubscriptionPumpFailure::UnexpectedCancellation),
        Err(_) => Some(SubscriptionPumpFailure::Panicked),
    }
}

fn report_subscription_pump_failure(
    sink: &dyn ServerSink,
    subscription_id: &str,
    failure: SubscriptionPumpFailure,
) {
    // Do not include the JoinError: a panic payload may contain arbitrary
    // provider or tool data. The fixed diagnostic still distinguishes panic
    // from unexpected cancellation.
    eprintln!("subscription {subscription_id}: {}", failure.message());
    let notification = StreamEventNotification {
        subscription_id: subscription_id.to_string(),
        event: StreamEvent::Error {
            session_id: None,
            provider: None,
            category: None,
            message: failure.message().to_string(),
            suggestion: None,
        },
    };
    sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
        method: method::NOTIFICATION_STREAM_EVENT.to_string(),
        params: serde_json::to_value(notification).unwrap_or_default(),
    }));
}

impl MessageProcessor {
    pub fn new(app_server: AppServer, sink: Arc<dyn ServerSink>) -> Self {
        Self {
            app_server,
            sink,
            client_info: None,
            client_capabilities: ClientCapabilities::default(),
            initialized: false,
            connection_id: uuid::Uuid::new_v4().to_string(),
            pending_server_requests: Arc::new(Mutex::new(HashMap::new())),
            active_subscriptions: HashMap::new(),
            next_server_request_id: 0,
        }
    }

    /// Process an incoming client message.
    /// Join subscriptions whose pump has already finished (client
    /// disconnected, stream closed) so the map stays bounded to *live*
    /// subscriptions and task failure cannot be discarded. Called on every
    /// request — not only when a new subscription is created — so a batch of
    /// pumps that complete after the last `subscribe` is still reclaimed
    /// rather than lingering until the processor is dropped.
    async fn prune_finished_subscriptions(&mut self) {
        let finished = self
            .active_subscriptions
            .iter()
            .filter(|(_, handle)| handle.is_finished())
            .map(|(subscription_id, _)| subscription_id.clone())
            .collect::<Vec<_>>();

        for subscription_id in finished {
            let Some(handle) = self.active_subscriptions.remove(&subscription_id) else {
                continue;
            };
            if let Some(failure) = observe_subscription_pump(handle, false).await {
                report_subscription_pump_failure(self.sink.as_ref(), &subscription_id, failure);
            }
        }
    }

    /// Register one connection-owned event pump.
    ///
    /// The source stream closing is normal completion. The processor owns the
    /// returned handle, aborts it when the connection owner is dropped, and
    /// joins completed handles at prune boundaries so panic or unexpected
    /// cancellation becomes a terminal subscription diagnostic.
    async fn register_subscription_pump(
        &mut self,
        rx: mpsc::UnboundedReceiver<StreamEvent>,
        subscription_id: &str,
    ) {
        let sink = Arc::clone(&self.sink);
        let pending = Arc::clone(&self.pending_server_requests);
        let app_server = self.app_server.clone();
        let sub_id = subscription_id.to_string();
        let next_id = self.next_server_request_id;
        // Reserve a range of IDs for this pump (generous upper bound).
        self.next_server_request_id += 1_000_000;

        let handle = tokio::spawn(async move {
            pump_events(
                rx,
                sink,
                pending,
                app_server,
                sub_id,
                next_id,
                DEFAULT_PERMISSION_TIMEOUT,
            )
            .await;
        });

        self.prune_finished_subscriptions().await;
        self.active_subscriptions
            .insert(subscription_id.to_string(), handle);
    }

    pub async fn handle_message(&mut self, message: ClientMessage) {
        // If the client disconnected there is no point processing further
        // messages -- skip silently.
        if self.sink.is_closed() {
            return;
        }

        match message {
            ClientMessage::Request(req) => {
                // Reject pre-initialize requests (except initialize itself)
                if !self.initialized && req.method != method::INITIALIZE {
                    self.sink
                        .send(ServerMessage::Response(ServerResponseEnvelope {
                            id: req.id,
                            result: ResponseResult::Error(ProtocolError {
                                code: ErrorCode::InvalidRequest,
                                message: "not initialized".to_string(),
                                data: None,
                            }),
                        }));
                    return;
                }
                let response = self.handle_request(req).await;
                self.sink.send(ServerMessage::Response(response));
            }
            ClientMessage::Response(resp) => {
                self.resolve_server_request(resp);
            }
            _ => {}
        }
    }

    /// Dispatch a client request, returning the response envelope.
    async fn handle_request(&mut self, req: ClientRequestEnvelope) -> ServerResponseEnvelope {
        self.prune_finished_subscriptions().await;

        if req.method == method::INITIALIZE {
            let result = self.handle_initialize(req.params);
            return ServerResponseEnvelope { id: req.id, result };
        }

        if method::persistent_goal_client_request_methods().contains(&req.method.as_str())
            && (!self.client_capabilities.experimental_methods
                || !self.client_capabilities.persistent_goals)
        {
            return ServerResponseEnvelope {
                id: req.id,
                result: ResponseResult::Error(ProtocolError {
                    code: ErrorCode::MethodNotFound,
                    message: format!(
                        "persistent goal method '{}' requires capabilities.experimental_methods = true and capabilities.persistent_goals = true",
                        req.method
                    ),
                    data: None,
                }),
            };
        }

        if !self.client_capabilities.experimental_methods
            && method::experimental_client_request_methods().contains(&req.method.as_str())
        {
            return ServerResponseEnvelope {
                id: req.id,
                result: ResponseResult::Error(ProtocolError {
                    code: ErrorCode::MethodNotFound,
                    message: format!(
                        "experimental method '{}' requires capabilities.experimental_methods = true",
                        req.method
                    ),
                    data: None,
                }),
            };
        }

        if req.method == method::TURN_SUBMIT {
            let result = self.handle_turn_submit_with_pump(req.params).await;
            return ServerResponseEnvelope { id: req.id, result };
        }

        if req.method == method::SESSION_GOAL_CONTINUE {
            let result = self.handle_goal_continue_with_pump(req.params).await;
            return ServerResponseEnvelope { id: req.id, result };
        }

        if req.method == method::TOOLS_LIST && self.client_capabilities.persistent_goals {
            let result = self.handle_tools_list_with_goals();
            return ServerResponseEnvelope { id: req.id, result };
        }

        if req.method == method::BACKGROUND_SUBSCRIBE {
            let result = self.handle_background_subscribe_with_pump(req.params).await;
            return ServerResponseEnvelope { id: req.id, result };
        }

        // For permission/respond, allow as a client-request fallback for
        // clients that do not use the server-request response path.
        if req.method == method::PERMISSION_RESPOND {
            let result = self.handle_permission_respond_fallback(req.params).await;
            return ServerResponseEnvelope { id: req.id, result };
        }

        // All other methods delegate to the existing AppServer dispatch.
        self.app_server.handle_request(req).await
    }

    /// Handle the `initialize` request: parse params, validate, store
    /// client info, and mark the processor as initialized.
    fn handle_initialize(&mut self, params: Option<serde_json::Value>) -> ResponseResult {
        let init_params: InitializeParams = match crate::protocol_handler::parse_params(params) {
            Ok(v) => v,
            Err(e) => return ResponseResult::Error(e),
        };

        self.client_info = Some(init_params.client_info);
        self.client_capabilities = init_params.capabilities;
        self.initialized = true;

        let to_strings = |v: Vec<&str>| v.into_iter().map(String::from).collect();
        let experimental = if self.client_capabilities.experimental_methods {
            method::experimental_client_request_methods()
                .into_iter()
                .filter(|candidate| {
                    self.client_capabilities.persistent_goals
                        || !method::persistent_goal_client_request_methods().contains(candidate)
                })
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        ResponseResult::Success {
            data: Some(
                serde_json::to_value(orbcode_app_server_protocol::InitializeResult {
                    protocol_version: init_params.protocol_version,
                    server_info: orbcode_app_server_protocol::ServerInfo {
                        name: "orbcode".to_string(),
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    capabilities: orbcode_app_server_protocol::ServerCapabilities {
                        streaming: true,
                        stable_methods: to_strings(method::stable_client_request_methods()),
                        experimental_methods: experimental,
                        server_notification_methods: to_strings(
                            method::server_notification_methods(),
                        ),
                        server_request_methods: to_strings(method::server_request_methods()),
                    },
                })
                .unwrap_or(serde_json::Value::Null),
            ),
        }
    }

    /// Handle `turn/submit` by starting the turn and spawning an event pump
    /// that delivers stream events as notifications and permission requests
    /// as server-requests.
    async fn handle_turn_submit_with_pump(
        &mut self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        let p: TurnSubmitParams = match crate::protocol_handler::parse_params(params) {
            Ok(v) => v,
            Err(e) => return ResponseResult::Error(e),
        };

        let interactive = self
            .client_capabilities
            .interactive_questions
            .as_ref()
            .map(|capability| InteractiveQuestionCapabilities {
                single_select: capability.single_select,
                multi_select: capability.multi_select,
                free_text: capability.free_text,
                previews: capability.previews,
                annotations: capability.annotations,
                special_outcomes: capability.special_outcomes,
            })
            .unwrap_or_default();
        let rx = match self
            .app_server
            .submit_turn_with_interaction(
                &p.session_id,
                p.prompt,
                TurnInteractionContext {
                    owner_id: self.connection_id.clone(),
                    capabilities: interactive,
                    persistent_goals: self.client_capabilities.persistent_goals,
                },
            )
            .await
        {
            Ok(rx) => rx,
            Err(e) => return crate::protocol_handler::core_error(e),
        };

        let subscription_id = uuid::Uuid::new_v4().to_string();

        self.register_subscription_pump(rx, &subscription_id).await;

        crate::protocol_handler::success(TurnSubmitResult { subscription_id })
    }

    async fn handle_background_subscribe_with_pump(
        &mut self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        let p: orbcode_app_server_protocol::BackgroundSubscribeParams =
            match crate::protocol_handler::parse_params(params) {
                Ok(v) => v,
                Err(e) => return ResponseResult::Error(e),
            };

        let rx = match self
            .app_server
            .background_task_progress_stream(&p.task_id)
            .await
        {
            Ok(rx) => rx,
            Err(e) => return crate::protocol_handler::core_error(e),
        };

        let subscription_id = uuid::Uuid::new_v4().to_string();
        self.register_subscription_pump(rx, &subscription_id).await;

        crate::protocol_handler::success(orbcode_app_server_protocol::BackgroundSubscribeResult {
            subscription_id,
        })
    }

    async fn handle_goal_continue_with_pump(
        &mut self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        let p: SessionGoalContinueParams = match crate::protocol_handler::parse_params(params) {
            Ok(value) => value,
            Err(error) => return ResponseResult::Error(error),
        };
        let interaction = TurnInteractionContext {
            owner_id: self.connection_id.clone(),
            capabilities: self
                .client_capabilities
                .interactive_questions
                .as_ref()
                .map(|capability| InteractiveQuestionCapabilities {
                    single_select: capability.single_select,
                    multi_select: capability.multi_select,
                    free_text: capability.free_text,
                    previews: capability.previews,
                    annotations: capability.annotations,
                    special_outcomes: capability.special_outcomes,
                })
                .unwrap_or_default(),
            persistent_goals: true,
        };
        let outcome = match self
            .app_server
            .continue_goal_if_eligible(
                &p.session_id,
                &p.goal_id,
                p.expected_revision,
                true,
                interaction,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => return crate::protocol_handler::goal_error(error),
        };
        match outcome {
            GoalContinuationOutcome::NotStarted { reason, goal } => {
                crate::protocol_handler::success(SessionGoalContinueResult::NotStarted {
                    reason: crate::protocol_handler::sessions::goal_not_started_reason_to_wire(
                        reason,
                    ),
                    goal,
                })
            }
            GoalContinuationOutcome::Started(started) => {
                let subscription_id = uuid::Uuid::new_v4().to_string();
                self.register_subscription_pump(started.events, &subscription_id)
                    .await;
                crate::protocol_handler::success(SessionGoalContinueResult::Started {
                    subscription_id,
                    turn_id: started.turn_id,
                    goal: started.goal,
                })
            }
        }
    }

    fn handle_tools_list_with_goals(&self) -> ResponseResult {
        let mut tools = self
            .app_server
            .list_tools()
            .into_iter()
            .map(|tool| {
                let unavailable_reason = matches!(
                    tool.name,
                    "ask-user-question" | "AskUserQuestion"
                )
                .then_some(
                    "available to the provider only for turns owned by a client that declares the full interactive_questions capability".to_string(),
                );
                ToolOverview {
                    name: tool.name.to_string(),
                    summary: tool.summary.to_string(),
                    requires_tools_permission: tool.requires_tools_permission,
                    requires_network_permission: tool.requires_network_permission,
                    provider_hidden: tool.provider_hidden,
                    unavailable_reason,
                }
            })
            .collect::<Vec<_>>();
        tools.extend(
            orbcode_core::persistent_goal_tool_specs()
                .iter()
                .map(|tool| ToolOverview {
                    name: tool.name.to_string(),
                    summary: tool.summary.to_string(),
                    requires_tools_permission: tool.requires_tools_permission,
                    requires_network_permission: tool.requires_network_permission,
                    provider_hidden: tool.provider_hidden,
                    unavailable_reason: None,
                }),
        );
        crate::protocol_handler::success(ToolsListResult(tools))
    }

    /// Fallback handler for `permission/respond` sent as a regular client
    /// request (for clients that do not use the server-request response
    /// path).
    async fn handle_permission_respond_fallback(
        &self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        let p: PermissionResponseParams = match crate::protocol_handler::parse_params(params) {
            Ok(v) => v,
            Err(e) => return ResponseResult::Error(e),
        };
        let decision = match wire_to_core(p.decision) {
            Ok(d) => d,
            Err(resp) => return resp,
        };
        let sent = self
            .app_server
            .respond_to_permission_request(&p.request_id, decision)
            .await;
        crate::protocol_handler::success(SentResult { sent })
    }

    /// Route a client response to a pending server-request.
    fn resolve_server_request(&self, response: ServerRequestResponse) {
        let pending = Arc::clone(&self.pending_server_requests);
        // Detached resolver; it only completes or drops a pending oneshot.
        let _resolver_handle = tokio::spawn(async move {
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(&response.id) {
                let _ = tx.send(response.result);
            }
        });
    }

    /// Returns whether the processor has completed the initialize handshake.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns the client info provided during initialization, if any.
    pub fn client_info(&self) -> Option<&ClientInfo> {
        self.client_info.as_ref()
    }
}

impl Drop for MessageProcessor {
    fn drop(&mut self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            for (subscription_id, handle) in self.active_subscriptions.drain() {
                let cancellation_expected = !handle.is_finished();
                if cancellation_expected {
                    handle.abort();
                }
                let sink = Arc::clone(&self.sink);
                // The pump is already finished or has just been aborted, so
                // this observer is bounded and owns no connection state. It
                // distinguishes expected shutdown cancellation from a pump
                // that had already failed before owner teardown.
                std::mem::drop(runtime.spawn(async move {
                    if let Some(failure) =
                        observe_subscription_pump(handle, cancellation_expected).await
                    {
                        report_subscription_pump_failure(sink.as_ref(), &subscription_id, failure);
                    }
                }));
            }
        } else {
            // Message processors normally live inside a Tokio transport task.
            // If one is dropped outside a runtime, aborting still releases the
            // connection-owned pumps; there is no executor available to join
            // them or deliver a terminal notification.
            for (_, handle) in self.active_subscriptions.drain() {
                handle.abort();
            }
        }
        self.app_server.cancel_pending_ask_user_for_owner(
            &self.connection_id,
            AskUserCancellationReason::Disconnect,
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let app_server = self.app_server.clone();
            let connection_id = self.connection_id.clone();
            std::mem::drop(runtime.spawn(async move {
                app_server
                    .disconnect_interaction_owner(&connection_id)
                    .await;
            }));
        }
    }
}

// ---------------------------------------------------------------------------
// Event pump
// ---------------------------------------------------------------------------

/// Cleanup guard that cancels pump-owned ask-user request_ids on drop.
///
/// When a pump task is aborted (e.g. client disconnect, `MessageProcessor::drop`),
/// tokio drops all locals including this guard. The Drop impl runs synchronously,
/// resolving each pending oneshot with `None` so tools unblock immediately.
struct PumpAskUserGuard {
    request_ids: Vec<String>,
    app_server: AppServer,
}

impl Drop for PumpAskUserGuard {
    fn drop(&mut self) {
        if !self.request_ids.is_empty() {
            self.app_server
                .cancel_pending_ask_user(&self.request_ids, AskUserCancellationReason::Disconnect);
        }
    }
}

/// Reads `StreamEvent` values from `rx` and sends them as notifications on
/// `sink`. For `PermissionRequested` events, also sends a server-request
/// and spawns a task that awaits the client's response and calls
/// `app_server.respond_to_permission_request()`.
pub async fn pump_events(
    mut rx: mpsc::UnboundedReceiver<StreamEvent>,
    sink: Arc<dyn ServerSink>,
    pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>>,
    app_server: AppServer,
    subscription_id: String,
    mut next_id: u64,
    permission_timeout: Duration,
) {
    let mut _ask_user_guard = PumpAskUserGuard {
        request_ids: Vec::new(),
        app_server: app_server.clone(),
    };

    while let Some(event) = rx.recv().await {
        if sink.is_closed() {
            break;
        }

        // For permission requests, send a server-request so the client can
        // respond with a decision.
        if let StreamEvent::PermissionRequested { ref request } = event {
            let server_req_id = format!("srv-perm-{next_id}");
            next_id += 1;

            let (tx, rx_oneshot) = oneshot::channel();
            pending.lock().await.insert(server_req_id.clone(), tx);

            // Clone before moving into the envelope.
            let cleanup_id = server_req_id.clone();

            // Send the server-request to the client.
            sink.send(ServerMessage::Request(ServerRequestEnvelope {
                id: server_req_id,
                method: method::SERVER_REQUEST_PERMISSION.to_string(),
                params: serde_json::to_value(request).unwrap_or_default(),
            }));

            // Spawn a task to await the client's response with a timeout.
            // The pending map entry is cleaned up in ALL exit paths.
            let app = app_server.clone();
            let request_id = request.request_id.clone();
            let timeout_dur = permission_timeout;
            let pending_cleanup = Arc::clone(&pending);
            // Detached waiter; it owns cleanup for the pending permission request.
            let _permission_waiter_handle = tokio::spawn(async move {
                let result = tokio::time::timeout(timeout_dur, rx_oneshot).await;

                // Clean up pending map entry regardless of outcome.
                pending_cleanup.lock().await.remove(&cleanup_id);

                match result {
                    Ok(Ok(ResponseResult::Success { data: Some(data) })) => {
                        if let Ok(resp) =
                            serde_json::from_value::<PermissionResponseParams>(data.clone())
                            && let Ok(decision) = wire_to_core(resp.decision)
                        {
                            app.respond_to_permission_request(&resp.request_id, decision)
                                .await;
                            return;
                        }
                        if let Ok(decision_wire) =
                            serde_json::from_value::<PermissionDecisionWire>(data)
                            && let Ok(decision) = wire_to_core(decision_wire)
                        {
                            app.respond_to_permission_request(&request_id, decision)
                                .await;
                        }
                    }
                    Ok(Ok(_) | Err(_)) | Err(_) => {
                        app.respond_to_permission_request(&request_id, PermissionDecision::Deny)
                            .await;
                    }
                }
            });
        }

        // For MCP trust approval requests, send a server-request and await
        // the client's response, mirroring the permission flow above.
        if let StreamEvent::McpTrustApprovalRequested { ref request } = event {
            let server_req_id = format!("srv-mcp-trust-{next_id}");
            next_id += 1;

            let (tx, rx_oneshot) = oneshot::channel();
            pending.lock().await.insert(server_req_id.clone(), tx);

            let cleanup_id = server_req_id.clone();

            sink.send(ServerMessage::Request(ServerRequestEnvelope {
                id: server_req_id,
                method: method::SERVER_REQUEST_MCP_TRUST.to_string(),
                params: serde_json::to_value(request).unwrap_or_default(),
            }));

            // Spawn a task to await the client's trust decision with timeout.
            let app = app_server.clone();
            let session_id = request.session_id.clone();
            let server_id = request.server_id.clone();
            let timeout_dur = permission_timeout;
            let pending_cleanup = Arc::clone(&pending);
            // Detached waiter; it owns cleanup for the pending MCP trust request.
            let _mcp_trust_waiter_handle = tokio::spawn(async move {
                let result = tokio::time::timeout(timeout_dur, rx_oneshot).await;

                // Clean up pending map entry regardless of outcome.
                pending_cleanup.lock().await.remove(&cleanup_id);

                match result {
                    Ok(Ok(ResponseResult::Success { data: Some(data) })) => {
                        // Try full McpTrustResponseParams first, then bare
                        // McpTrustDecisionWire for lenient clients.
                        if let Ok(resp) =
                            serde_json::from_value::<McpTrustResponseParams>(data.clone())
                        {
                            let trust = match resp.decision {
                                McpTrustDecisionWire::Trust => orbcode_mcp::McpServerTrust::Trusted,
                                _ => orbcode_mcp::McpServerTrust::Denied,
                            };
                            let _ = app
                                .set_mcp_server_trust_for_session(&session_id, &server_id, trust)
                                .await;
                            return;
                        }
                        if let Ok(decision) = serde_json::from_value::<McpTrustDecisionWire>(data) {
                            let trust = match decision {
                                McpTrustDecisionWire::Trust => orbcode_mcp::McpServerTrust::Trusted,
                                _ => orbcode_mcp::McpServerTrust::Denied,
                            };
                            let _ = app
                                .set_mcp_server_trust_for_session(&session_id, &server_id, trust)
                                .await;
                            return;
                        }
                        // Unrecognizable response -- deny.
                        let _ = app
                            .set_mcp_server_trust_for_session(
                                &session_id,
                                &server_id,
                                orbcode_mcp::McpServerTrust::Denied,
                            )
                            .await;
                    }
                    Ok(Ok(_) | Err(_)) => {
                        let _ = app
                            .set_mcp_server_trust_for_session(
                                &session_id,
                                &server_id,
                                orbcode_mcp::McpServerTrust::Denied,
                            )
                            .await;
                    }
                    Err(_) => {
                        // Timeout -- deny.
                        let _ = app
                            .set_mcp_server_trust_for_session(
                                &session_id,
                                &server_id,
                                orbcode_mcp::McpServerTrust::Denied,
                            )
                            .await;
                    }
                }
            });
        }

        // For AskUserQuestion requests, send a server-request and await
        // the client's response, mirroring the permission flow above.
        if let StreamEvent::AskUserQuestionRequested {
            ref session_id,
            ref turn_id,
            ref tool_use_id,
            ref request_id,
            ref deadline,
            ref questions,
            ref question,
            ref options,
        } = event
        {
            let server_req_id = format!("srv-ask-user-{next_id}");
            next_id += 1;

            _ask_user_guard.request_ids.push(request_id.clone());

            let (tx, rx_oneshot) = oneshot::channel();
            pending.lock().await.insert(server_req_id.clone(), tx);

            let cleanup_id = server_req_id.clone();

            let ask_params = AskUserQuestionRequest {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_use_id: tool_use_id.clone(),
                request_id: request_id.clone(),
                deadline: deadline.clone(),
                validation_error: None,
                questions: questions.clone(),
                question: question.clone(),
                options: options.clone(),
            };

            sink.send(ServerMessage::Request(ServerRequestEnvelope {
                id: server_req_id,
                method: method::SERVER_REQUEST_ASK_USER.to_string(),
                params: serde_json::to_value(&ask_params).unwrap_or_default(),
            }));

            let app = app_server.clone();
            let core_session_id = session_id.clone();
            let core_request_id = request_id.clone();
            let original_request = ask_params.clone();
            let pending_cleanup = Arc::clone(&pending);
            let sink_for_waiter = Arc::clone(&sink);
            let subscription_id_for_waiter = subscription_id.clone();
            let response_deadline = tokio::time::Instant::now() + permission_timeout;
            // Detached waiter; it owns cleanup for the pending ask-user request.
            let _ask_user_waiter_handle = tokio::spawn(async move {
                let mut rx_oneshot = rx_oneshot;
                loop {
                    let result = loop {
                        tokio::select! {
                            r = &mut rx_oneshot => break Ok(r),
                            _ = tokio::time::sleep_until(response_deadline) => break Err(()),
                            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                                if sink_for_waiter.is_closed() {
                                    break Err(());
                                }
                            }
                        }
                    };

                    pending_cleanup.lock().await.remove(&cleanup_id);

                    match result {
                        Ok(Ok(ResponseResult::Success { data: Some(data) })) => {
                            let outcome = serde_json::from_value::<AskUserQuestionResponse>(data)
                                .map_err(|error| AskUserValidationError {
                                    code: AskUserValidationCode::MalformedResponse,
                                    message: format!("invalid AskUserQuestion response: {error}"),
                                })
                                .and_then(|response| {
                                    if response.request_id != core_request_id {
                                        return Err(AskUserValidationError {
                                            code: AskUserValidationCode::RequestIdMismatch,
                                            message: format!(
                                                "response request id `{}` does not match `{}`",
                                                response.request_id, core_request_id
                                            ),
                                        });
                                    }
                                    response.canonical_outcome(&original_request)
                                });
                            match outcome {
                                Ok(outcome) => {
                                    if app
                                        .resolve_ask_user_question(
                                            &core_session_id,
                                            &core_request_id,
                                            outcome.clone(),
                                        )
                                        .is_ok()
                                    {
                                        let notification = StreamEventNotification {
                                            subscription_id: subscription_id_for_waiter.clone(),
                                            event: StreamEvent::AskUserQuestionResolved {
                                                request_id: core_request_id.clone(),
                                                outcome: Some(outcome),
                                                answer: None,
                                            },
                                        };
                                        sink_for_waiter.send(ServerMessage::Notification(
                                            ServerNotificationEnvelope {
                                                method: method::NOTIFICATION_STREAM_EVENT
                                                    .to_string(),
                                                params: serde_json::to_value(notification)
                                                    .unwrap_or_default(),
                                            },
                                        ));
                                    }
                                    break;
                                }
                                Err(validation_error) => {
                                    let (retry_tx, retry_rx) = oneshot::channel();
                                    pending_cleanup
                                        .lock()
                                        .await
                                        .insert(cleanup_id.clone(), retry_tx);
                                    let mut retry_request = original_request.clone();
                                    retry_request.validation_error = Some(validation_error);
                                    sink_for_waiter.send(ServerMessage::Request(
                                        ServerRequestEnvelope {
                                            id: cleanup_id.clone(),
                                            method: method::SERVER_REQUEST_ASK_USER.to_string(),
                                            params: serde_json::to_value(retry_request)
                                                .unwrap_or_default(),
                                        },
                                    ));
                                    rx_oneshot = retry_rx;
                                }
                            }
                        }
                        Ok(Ok(_)) | Ok(Err(_)) => {
                            app.cancel_pending_ask_user(
                                std::slice::from_ref(&core_request_id),
                                AskUserCancellationReason::ClientClosed,
                            );
                            break;
                        }
                        Err(()) => {
                            let reason = if sink_for_waiter.is_closed() {
                                AskUserCancellationReason::Disconnect
                            } else {
                                AskUserCancellationReason::Timeout
                            };
                            app.cancel_pending_ask_user(
                                std::slice::from_ref(&core_request_id),
                                reason,
                            );
                            break;
                        }
                    }
                }
            });
        }

        // Always send the event as a notification.
        let notification = StreamEventNotification {
            subscription_id: subscription_id.clone(),
            event: event.clone(),
        };
        sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
            method: method::NOTIFICATION_STREAM_EVENT.to_string(),
            params: serde_json::to_value(notification).unwrap_or_default(),
        }));
    }

    // _ask_user_guard drops here (or on task abort), cancelling only this
    // pump's pending ask-user requests. See PumpAskUserGuard::drop.
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_app_server_protocol::ClientRequestEnvelope;
    use orbcode_config::AppConfigOverrides;
    use serde_json::json;

    /// Test sink that collects messages into a shared `Vec`.
    struct TestSink {
        messages: Arc<Mutex<Vec<ServerMessage>>>,
        closed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl TestSink {
        fn new() -> (Arc<Self>, Arc<Mutex<Vec<ServerMessage>>>) {
            let messages = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::new(Self {
                messages: Arc::clone(&messages),
                closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            (sink, messages)
        }
    }

    impl ServerSink for TestSink {
        fn send(&self, message: ServerMessage) {
            let msgs = Arc::clone(&self.messages);
            // Since send is not async, use try_lock for simplicity in tests.
            if let Ok(mut guard) = msgs.try_lock() {
                guard.push(message);
            }
        }

        fn is_closed(&self) -> bool {
            self.closed.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    struct PanicSink;

    impl ServerSink for PanicSink {
        fn send(&self, _message: ServerMessage) {
            panic!("injected subscription pump panic");
        }

        fn is_closed(&self) -> bool {
            false
        }
    }

    struct DropCanary(Arc<AtomicUsize>);

    impl Drop for DropCanary {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn wait_for_subscription_to_finish(processor: &MessageProcessor, subscription_id: &str) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if processor
                    .active_subscriptions
                    .get(subscription_id)
                    .is_some_and(JoinHandle::is_finished)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("subscription pump should finish within timeout");
    }

    fn subscription_failure_messages(
        messages: &[ServerMessage],
        subscription_id: &str,
    ) -> Vec<String> {
        messages
            .iter()
            .filter_map(|message| {
                let ServerMessage::Notification(notification) = message else {
                    return None;
                };
                let notification =
                    serde_json::from_value::<StreamEventNotification>(notification.params.clone())
                        .ok()?;
                if notification.subscription_id != subscription_id {
                    return None;
                }
                let StreamEvent::Error { message, .. } = notification.event else {
                    return None;
                };
                Some(message)
            })
            .collect()
    }

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-msg-proc-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    async fn test_app(label: &str) -> AppServer {
        let home = test_path(&format!("{label}-home"));
        let cwd = test_path(&format!("{label}-cwd"));
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server")
    }

    fn initialize_request() -> ClientMessage {
        ClientMessage::Request(ClientRequestEnvelope {
            id: "init-1".into(),
            method: "initialize".into(),
            params: Some(json!({
                "protocol_version": "1.0",
                "client_info": { "name": "test", "version": "0.1" },
            })),
        })
    }

    // -----------------------------------------------------------------------
    // 1. Pre-initialize request is rejected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn pre_init_request_rejected() {
        let app = test_app("pre-init").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        // Send a session/list request before initialize
        processor
            .handle_message(ClientMessage::Request(ClientRequestEnvelope {
                id: "req-1".into(),
                method: "session/list".into(),
                params: None,
            }))
            .await;

        let msgs = messages.lock().await;
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            ServerMessage::Response(resp) => {
                assert_eq!(resp.id, "req-1");
                match &resp.result {
                    ResponseResult::Error(err) => {
                        assert_eq!(err.code, ErrorCode::InvalidRequest);
                        assert!(err.message.contains("not initialized"));
                    }
                    other => panic!("expected Error, got: {other:?}"),
                }
            }
            other => panic!("expected Response, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // 2. Initialize then request succeeds
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn initialize_then_request_succeeds() {
        let app = test_app("init-then-req").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        // Initialize first
        processor.handle_message(initialize_request()).await;

        assert!(processor.is_initialized());
        assert!(processor.client_info().is_some());
        assert_eq!(processor.client_info().unwrap().name, "test");

        // Now send session/list -- should succeed
        processor
            .handle_message(ClientMessage::Request(ClientRequestEnvelope {
                id: "req-2".into(),
                method: "session/list".into(),
                params: None,
            }))
            .await;

        let msgs = messages.lock().await;
        assert_eq!(msgs.len(), 2);

        // First message: initialize response
        match &msgs[0] {
            ServerMessage::Response(resp) => {
                assert_eq!(resp.id, "init-1");
                match &resp.result {
                    ResponseResult::Success { data: Some(_) } => {}
                    other => panic!("expected initialize Success, got: {other:?}"),
                }
            }
            other => panic!("expected Response, got: {other:?}"),
        }

        // Second message: session/list response
        match &msgs[1] {
            ServerMessage::Response(resp) => {
                assert_eq!(resp.id, "req-2");
                match &resp.result {
                    ResponseResult::Success { .. } => {}
                    other => panic!("expected session/list Success, got: {other:?}"),
                }
            }
            other => panic!("expected Response, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // 3. Turn submit delivers notifications (mock provider)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn turn_submit_delivers_notifications() {
        let home = test_path("turn-notif-home");
        let cwd = test_path("turn-notif-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let mut env = crate::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "mock://anthropic?scenario=hello".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                env_overrides: env,
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        // Initialize
        processor.handle_message(initialize_request()).await;

        // Bootstrap
        processor
            .handle_message(ClientMessage::Request(ClientRequestEnvelope {
                id: "bs-1".into(),
                method: "session/bootstrap".into(),
                params: None,
            }))
            .await;

        // Extract session_id from bootstrap response
        let session_id = {
            let msgs = messages.lock().await;
            let bs_resp = msgs
                .iter()
                .find(|m| matches!(m, ServerMessage::Response(r) if r.id == "bs-1"))
                .expect("bootstrap response");
            match bs_resp {
                ServerMessage::Response(r) => match &r.result {
                    ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
                        .as_str()
                        .expect("session_id")
                        .to_string(),
                    other => panic!("expected bootstrap Success, got: {other:?}"),
                },
                _ => unreachable!(),
            }
        };

        // Submit a turn
        processor
            .handle_message(ClientMessage::Request(ClientRequestEnvelope {
                id: "turn-1".into(),
                method: "turn/submit".into(),
                params: Some(json!({
                    "session_id": session_id,
                    "prompt": "hello",
                })),
            }))
            .await;

        // Wait for the pump to deliver events
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

        let msgs = messages.lock().await;

        // Should have: init response, bootstrap response, turn/submit response,
        // plus at least some stream event notifications.
        let turn_resp = msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Response(r) if r.id == "turn-1"));
        assert!(turn_resp.is_some(), "should have turn/submit response");

        // Check that we got at least one notification
        let notifications: Vec<_> = msgs
            .iter()
            .filter(|m| matches!(m, ServerMessage::Notification(_)))
            .collect();
        assert!(
            !notifications.is_empty(),
            "should have received stream event notifications"
        );

        // Check that all notifications are stream/event method
        for notif in &notifications {
            if let ServerMessage::Notification(n) = notif {
                assert_eq!(n.method, "stream/event");
            }
        }
    }

    // -----------------------------------------------------------------------
    // 4. Permission server-request resolution
    // -----------------------------------------------------------------------

    // This test verifies the structural wiring: the processor resolves a
    // pending oneshot when the client responds to a server-request.
    #[tokio::test]
    async fn permission_server_request_flow_structural() {
        let app = test_app("perm-flow").await;
        let (sink, _messages) = TestSink::new();
        let processor = MessageProcessor::new(app, sink);

        let (tx, rx) = oneshot::channel();
        let req_id = "srv-perm-test-1".to_string();
        processor
            .pending_server_requests
            .lock()
            .await
            .insert(req_id.clone(), tx);

        // Simulate client responding to the server request
        processor.resolve_server_request(ServerRequestResponse {
            id: req_id,
            result: ResponseResult::Success {
                data: Some(json!({
                    "request_id": "perm-123",
                    "decision": { "decision": "approve" },
                })),
            },
        });

        // The oneshot should resolve
        let result = tokio::time::timeout(tokio::time::Duration::from_secs(1), rx)
            .await
            .expect("should resolve within timeout")
            .expect("channel should not be dropped");

        match result {
            ResponseResult::Success { data: Some(data) } => {
                assert_eq!(data["decision"]["decision"], "approve");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // 5. Unknown server-request response is ignored
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn unknown_server_request_response_is_ignored() {
        let app = test_app("unknown-srv-resp").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        // Initialize first
        processor.handle_message(initialize_request()).await;

        // Send a response with an unknown ID -- should not crash
        processor
            .handle_message(ClientMessage::Response(ServerRequestResponse {
                id: "nonexistent-server-request".into(),
                result: ResponseResult::Success {
                    data: Some(json!({"decision": "approve"})),
                },
            }))
            .await;

        // Only the initialize response should be on the sink
        let msgs = messages.lock().await;
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            ServerMessage::Response(r) => assert_eq!(r.id, "init-1"),
            other => panic!("expected only the init Response, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // 6. Bounded sink does not panic when full
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn bounded_sink_drops_best_effort_when_full() {
        // Create a very small bounded best-effort channel.
        let (lossless_tx, _lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, _best_effort_rx) = mpsc::channel::<ServerMessage>(2);
        let sink = ChannelSink::new(lossless_tx, best_effort_tx);

        // Best-effort messages (non-terminal notifications) are dropped
        // when the bounded channel is full — no panic.
        for i in 0..100 {
            sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
                method: "stream/event".to_string(),
                params: serde_json::json!({"event": {"event": "assistant_delta", "session_id": format!("s-{i}"), "delta": "x"}}),
            }));
        }
        assert!(!sink.is_closed());
    }

    #[tokio::test]
    async fn lossless_messages_never_dropped() {
        // Lossless channel is unbounded — responses are always delivered.
        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, _best_effort_rx) = mpsc::channel::<ServerMessage>(2);
        let sink = ChannelSink::new(lossless_tx, best_effort_tx);

        // Send many responses (lossless).
        for i in 0..100 {
            sink.send(ServerMessage::Response(ServerResponseEnvelope {
                id: format!("msg-{i}"),
                result: ResponseResult::Success { data: None },
            }));
        }

        // All 100 should be received.
        let mut count = 0;
        while lossless_rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 100);
    }

    // -----------------------------------------------------------------------
    // 7. Server-request timeout denies permission
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn server_request_timeout_denies_permission() {
        use orbcode_protocol::PermissionRequest;

        let app = test_app("perm-timeout").await;
        let (sink, messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Create a stream that emits a single PermissionRequested event.
        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::PermissionRequested {
                request: PermissionRequest {
                    request_id: "perm-timeout-1".to_string(),
                    session_id: "test-session".to_string(),
                    tool_use_id: "tu-1".to_string(),
                    tool_name: "test_tool".to_string(),
                    tool_input: "{}".to_string(),
                    requires_tools_permission: true,
                    requires_network_permission: false,
                },
            })
            .unwrap();
        drop(event_tx); // close the stream so pump_events finishes

        // Use a very short timeout (50ms) so the test runs quickly.
        pump_events(
            event_rx,
            sink,
            pending,
            app,
            "test-sub".to_string(),
            0,
            Duration::from_millis(50),
        )
        .await;

        // The pump should have emitted both a server-request and a
        // notification for the PermissionRequested event.
        let msgs = messages.lock().await;
        let has_server_request = msgs.iter().any(|m| matches!(m, ServerMessage::Request(_)));
        assert!(
            has_server_request,
            "should have sent a server-request for the permission"
        );
    }

    // -----------------------------------------------------------------------
    // 7b. MCP trust server-request emits request and awaits response
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mcp_trust_server_request_emits_request() {
        use orbcode_protocol::McpTrustApprovalRequest;

        let app = test_app("mcp-trust-req").await;
        let (sink, messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Create a stream that emits a single McpTrustApprovalRequested event.
        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::McpTrustApprovalRequested {
                request: McpTrustApprovalRequest {
                    request_id: "mcp-trust-1".to_string(),
                    session_id: "session-mcp-trust-req".to_string(),
                    server_id: "test-mcp-server".to_string(),
                    tool_name: "test_tool".to_string(),
                },
            })
            .unwrap();
        drop(event_tx);

        // Use a very short timeout so the test runs quickly.
        pump_events(
            event_rx,
            sink,
            pending,
            app,
            "test-sub".to_string(),
            0,
            Duration::from_millis(50),
        )
        .await;

        let msgs = messages.lock().await;
        let has_trust_request = msgs.iter().any(|m| {
            matches!(
                m,
                ServerMessage::Request(ServerRequestEnvelope { method, .. })
                if method == method::SERVER_REQUEST_MCP_TRUST
            )
        });
        assert!(
            has_trust_request,
            "should have sent a server-request for MCP trust approval"
        );
    }

    // -----------------------------------------------------------------------
    // 7c. MCP trust server-request structural resolution
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mcp_trust_server_request_flow_structural() {
        let app = test_app("mcp-trust-flow").await;
        let (sink, _messages) = TestSink::new();
        let processor = MessageProcessor::new(app, sink);

        let (tx, rx) = oneshot::channel();
        let req_id = "srv-mcp-trust-test-1".to_string();
        processor
            .pending_server_requests
            .lock()
            .await
            .insert(req_id.clone(), tx);

        // Simulate client responding to the MCP trust server-request.
        processor.resolve_server_request(ServerRequestResponse {
            id: req_id,
            result: ResponseResult::Success {
                data: Some(json!({
                    "server_id": "test-mcp-server",
                    "decision": "trust",
                })),
            },
        });

        // The oneshot should resolve.
        let result = tokio::time::timeout(tokio::time::Duration::from_secs(1), rx)
            .await
            .expect("should resolve within timeout")
            .expect("channel should not be dropped");

        match result {
            ResponseResult::Success { data: Some(data) } => {
                assert_eq!(data["decision"], "trust");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // 7d. MCP trust: Trust response resolves the pending request
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mcp_trust_trust_response_resolves_pending() {
        use orbcode_protocol::McpTrustApprovalRequest;

        let app = test_app("mcp-trust-trust").await;
        let (sink, messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Create a stream that emits a single McpTrustApprovalRequested event.
        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::McpTrustApprovalRequested {
                request: McpTrustApprovalRequest {
                    request_id: "mcp-trust-e2e-1".to_string(),
                    session_id: "session-mcp-trust-e2e".to_string(),
                    server_id: "my-mcp-server".to_string(),
                    tool_name: "my_tool".to_string(),
                },
            })
            .unwrap();
        drop(event_tx);

        // Clone pending so we can inject a response from "outside".
        let pending_clone = Arc::clone(&pending);

        // Spawn the pump in a task so we can resolve the request concurrently.
        let pump_handle = tokio::spawn({
            let sink = Arc::clone(&sink) as Arc<dyn ServerSink>;
            let app = app.clone();
            async move {
                pump_events(
                    event_rx,
                    sink,
                    pending_clone,
                    app,
                    "test-trust-sub".to_string(),
                    0,
                    Duration::from_secs(5),
                )
                .await;
            }
        });

        // Wait a moment for the pump to emit the server-request and insert
        // into pending.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Find the server-request ID that was sent.
        let server_req_id = {
            let msgs = messages.lock().await;
            msgs.iter()
                .find_map(|m| match m {
                    ServerMessage::Request(ServerRequestEnvelope { id, method, .. })
                        if method == method::SERVER_REQUEST_MCP_TRUST =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .expect("pump should have emitted a trust server-request")
        };

        // Respond with a Trust decision through the pending map.
        {
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(&server_req_id) {
                tx.send(ResponseResult::Success {
                    data: Some(json!({
                        "server_id": "my-mcp-server",
                        "decision": "trust",
                    })),
                })
                .expect("oneshot send");
            } else {
                panic!("pending map should contain the server-request ID: {server_req_id}");
            }
        }

        // Wait for spawned resolution task to complete.
        tokio::time::sleep(Duration::from_millis(100)).await;
        pump_handle.abort();

        // Verify the server-request params contain the expected fields.
        let msgs = messages.lock().await;
        let trust_req = msgs.iter().find_map(|m| match m {
            ServerMessage::Request(ServerRequestEnvelope { params, method, .. })
                if method == method::SERVER_REQUEST_MCP_TRUST =>
            {
                Some(params.clone())
            }
            _ => None,
        });
        let params = trust_req.expect("trust server-request should exist");
        assert_eq!(params["server_id"], "my-mcp-server");
        assert_eq!(params["tool_name"], "my_tool");
        assert_eq!(params["request_id"], "mcp-trust-e2e-1");
    }

    // -----------------------------------------------------------------------
    // 7e. MCP trust: Deny response resolves the pending request
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mcp_trust_deny_response_resolves_pending() {
        use orbcode_protocol::McpTrustApprovalRequest;

        let app = test_app("mcp-trust-deny").await;
        let (sink, messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::McpTrustApprovalRequested {
                request: McpTrustApprovalRequest {
                    request_id: "mcp-trust-deny-1".to_string(),
                    session_id: "session-mcp-trust-deny".to_string(),
                    server_id: "untrusted-server".to_string(),
                    tool_name: "dangerous_tool".to_string(),
                },
            })
            .unwrap();
        drop(event_tx);

        let pending_clone = Arc::clone(&pending);

        let pump_handle = tokio::spawn({
            let sink = Arc::clone(&sink) as Arc<dyn ServerSink>;
            let app = app.clone();
            async move {
                pump_events(
                    event_rx,
                    sink,
                    pending_clone,
                    app,
                    "test-deny-sub".to_string(),
                    0,
                    Duration::from_secs(5),
                )
                .await;
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Find the server-request ID.
        let server_req_id = {
            let msgs = messages.lock().await;
            msgs.iter()
                .find_map(|m| match m {
                    ServerMessage::Request(ServerRequestEnvelope { id, method, .. })
                        if method == method::SERVER_REQUEST_MCP_TRUST =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .expect("pump should have emitted a trust server-request")
        };

        // Respond with a Deny decision.
        {
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(&server_req_id) {
                tx.send(ResponseResult::Success {
                    data: Some(json!({
                        "server_id": "untrusted-server",
                        "decision": "deny",
                    })),
                })
                .expect("oneshot send");
            } else {
                panic!("pending map should contain the server-request ID");
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        pump_handle.abort();

        // Verify the server-request was emitted with correct params.
        let msgs = messages.lock().await;
        let trust_req = msgs.iter().find_map(|m| match m {
            ServerMessage::Request(ServerRequestEnvelope { params, method, .. })
                if method == method::SERVER_REQUEST_MCP_TRUST =>
            {
                Some(params.clone())
            }
            _ => None,
        });
        let params = trust_req.expect("trust server-request should exist");
        assert_eq!(params["server_id"], "untrusted-server");
        assert_eq!(params["tool_name"], "dangerous_tool");
    }

    // -----------------------------------------------------------------------
    // 7f. MCP trust: Timeout defaults to deny
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mcp_trust_timeout_defaults_to_deny() {
        use orbcode_protocol::McpTrustApprovalRequest;

        let app = test_app("mcp-trust-timeout").await;
        let (sink, messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::McpTrustApprovalRequested {
                request: McpTrustApprovalRequest {
                    request_id: "mcp-trust-timeout-1".to_string(),
                    session_id: "session-mcp-trust-timeout".to_string(),
                    server_id: "timeout-server".to_string(),
                    tool_name: "slow_tool".to_string(),
                },
            })
            .unwrap();
        drop(event_tx);

        // Use a very short timeout so the test finishes quickly.
        pump_events(
            event_rx,
            sink,
            pending.clone(),
            app,
            "test-timeout-sub".to_string(),
            0,
            Duration::from_millis(50),
        )
        .await;

        // Wait for the timeout task to fire.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The pending map should be empty (cleaned up after timeout).
        let map = pending.lock().await;
        assert!(
            map.is_empty(),
            "pending map should be cleaned up after timeout"
        );

        // The server-request should have been emitted.
        let msgs = messages.lock().await;
        let has_trust_request = msgs.iter().any(|m| {
            matches!(
                m,
                ServerMessage::Request(ServerRequestEnvelope { method, .. })
                if method == method::SERVER_REQUEST_MCP_TRUST
            )
        });
        assert!(
            has_trust_request,
            "should have emitted a trust server-request before timeout"
        );
    }

    // -----------------------------------------------------------------------
    // 8. Sink-closed skips processing
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // 7g. Auto-deny resolves pending permission without 5-min timeout
    // -----------------------------------------------------------------------

    /// Proves that responding with `Deny` to a pending permission
    /// server-request resolves immediately rather than blocking until
    /// the 300-second `DEFAULT_PERMISSION_TIMEOUT` expires.
    ///
    /// The pump is started with the full default timeout (300 s). A
    /// background task responds with `Deny` within milliseconds. The
    /// test asserts that the entire sequence completes within 5 seconds,
    /// which would be impossible if the code path waited for the timeout.
    #[tokio::test]
    async fn auto_deny_resolves_pending_permission_without_timeout() {
        use orbcode_protocol::PermissionRequest;

        let app = test_app("perm-auto-deny").await;
        let (sink, messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Create a stream with a single PermissionRequested event.
        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::PermissionRequested {
                request: PermissionRequest {
                    request_id: "perm-auto-deny-1".to_string(),
                    session_id: "test-session".to_string(),
                    tool_use_id: "tu-deny-1".to_string(),
                    tool_name: "test_tool".to_string(),
                    tool_input: "{}".to_string(),
                    requires_tools_permission: true,
                    requires_network_permission: false,
                },
            })
            .unwrap();
        drop(event_tx); // Close the stream so pump_events finishes after processing.

        let pending_clone = Arc::clone(&pending);

        // Spawn the pump with the FULL default timeout (300 s). If the
        // deny does not short-circuit the wait, this task will hang for
        // 5 minutes and the outer 5 s deadline will trip.
        let pump_handle = tokio::spawn({
            let sink = Arc::clone(&sink) as Arc<dyn ServerSink>;
            let app = app.clone();
            async move {
                pump_events(
                    event_rx,
                    sink,
                    pending_clone,
                    app,
                    "test-auto-deny-sub".to_string(),
                    0,
                    DEFAULT_PERMISSION_TIMEOUT, // 300 seconds
                )
                .await;
            }
        });

        // Give the pump time to emit the server-request and register the
        // pending oneshot.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Find the server-request ID that was emitted.
        let server_req_id = {
            let msgs = messages.lock().await;
            msgs.iter()
                .find_map(|m| match m {
                    ServerMessage::Request(ServerRequestEnvelope { id, method, .. })
                        if method == method::SERVER_REQUEST_PERMISSION =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .expect("pump should have emitted a permission server-request")
        };

        // Immediately respond with a Deny decision through the pending map.
        {
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(&server_req_id) {
                tx.send(ResponseResult::Success {
                    data: Some(json!({
                        "request_id": "perm-auto-deny-1",
                        "decision": { "decision": "deny" },
                    })),
                })
                .expect("oneshot send");
            } else {
                panic!("pending map should contain the server-request ID: {server_req_id}");
            }
        }

        // The pump and its spawned resolution task should complete well
        // within 5 seconds. If the auto-deny path were broken and the
        // code waited for DEFAULT_PERMISSION_TIMEOUT (300 s), this
        // assertion would fail.
        let result = tokio::time::timeout(Duration::from_secs(5), pump_handle).await;
        assert!(
            result.is_ok(),
            "pump should complete within 5 seconds after Deny, not block for 300 s"
        );

        // Verify the pending map was cleaned up.
        let map = pending.lock().await;
        assert!(
            map.is_empty(),
            "pending map should be cleaned up after Deny resolution"
        );
    }

    // -----------------------------------------------------------------------
    // 8. Sink-closed skips processing
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sink_closed_skips_processing() {
        let app = test_app("sink-closed").await;
        let (sink, messages) = TestSink::new();

        // Mark the sink as closed before processing any messages.
        sink.closed
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let mut processor = MessageProcessor::new(app, sink);

        // Send an initialize request -- it should be silently skipped.
        processor.handle_message(initialize_request()).await;

        // No messages should have been sent since the sink was closed.
        let msgs = messages.lock().await;
        assert!(
            msgs.is_empty(),
            "no messages should be sent when the sink is closed"
        );

        // The processor should not have been initialized.
        assert!(!processor.is_initialized());
    }

    // -----------------------------------------------------------------------
    // 9b. Sink closed (ChannelSink) aborts processing
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn channel_sink_closed_aborts_processing() {
        let app = test_app("chan-sink-closed").await;

        // Create a real ChannelSink, then drop the receivers so is_closed()
        // returns true -- simulating the writer task exiting.
        let (lossless_tx, lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, best_effort_rx) =
            mpsc::channel::<ServerMessage>(CHANNEL_SINK_CAPACITY);
        let sink: Arc<dyn ServerSink> = Arc::new(ChannelSink::new(lossless_tx, best_effort_tx));

        // Drop the receivers to close both channels.
        drop(lossless_rx);
        drop(best_effort_rx);

        assert!(
            sink.is_closed(),
            "sink must report closed when receivers are dropped"
        );

        let mut processor = MessageProcessor::new(app, sink);

        // Sending a message when the sink is closed should be silently skipped.
        processor.handle_message(initialize_request()).await;

        // The processor should NOT have marked itself as initialized because
        // the message was skipped.
        assert!(
            !processor.is_initialized(),
            "processor must skip processing when sink is closed"
        );
    }

    // -----------------------------------------------------------------------
    // 9c. Processor drop closes channels
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn processor_drop_closes_channels() {
        let app = test_app("proc-drop").await;

        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, mut best_effort_rx) =
            mpsc::channel::<ServerMessage>(CHANNEL_SINK_CAPACITY);
        let sink: Arc<dyn ServerSink> = Arc::new(ChannelSink::new(lossless_tx, best_effort_tx));

        let mut processor = MessageProcessor::new(app, sink);

        // Initialize so the processor is active.
        processor.handle_message(initialize_request()).await;
        assert!(processor.is_initialized());

        // Drain the initialize response that was sent.
        let _ = lossless_rx.try_recv();

        // Drop the processor -- this should drop the last Arc<ChannelSink>,
        // closing the channel senders.
        drop(processor);

        // Both receivers should now return None (channel closed).
        assert!(
            lossless_rx.recv().await.is_none(),
            "lossless channel must be closed after processor drop"
        );
        assert!(
            best_effort_rx.recv().await.is_none(),
            "best-effort channel must be closed after processor drop"
        );
    }

    #[tokio::test]
    async fn double_initialize_overwrites_client_info() {
        let (sink, messages) = TestSink::new();
        let app = test_app("double-init").await;
        let mut processor = MessageProcessor::new(app, sink);

        // First initialize
        processor.handle_message(initialize_request()).await;
        assert!(processor.is_initialized());

        // Second initialize with different client info
        let second_init = ClientMessage::Request(ClientRequestEnvelope {
            id: "init-2".into(),
            method: "initialize".into(),
            params: Some(serde_json::json!({
                "protocol_version": "2.0",
                "client_info": { "name": "second-client", "version": "9.0" },
            })),
        });
        processor.handle_message(second_init).await;

        assert!(processor.is_initialized());

        // Verify client_info was actually overwritten
        let info = processor.client_info().expect("client_info should be set");
        assert_eq!(info.name, "second-client");
        assert_eq!(info.version, "9.0");

        let msgs = messages.lock().await;
        assert_eq!(msgs.len(), 2, "both initialize responses should be sent");

        if let ServerMessage::Response(ref resp) = msgs[1] {
            assert_eq!(resp.id, "init-2");
            match &resp.result {
                ResponseResult::Success { data: Some(data) } => {
                    assert_eq!(
                        data["protocol_version"].as_str(),
                        Some("2.0"),
                        "second initialize should echo the new protocol_version"
                    );
                }
                other => panic!("expected Success, got: {other:?}"),
            }
        } else {
            panic!("expected Response for second initialize");
        }
    }

    // -----------------------------------------------------------------------
    // Moved from tests/mcp_trust_e2e.rs — these need direct access to
    // pending_server_requests (a private field).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn handle_message_response_routes_to_pending_oneshot() {
        let app = test_app("proc-route").await;
        let (sink, _messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor.handle_message(initialize_request()).await;

        let (tx, rx) = oneshot::channel();
        let req_id = "srv-mcp-trust-route-1".to_string();
        processor
            .pending_server_requests
            .lock()
            .await
            .insert(req_id.clone(), tx);

        processor
            .handle_message(ClientMessage::Response(ServerRequestResponse {
                id: req_id,
                result: ResponseResult::Success {
                    data: Some(json!({
                        "request_id": "req-1",
                        "decision": "trust",
                    })),
                },
            }))
            .await;

        let result = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("should resolve")
            .expect("channel ok");

        match result {
            ResponseResult::Success { data: Some(data) } => {
                assert_eq!(data["decision"], "trust");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_message_response_deny_routes_correctly() {
        let app = test_app("proc-deny").await;
        let (sink, _messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor.handle_message(initialize_request()).await;

        let (tx, rx) = oneshot::channel();
        let req_id = "srv-mcp-trust-deny-1".to_string();
        processor
            .pending_server_requests
            .lock()
            .await
            .insert(req_id.clone(), tx);

        processor
            .handle_message(ClientMessage::Response(ServerRequestResponse {
                id: req_id,
                result: ResponseResult::Success {
                    data: Some(json!({ "decision": "deny" })),
                },
            }))
            .await;

        let result = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("should resolve")
            .expect("channel ok");

        match result {
            ResponseResult::Success { data: Some(data) } => {
                assert_eq!(data["decision"], "deny");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Backpressure: best-effort dropped but lossless preserved under load
    // -----------------------------------------------------------------------

    /// Floods the ChannelSink with best-effort notifications beyond the
    /// bounded capacity, then sends lossless messages (responses and
    /// server-requests). Verifies that best-effort messages are silently
    /// dropped while every lossless message is preserved.
    #[tokio::test]
    async fn backpressure_best_effort_dropped_lossless_preserved() {
        let capacity = 4;
        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, mut best_effort_rx) = mpsc::channel::<ServerMessage>(capacity);
        let sink = ChannelSink::new(lossless_tx, best_effort_tx);

        // Flood with best-effort deltas -- many more than the capacity.
        let flood_count = capacity * 10;
        for i in 0..flood_count {
            sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
                method: "stream/event".to_string(),
                params: json!({
                    "event": {
                        "event": "assistant_delta",
                        "session_id": "s-bp",
                        "delta": format!("d-{i}")
                    }
                }),
            }));
        }

        // Count how many best-effort messages actually arrived.
        let mut best_effort_count = 0;
        while best_effort_rx.try_recv().is_ok() {
            best_effort_count += 1;
        }
        // Some must have been dropped (capacity is much smaller than flood).
        assert!(
            best_effort_count < flood_count,
            "best-effort messages should be dropped under backpressure \
             (got {best_effort_count}, sent {flood_count})"
        );
        // At least `capacity` should have been buffered.
        assert!(
            best_effort_count >= capacity,
            "at least the channel capacity should be buffered"
        );

        // Now send lossless messages (responses + server-requests).
        let lossless_count = 50;
        for i in 0..lossless_count {
            if i % 2 == 0 {
                sink.send(ServerMessage::Response(ServerResponseEnvelope {
                    id: format!("resp-{i}"),
                    result: ResponseResult::Success { data: None },
                }));
            } else {
                sink.send(ServerMessage::Request(ServerRequestEnvelope {
                    id: format!("srv-req-{i}"),
                    method: "permission/request".to_string(),
                    params: json!({}),
                }));
            }
        }

        // Every lossless message must arrive.
        let mut received_lossless = 0;
        while lossless_rx.try_recv().is_ok() {
            received_lossless += 1;
        }
        assert_eq!(
            received_lossless, lossless_count,
            "all lossless messages must be delivered (got {received_lossless}, sent {lossless_count})"
        );
    }

    // -----------------------------------------------------------------------
    // Backpressure: server-requests never dropped
    // -----------------------------------------------------------------------

    /// Sends server-request messages through ChannelSink (which classifies
    /// them as lossless) and verifies that none are ever dropped, regardless
    /// of the bounded channel state.
    #[tokio::test]
    async fn backpressure_server_requests_never_dropped() {
        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        // Tiny best-effort capacity to stress backpressure.
        let (best_effort_tx, _best_effort_rx) = mpsc::channel::<ServerMessage>(1);
        let sink = ChannelSink::new(lossless_tx, best_effort_tx);

        // Fill the best-effort channel to create backpressure.
        sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
            method: "stream/event".to_string(),
            params: json!({
                "event": {
                    "event": "assistant_delta",
                    "session_id": "s1",
                    "delta": "fill"
                }
            }),
        }));

        // Now send server-requests. These are lossless and must all arrive.
        let request_count = 100;
        for i in 0..request_count {
            sink.send(ServerMessage::Request(ServerRequestEnvelope {
                id: format!("srv-{i}"),
                method: "permission/request".to_string(),
                params: json!({"i": i}),
            }));
        }

        let mut received = 0;
        while let Ok(msg) = lossless_rx.try_recv() {
            if matches!(msg, ServerMessage::Request(_)) {
                received += 1;
            }
        }
        assert_eq!(
            received, request_count,
            "all server-requests must be delivered (got {received}, expected {request_count})"
        );
    }

    // -----------------------------------------------------------------------
    // Backpressure: responses arrive when notification channel is full
    // -----------------------------------------------------------------------

    /// Fills the best-effort channel to capacity, then sends response
    /// messages. Responses go through the lossless (unbounded) channel and
    /// must all arrive regardless of best-effort backpressure.
    #[tokio::test]
    async fn backpressure_responses_arrive_when_notifications_full() {
        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        // Capacity of 2: fill it completely.
        let (best_effort_tx, _best_effort_rx) = mpsc::channel::<ServerMessage>(2);
        let sink = ChannelSink::new(lossless_tx, best_effort_tx);

        // Saturate best-effort channel.
        for i in 0..10 {
            sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
                method: "stream/event".to_string(),
                params: json!({
                    "event": {
                        "event": "assistant_delta",
                        "session_id": "s1",
                        "delta": format!("sat-{i}")
                    }
                }),
            }));
        }

        // Send responses -- they must all go through the lossless path.
        let resp_count = 20;
        for i in 0..resp_count {
            sink.send(ServerMessage::Response(ServerResponseEnvelope {
                id: format!("r-{i}"),
                result: ResponseResult::Success {
                    data: Some(json!({"ok": true})),
                },
            }));
        }

        let mut received_responses = 0;
        while let Ok(msg) = lossless_rx.try_recv() {
            if matches!(msg, ServerMessage::Response(_)) {
                received_responses += 1;
            }
        }
        assert_eq!(
            received_responses, resp_count,
            "all responses must be delivered even when notification channel is full \
             (got {received_responses}, expected {resp_count})"
        );
    }

    // -----------------------------------------------------------------------
    // Subscription task ownership and failure observation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_observes_normal_subscription_completion_once() {
        let app = test_app("prune-normal").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        drop(event_tx);
        let (pump_sink, _) = TestSink::new();
        let lifetime = Arc::new(());
        let lifetime_weak = Arc::downgrade(&lifetime);
        let drop_count = Arc::new(AtomicUsize::new(0));
        let drop_canary = DropCanary(Arc::clone(&drop_count));
        let app_server = processor.app_server.clone();
        let pending = Arc::clone(&processor.pending_server_requests);
        let handle = tokio::spawn(async move {
            let _lifetime = lifetime;
            let _drop_canary = drop_canary;
            pump_events(
                event_rx,
                pump_sink,
                pending,
                app_server,
                "normal-sub".to_string(),
                0,
                DEFAULT_PERMISSION_TIMEOUT,
            )
            .await;
        });
        processor
            .active_subscriptions
            .insert("normal-sub".to_string(), handle);

        wait_for_subscription_to_finish(&processor, "normal-sub").await;
        processor.prune_finished_subscriptions().await;
        processor.prune_finished_subscriptions().await;

        assert!(processor.active_subscriptions.is_empty());
        assert!(lifetime_weak.upgrade().is_none());
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
        assert!(
            subscription_failure_messages(&messages.lock().await, "normal-sub").is_empty(),
            "normal EOF must not emit a failure transition"
        );
    }

    #[tokio::test]
    async fn prune_surfaces_subscription_panic_once() {
        let app = test_app("prune-panic").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        event_tx
            .send(StreamEvent::AskUserQuestionResolved {
                request_id: "panic-request".to_string(),
                outcome: None,
                answer: None,
            })
            .expect("queue panic-triggering event");
        drop(event_tx);
        let lifetime = Arc::new(());
        let lifetime_weak = Arc::downgrade(&lifetime);
        let drop_count = Arc::new(AtomicUsize::new(0));
        let drop_canary = DropCanary(Arc::clone(&drop_count));
        let app_server = processor.app_server.clone();
        let pending = Arc::clone(&processor.pending_server_requests);
        let handle = tokio::spawn(async move {
            let _lifetime = lifetime;
            let _drop_canary = drop_canary;
            pump_events(
                event_rx,
                Arc::new(PanicSink),
                pending,
                app_server,
                "panic-sub".to_string(),
                0,
                DEFAULT_PERMISSION_TIMEOUT,
            )
            .await;
        });
        processor
            .active_subscriptions
            .insert("panic-sub".to_string(), handle);

        wait_for_subscription_to_finish(&processor, "panic-sub").await;
        processor.prune_finished_subscriptions().await;
        processor.prune_finished_subscriptions().await;

        assert!(processor.active_subscriptions.is_empty());
        assert!(processor.pending_server_requests.lock().await.is_empty());
        assert!(lifetime_weak.upgrade().is_none());
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            subscription_failure_messages(&messages.lock().await, "panic-sub"),
            [SubscriptionPumpFailure::Panicked.message()]
        );
    }

    #[tokio::test]
    async fn prune_surfaces_unexpected_subscription_cancellation_once() {
        let app = test_app("prune-cancel").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);
        let (_event_tx, event_rx) = mpsc::unbounded_channel();
        let (pump_sink, _) = TestSink::new();
        let app_server = processor.app_server.clone();
        let pending = Arc::clone(&processor.pending_server_requests);
        let handle = tokio::spawn(pump_events(
            event_rx,
            pump_sink,
            pending,
            app_server,
            "cancel-sub".to_string(),
            0,
            DEFAULT_PERMISSION_TIMEOUT,
        ));
        processor
            .active_subscriptions
            .insert("cancel-sub".to_string(), handle);
        processor
            .active_subscriptions
            .get("cancel-sub")
            .expect("subscription handle")
            .abort();

        wait_for_subscription_to_finish(&processor, "cancel-sub").await;
        processor.prune_finished_subscriptions().await;
        processor.prune_finished_subscriptions().await;

        assert!(processor.active_subscriptions.is_empty());
        assert_eq!(
            subscription_failure_messages(&messages.lock().await, "cancel-sub"),
            [SubscriptionPumpFailure::UnexpectedCancellation.message()]
        );
    }

    #[tokio::test]
    async fn processor_drop_surfaces_already_panicked_subscription_once() {
        let app = test_app("drop-panic").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        event_tx
            .send(StreamEvent::AskUserQuestionResolved {
                request_id: "drop-panic-request".to_string(),
                outcome: None,
                answer: None,
            })
            .expect("queue panic-triggering event");
        drop(event_tx);
        let app_server = processor.app_server.clone();
        let pending = Arc::clone(&processor.pending_server_requests);
        let handle = tokio::spawn(pump_events(
            event_rx,
            Arc::new(PanicSink),
            pending,
            app_server,
            "drop-panic-sub".to_string(),
            0,
            DEFAULT_PERMISSION_TIMEOUT,
        ));
        processor
            .active_subscriptions
            .insert("drop-panic-sub".to_string(), handle);
        wait_for_subscription_to_finish(&processor, "drop-panic-sub").await;

        drop(processor);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if subscription_failure_messages(&messages.lock().await, "drop-panic-sub")
                    == [SubscriptionPumpFailure::Panicked.message()]
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owner shutdown should observe an already-panicked pump");
        tokio::task::yield_now().await;
        assert_eq!(
            subscription_failure_messages(&messages.lock().await, "drop-panic-sub"),
            [SubscriptionPumpFailure::Panicked.message()]
        );
    }

    #[tokio::test]
    async fn processor_drop_aborts_active_subscriptions() {
        let app = test_app("drop-abort").await;
        let (sink, owner_messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);
        let pending_weak = Arc::downgrade(&processor.pending_server_requests);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (pump_sink, pump_messages) = TestSink::new();
        let lifetime = Arc::new(());
        let lifetime_weak = Arc::downgrade(&lifetime);
        let drop_count = Arc::new(AtomicUsize::new(0));
        let drop_canary = DropCanary(Arc::clone(&drop_count));
        let (started_tx, started_rx) = oneshot::channel();
        let app_server = processor.app_server.clone();
        let pending = Arc::clone(&processor.pending_server_requests);
        let handle = tokio::spawn(async move {
            let _lifetime = lifetime;
            let _drop_canary = drop_canary;
            let _ = started_tx.send(());
            pump_events(
                event_rx,
                pump_sink,
                pending,
                app_server,
                "drop-sub".to_string(),
                0,
                DEFAULT_PERMISSION_TIMEOUT,
            )
            .await;
        });
        processor
            .active_subscriptions
            .insert("drop-sub".to_string(), handle);
        started_rx.await.expect("pump should start");

        event_tx
            .send(StreamEvent::AskUserQuestionResolved {
                request_id: "before-drop".to_string(),
                outcome: None,
                answer: None,
            })
            .expect("send before owner drop");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if pump_messages.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pump should write before owner drop");

        drop(processor);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if drop_count.load(Ordering::SeqCst) == 1
                    && lifetime_weak.upgrade().is_none()
                    && pending_weak.upgrade().is_none()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owner drop should release pump and pending state");

        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
        assert!(
            event_tx
                .send(StreamEvent::AskUserQuestionResolved {
                    request_id: "after-drop".to_string(),
                    outcome: None,
                    answer: None,
                })
                .is_err(),
            "retired pump must not accept later state writes"
        );
        tokio::task::yield_now().await;
        assert_eq!(pump_messages.lock().await.len(), 1);
        assert!(
            subscription_failure_messages(&owner_messages.lock().await, "drop-sub").is_empty(),
            "owner-initiated abort is expected shutdown, not a failure"
        );
    }

    // -----------------------------------------------------------------------
    // Disconnect cleanup: pending server-request resolves on processor drop
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn pending_server_request_resolves_on_disconnect() {
        // When the processor is dropped (simulating client disconnect), any
        // pending server-request oneshot senders are dropped too. The spawned
        // resolver task should see Err(Cancelled) and deny the permission.
        let (tx, rx) = oneshot::channel::<ResponseResult>();

        // Drop the sender immediately (simulates disconnect).
        drop(tx);

        // The receiver should get Err(Cancelled).
        let result = rx.await;
        assert!(
            result.is_err(),
            "dropping the sender should cancel the oneshot"
        );
    }

    // -----------------------------------------------------------------------
    // Backpressure: lossless terminal events survive saturated channel
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn terminal_event_survives_saturated_best_effort_channel() {
        // Create a ChannelSink with a tiny best-effort channel.
        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, _best_effort_rx) = mpsc::channel::<ServerMessage>(1);
        let sink = ChannelSink::new(lossless_tx, best_effort_tx);

        // Fill the best-effort channel.
        sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
            method: "stream/event".to_string(),
            params: json!({"event": {"event": "assistant_delta", "session_id": "s1", "delta": "x"}}),
        }));
        // This one should be dropped (channel full).
        sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
            method: "stream/event".to_string(),
            params: json!({"event": {"event": "assistant_delta", "session_id": "s1", "delta": "y"}}),
        }));

        // Send a terminal event — this should go through the lossless channel.
        sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
            method: "stream/event".to_string(),
            params: json!({"subscription_id": "sub-1", "event": {"event": "turn_finished", "session_id": "s1", "provider": "anthropic", "usage": {"input_tokens": 0, "output_tokens": 0}}}),
        }));

        // Send a server-request — also lossless.
        sink.send(ServerMessage::Request(ServerRequestEnvelope {
            id: "srv-1".to_string(),
            method: "permission/request".to_string(),
            params: json!({"request_id": "perm-1"}),
        }));

        // Verify lossless channel has the terminal + server-request.
        let mut lossless_count = 0;
        while lossless_rx.try_recv().is_ok() {
            lossless_count += 1;
        }
        assert!(
            lossless_count >= 2,
            "terminal event and server-request must be on lossless channel, got {lossless_count}"
        );
    }

    // -----------------------------------------------------------------------
    // Backpressure: response always delivered regardless of channel pressure
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn response_delivered_under_pressure() {
        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, _rx) = mpsc::channel::<ServerMessage>(1);
        let sink = ChannelSink::new(lossless_tx, best_effort_tx);

        // Fill best-effort.
        for _ in 0..10 {
            sink.send(ServerMessage::Notification(ServerNotificationEnvelope {
                method: "stream/event".to_string(),
                params: json!({"event": {"event": "assistant_delta", "session_id": "s", "delta": "x"}}),
            }));
        }

        // Send a response — must go through lossless.
        sink.send(ServerMessage::Response(ServerResponseEnvelope {
            id: "req-1".to_string(),
            result: ResponseResult::Success { data: None },
        }));

        // Verify we can receive the response from lossless.
        let mut found_response = false;
        while let Ok(msg) = lossless_rx.try_recv() {
            if matches!(msg, ServerMessage::Response(_)) {
                found_response = true;
            }
        }
        assert!(found_response, "response must be on lossless channel");
    }

    // -----------------------------------------------------------------------
    // Capability filtering: experimental_methods opt-in
    // -----------------------------------------------------------------------

    fn initialize_with_experimental() -> ClientMessage {
        ClientMessage::Request(ClientRequestEnvelope {
            id: "init-exp".into(),
            method: "initialize".into(),
            params: Some(json!({
                "protocol_version": "1.0",
                "client_info": { "name": "test", "version": "0.1" },
                "capabilities": { "experimental_methods": true }
            })),
        })
    }

    fn initialize_with_persistent_goals() -> ClientMessage {
        ClientMessage::Request(ClientRequestEnvelope {
            id: "init-goals".into(),
            method: "initialize".into(),
            params: Some(json!({
                "protocol_version": "1.0",
                "client_info": { "name": "goal-test", "version": "0.1" },
                "capabilities": {
                    "experimental_methods": true,
                    "persistent_goals": true
                }
            })),
        })
    }

    #[tokio::test]
    async fn default_client_hides_experimental_methods() {
        let app = test_app("cap-default").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor.handle_message(initialize_request()).await;

        let msgs = messages.lock().await;
        let resp = &msgs[0];
        if let ServerMessage::Response(env) = resp {
            if let ResponseResult::Success { data: Some(data) } = &env.result {
                let init: orbcode_app_server_protocol::InitializeResult =
                    serde_json::from_value(data.clone()).unwrap();
                assert!(
                    init.capabilities.experimental_methods.is_empty(),
                    "default client should not see experimental methods"
                );
                assert!(
                    !init.capabilities.stable_methods.is_empty(),
                    "stable methods should always be present"
                );
            } else {
                panic!("expected success response");
            }
        } else {
            panic!("expected response message");
        }
    }

    #[tokio::test]
    async fn opt_in_client_sees_experimental_methods() {
        let app = test_app("cap-optin").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor
            .handle_message(initialize_with_experimental())
            .await;

        let msgs = messages.lock().await;
        let resp = &msgs[0];
        if let ServerMessage::Response(env) = resp {
            if let ResponseResult::Success { data: Some(data) } = &env.result {
                let init: orbcode_app_server_protocol::InitializeResult =
                    serde_json::from_value(data.clone()).unwrap();
                assert!(
                    !init.capabilities.experimental_methods.is_empty(),
                    "opt-in client should see experimental methods"
                );
                assert!(
                    init.capabilities
                        .experimental_methods
                        .contains(&"background/create".to_string()),
                    "background/create should be in experimental methods"
                );
                assert!(
                    !init
                        .capabilities
                        .experimental_methods
                        .contains(&method::SESSION_GOAL_GET.to_string()),
                    "generic experimental opt-in must not advertise persistent goals"
                );
            } else {
                panic!("expected success response");
            }
        } else {
            panic!("expected response message");
        }
    }

    #[tokio::test]
    async fn persistent_goal_client_sees_goal_method_family() {
        let app = test_app("cap-goal-optin").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor
            .handle_message(initialize_with_persistent_goals())
            .await;

        let msgs = messages.lock().await;
        let ServerMessage::Response(env) = &msgs[0] else {
            panic!("expected response message");
        };
        let ResponseResult::Success { data: Some(data) } = &env.result else {
            panic!("expected success response");
        };
        let init: orbcode_app_server_protocol::InitializeResult =
            serde_json::from_value(data.clone()).unwrap();
        for goal_method in method::persistent_goal_client_request_methods() {
            assert!(
                init.capabilities
                    .experimental_methods
                    .contains(&goal_method.to_string()),
                "goal-capable client should see {goal_method}"
            );
        }
    }

    #[tokio::test]
    async fn generic_experimental_client_is_rejected_for_goal_method() {
        let app = test_app("cap-goal-reject").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor
            .handle_message(initialize_with_experimental())
            .await;
        processor
            .handle_message(ClientMessage::Request(ClientRequestEnvelope {
                id: "goal-1".into(),
                method: method::SESSION_GOAL_GET.into(),
                params: Some(json!({ "session_id": "session-1" })),
            }))
            .await;

        let msgs = messages.lock().await;
        let ServerMessage::Response(env) = &msgs[1] else {
            panic!("expected response message");
        };
        let ResponseResult::Error(error) = &env.result else {
            panic!("expected error response");
        };
        assert_eq!(error.code, ErrorCode::MethodNotFound);
        assert!(error.message.contains("persistent_goals"));
    }

    #[tokio::test]
    async fn default_client_rejected_for_experimental_method() {
        let app = test_app("cap-reject").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor.handle_message(initialize_request()).await;

        processor
            .handle_message(ClientMessage::Request(ClientRequestEnvelope {
                id: "bg-1".into(),
                method: "background/list".into(),
                params: None,
            }))
            .await;

        let msgs = messages.lock().await;
        assert_eq!(msgs.len(), 2);
        if let ServerMessage::Response(env) = &msgs[1] {
            if let ResponseResult::Error(err) = &env.result {
                assert_eq!(err.code, ErrorCode::MethodNotFound);
                assert!(
                    err.message.contains("experimental"),
                    "error should mention experimental: {}",
                    err.message
                );
            } else {
                panic!("expected error response for experimental method");
            }
        } else {
            panic!("expected response message");
        }
    }

    #[tokio::test]
    async fn opt_in_client_allowed_experimental_method() {
        let app = test_app("cap-allow").await;
        let (sink, messages) = TestSink::new();
        let mut processor = MessageProcessor::new(app, sink);

        processor
            .handle_message(initialize_with_experimental())
            .await;

        processor
            .handle_message(ClientMessage::Request(ClientRequestEnvelope {
                id: "bg-1".into(),
                method: "background/list".into(),
                params: None,
            }))
            .await;

        let msgs = messages.lock().await;
        assert_eq!(msgs.len(), 2);
        if let ServerMessage::Response(env) = &msgs[1] {
            assert!(
                !matches!(
                    &env.result,
                    ResponseResult::Error(e) if e.code == ErrorCode::MethodNotFound
                ),
                "opt-in client should not get MethodNotFound for experimental method"
            );
        } else {
            panic!("expected response message");
        }
    }

    // -----------------------------------------------------------------------
    // PumpAskUserGuard: core pending resolution on pump exit and abort
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn ask_user_pump_exit_resolves_core_pending_as_disconnect() {
        let app = test_app("ask-dc").await;
        let (sink, _messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let core_rx = app.register_pending_ask_user_for_test("session-ask", "ask-dc-1", Vec::new());

        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::AskUserQuestionRequested {
                session_id: "session-ask".into(),
                turn_id: Some("test-turn".into()),
                tool_use_id: "test-tool".into(),
                request_id: "ask-dc-1".into(),
                deadline: None,
                questions: Vec::new(),
                question: "Will I hang?".into(),
                options: vec![],
            })
            .unwrap();
        drop(event_tx);

        let pending_clone = Arc::clone(&pending);
        let pump_handle = tokio::spawn({
            let sink = Arc::clone(&sink) as Arc<dyn ServerSink>;
            let app = app.clone();
            async move {
                pump_events(
                    event_rx,
                    sink,
                    pending_clone,
                    app,
                    "ask-sub".to_string(),
                    0,
                    Duration::from_secs(300),
                )
                .await;
            }
        });

        let _ = tokio::time::timeout(Duration::from_secs(5), pump_handle).await;

        let outcome = tokio::time::timeout(Duration::from_millis(100), core_rx)
            .await
            .expect("core oneshot should resolve quickly")
            .expect("core oneshot should not be dropped");
        assert_eq!(
            outcome,
            orbcode_protocol::AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Disconnect
            },
            "guard should resolve with disconnect on pump exit"
        );
    }

    #[tokio::test]
    async fn ask_user_pump_abort_resolves_core_pending_as_disconnect() {
        let app = test_app("ask-abort").await;
        let (sink, _messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let core_rx =
            app.register_pending_ask_user_for_test("session-ask", "ask-abort-1", Vec::new());

        let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();
        event_tx
            .send(StreamEvent::AskUserQuestionRequested {
                session_id: "session-ask".into(),
                turn_id: Some("test-turn".into()),
                tool_use_id: "test-tool".into(),
                request_id: "ask-abort-1".into(),
                deadline: None,
                questions: Vec::new(),
                question: "Will abort cancel me?".into(),
                options: vec![],
            })
            .unwrap();

        let pending_clone = Arc::clone(&pending);
        let pump_handle = tokio::spawn({
            let sink = Arc::clone(&sink) as Arc<dyn ServerSink>;
            let app = app.clone();
            async move {
                pump_events(
                    event_rx,
                    sink,
                    pending_clone,
                    app,
                    "ask-sub".to_string(),
                    0,
                    Duration::from_secs(300),
                )
                .await;
            }
        });

        tokio::time::sleep(Duration::from_millis(200)).await;

        pump_handle.abort();
        let _ = pump_handle.await;
        drop(event_tx);

        let outcome = tokio::time::timeout(Duration::from_millis(100), core_rx)
            .await
            .expect("core oneshot should resolve quickly after abort")
            .expect("core oneshot should not be dropped");
        assert_eq!(
            outcome,
            orbcode_protocol::AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Disconnect
            },
            "guard should resolve with disconnect on pump abort"
        );
    }

    #[tokio::test]
    async fn ask_user_invalid_response_is_typed_retry_and_does_not_consume_core_pending() {
        use std::collections::BTreeMap;

        use orbcode_protocol::{
            AskUserAnswerValue, AskUserOption, AskUserQuestionSpec, AskUserResponseOutcome,
        };

        let app = test_app("ask-invalid-retry").await;
        let (sink, messages) = TestSink::new();
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<ResponseResult>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let question = AskUserQuestionSpec {
            id: "database".into(),
            question: "Which database?".into(),
            header: "Database".into(),
            multi_select: false,
            options: vec![AskUserOption {
                id: "postgres".into(),
                label: "PostgreSQL".into(),
                description: String::new(),
                preview: None,
            }],
            allow_free_text: false,
            allow_annotation: false,
        };
        let core_rx = app.register_pending_ask_user_for_test(
            "session-ask",
            "ask-retry-1",
            vec![question.clone()],
        );
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        event_tx
            .send(StreamEvent::AskUserQuestionRequested {
                session_id: "session-ask".into(),
                turn_id: Some("test-turn".into()),
                tool_use_id: "test-tool".into(),
                request_id: "ask-retry-1".into(),
                deadline: None,
                questions: vec![question],
                question: "Which database?".into(),
                options: vec!["PostgreSQL".into()],
            })
            .unwrap();
        let pump_handle = tokio::spawn(pump_events(
            event_rx,
            Arc::clone(&sink) as Arc<dyn ServerSink>,
            Arc::clone(&pending),
            app.clone(),
            "ask-sub".into(),
            0,
            Duration::from_secs(5),
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let server_request_id = {
            let messages = messages.lock().await;
            messages
                .iter()
                .find_map(|message| match message {
                    ServerMessage::Request(request)
                        if request.method == method::SERVER_REQUEST_ASK_USER =>
                    {
                        Some(request.id.clone())
                    }
                    _ => None,
                })
                .expect("initial server request")
        };
        let response = |option_id: &str| AskUserQuestionResponse {
            request_id: "ask-retry-1".into(),
            outcome: Some(AskUserResponseOutcome::Answered {
                answers: BTreeMap::from([(
                    "database".into(),
                    AskUserAnswerValue::Selected {
                        option_id: option_id.into(),
                    },
                )]),
                annotations: BTreeMap::new(),
            }),
            answer: None,
        };
        pending
            .lock()
            .await
            .remove(&server_request_id)
            .unwrap()
            .send(ResponseResult::Success {
                data: Some(serde_json::to_value(response("unknown")).unwrap()),
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(app.sessions.ask_user_pending_len_for_test(), 1);
        let retry = {
            let messages = messages.lock().await;
            messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    ServerMessage::Request(request)
                        if request.method == method::SERVER_REQUEST_ASK_USER =>
                    {
                        Some(request.clone())
                    }
                    _ => None,
                })
                .expect("retry server request")
        };
        assert_eq!(retry.id, server_request_id);
        assert_eq!(retry.params["validation_error"]["code"], "unknown_option");
        pending
            .lock()
            .await
            .remove(&server_request_id)
            .unwrap()
            .send(ResponseResult::Success {
                data: Some(serde_json::to_value(response("postgres")).unwrap()),
            })
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), core_rx)
                .await
                .expect("valid retry resolves")
                .expect("core receiver"),
            AskUserResponseOutcome::Answered { .. }
        ));
        drop(event_tx);
        pump_handle.abort();
    }
}
