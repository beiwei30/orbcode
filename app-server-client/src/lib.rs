//! Protocol-preserving async client facade for AppServer.
//!
//! `AppClient` provides a high-level, strongly-typed API that mirrors the
//! public surface of [`orbcode_app_server::AppServer`]. Internally it delegates
//! through an [`InProcessTransport`] which routes every call through the full
//! protocol path via [`orbcode_app_server::message_processor::MessageProcessor`].
//!
//! All requests are serialised into protocol DTOs, processed by the
//! MessageProcessor (including the initialize handshake), and the responses
//! are deserialised back into typed Rust values. This ensures in-process
//! callers exercise the same contract as socket and WebSocket transports.

mod error;
#[cfg(feature = "in-process")]
mod in_process;
#[cfg(unix)]
mod ndjson_transport;
mod transport;
mod websocket_transport;

pub use error::ClientError;
#[cfg(feature = "in-process")]
pub use in_process::InProcessTransport;
#[cfg(unix)]
pub use ndjson_transport::NdjsonTransport;
pub use transport::ClientTransport;
pub use websocket_transport::WebSocketTransport;

pub use orbcode_app_server_protocol::*;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

#[cfg(feature = "in-process")]
use orbcode_app_server::{AppConfigOverrides, AppServer};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
#[cfg(feature = "in-process")]
use std::path::PathBuf;
use tokio::sync::{Mutex, mpsc};

/// High-level async client for AppServer operations.
///
/// `AppClient` wraps a [`ClientTransport`] implementation and exposes typed
/// async methods for every major AppServer operation. Every call goes through
/// the protocol message path (initialize, request/response, notifications,
/// server requests), ensuring protocol correctness regardless of transport.
pub struct AppClient {
    transport: Box<dyn ClientTransport>,
    #[cfg(feature = "test-support")]
    test_server: Option<AppServer>,
    stream_routes: Arc<Mutex<HashMap<String, StreamRoute>>>,
    pending_stream_events: Arc<Mutex<HashMap<String, VecDeque<orbcode_protocol::StreamEvent>>>>,
    pending_server_requests: Arc<Mutex<PendingServerRequests>>,
    notification_rx: Mutex<Option<mpsc::Receiver<ServerNotificationEnvelope>>>,
    server_request_rx: Mutex<Option<mpsc::Receiver<ServerRequestEnvelope>>>,
}

struct StreamRoute {
    tx: mpsc::UnboundedSender<orbcode_protocol::StreamEvent>,
    close_on_terminal_background_task: bool,
}

#[derive(serde::Deserialize)]
struct PermissionModeValue {
    mode: String,
}

#[derive(serde::Deserialize)]
struct ToolNameValue {
    name: String,
}

#[derive(Default)]
struct PendingServerRequests {
    permissions: HashMap<String, String>,
    mcp_trust: HashMap<String, String>,
    ask_user: HashMap<String, String>,
}

impl AppClient {
    /// Create a client backed by an in-process `AppServer`.
    ///
    /// This constructor sends the `initialize` handshake automatically. If
    /// initialization fails, an error is returned and the transport is torn
    /// down.
    #[cfg(feature = "in-process")]
    pub async fn new(app_server: AppServer) -> Result<Self, ClientError> {
        Self::new_in_process(app_server).await
    }

    /// Create a client backed by an in-process `AppServer` (same as `new`).
    #[cfg(feature = "in-process")]
    pub async fn new_in_process(app_server: AppServer) -> Result<Self, ClientError> {
        let transport = InProcessTransport::new(app_server.clone());
        let client = Self::from_transport(Box::new(transport)).await?;
        #[cfg(feature = "test-support")]
        {
            let mut client = client;
            client.test_server = Some(app_server);
            Ok(client)
        }
        #[cfg(not(feature = "test-support"))]
        Ok(client)
    }

    /// Construct an `AppServer` from a working directory and config overrides,
    /// then wrap it in an `AppClient` with automatic initialization. This is a
    /// convenience that combines [`AppServer::new`] and [`AppClient::new`].
    #[cfg(feature = "in-process")]
    pub async fn from_cwd(
        cwd: impl Into<PathBuf>,
        overrides: AppConfigOverrides,
    ) -> Result<Self, ClientError> {
        let app_server = AppServer::new(cwd, overrides)
            .await
            .map_err(ClientError::Core)?;
        Self::new_in_process(app_server).await
    }

    /// Connect to a `orbcode serve --socket <path>` server.
    /// Authenticates with the token and sends `initialize` automatically.
    pub async fn connect_socket(
        path: &std::path::Path,
        auth_token: &str,
    ) -> Result<Self, ClientError> {
        #[cfg(not(unix))]
        {
            let _ = (path, auth_token);
            return Err(ClientError::Transport(
                "Unix socket transport is not supported on this platform".into(),
            ));
        }
        #[cfg(unix)]
        let transport = NdjsonTransport::connect(path, auth_token).await?;
        #[cfg(unix)]
        Self::from_transport(Box::new(transport)).await
    }

    /// Connect to a `orbcode serve --websocket <addr>` server.
    /// Authenticates with the token and sends `initialize` automatically.
    pub async fn connect_websocket(endpoint: &str, auth_token: &str) -> Result<Self, ClientError> {
        let transport = WebSocketTransport::connect(endpoint, auth_token).await?;
        Self::from_transport(Box::new(transport)).await
    }

    /// Create a client from an already-constructed transport. The caller is
    /// responsible for ensuring the transport is connected and ready.
    /// Sends the `initialize` handshake automatically.
    pub async fn from_transport(transport: Box<dyn ClientTransport>) -> Result<Self, ClientError> {
        let (notification_tx, notification_rx) = mpsc::channel(256);
        let (server_request_tx, server_request_rx) = mpsc::channel(64);
        let client = Self {
            transport,
            #[cfg(feature = "test-support")]
            test_server: None,
            stream_routes: Arc::new(Mutex::new(HashMap::new())),
            pending_stream_events: Arc::new(Mutex::new(HashMap::new())),
            pending_server_requests: Arc::new(Mutex::new(PendingServerRequests::default())),
            notification_rx: Mutex::new(Some(notification_rx)),
            server_request_rx: Mutex::new(Some(server_request_rx)),
        };
        client.initialize().await?;
        client
            .start_router_tasks(notification_tx, server_request_tx)
            .await;
        Ok(client)
    }

    /// Access the underlying transport for taking notification/request receivers.
    pub fn transport(&self) -> &dyn ClientTransport {
        &*self.transport
    }

    #[cfg(feature = "test-support")]
    pub fn app_server(&self) -> Option<&AppServer> {
        self.test_server.as_ref()
    }

    async fn start_router_tasks(
        &self,
        notification_tx: mpsc::Sender<ServerNotificationEnvelope>,
        server_request_tx: mpsc::Sender<ServerRequestEnvelope>,
    ) {
        if let Some(notification_rx) = self.transport.take_notification_receiver().await {
            // Detached router; it exits when the transport notification channel closes.
            std::mem::drop(tokio::spawn(route_notifications(
                notification_rx,
                notification_tx,
                Arc::clone(&self.stream_routes),
                Arc::clone(&self.pending_stream_events),
            )));
        }

        if let Some(server_request_rx) = self.transport.take_server_request_receiver().await {
            // Detached router; it exits when the transport server-request channel closes.
            std::mem::drop(tokio::spawn(route_server_requests(
                server_request_rx,
                server_request_tx,
                Arc::clone(&self.pending_server_requests),
            )));
        }
    }

    /// Send a request and deserialize the response into a typed result.
    ///
    /// This is a convenience wrapper around `transport.request()` that handles
    /// the `serde_json::from_value` step internally. Callers can use it to
    /// avoid manual deserialization when the response type is known:
    ///
    /// ```rust,ignore
    /// let sessions: Vec<SessionSummary> = client.request_typed("session/list", None).await?;
    /// ```
    pub async fn request_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T, ClientError> {
        let value = self.transport.request(method, params).await?;
        serde_json::from_value(value).map_err(ClientError::Serialization)
    }

    async fn request_no_data(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<NoData, ClientError> {
        self.request_typed(method, params).await
    }

    // =========================================================================
    // Protocol lifecycle
    // =========================================================================

    /// Send the `initialize` handshake. Called automatically by [`new`].
    async fn initialize(&self) -> Result<InitializeResult, ClientError> {
        let result = self
            .transport
            .request(
                "initialize",
                Some(json!({
                    "protocol_version": "1.0",
                    "client_info": {
                        "name": "orbcode-client",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {
                        "streaming": true,
                        "experimental_methods": true,
                    },
                })),
            )
            .await?;
        serde_json::from_value(result).map_err(ClientError::Serialization)
    }

    // =========================================================================
    // Lifecycle
    // =========================================================================

    /// Start a new session or resume an existing one.
    pub async fn bootstrap(
        &self,
        requested_session: Option<&str>,
    ) -> Result<BootstrapState, ClientError> {
        let params = requested_session
            .map(|session_id| {
                serde_json::to_value(BootstrapParams {
                    session_id: Some(session_id.to_string()),
                    ..BootstrapParams::default()
                })
            })
            .transpose()?;
        self.request_typed(method::SESSION_BOOTSTRAP, params).await
    }

    pub async fn bootstrap_with_params(
        &self,
        params: BootstrapParams,
    ) -> Result<BootstrapState, ClientError> {
        self.request_typed(
            method::SESSION_BOOTSTRAP,
            Some(serde_json::to_value(params)?),
        )
        .await
    }

    /// Load a session record for read-only viewing (e.g. a workflow agent
    /// step's output). This never mutates the live session's runtime context
    /// (cwd/permissions/effort) or the live-session registry.
    pub async fn load_session_view(&self, session_id: &str) -> Result<BootstrapState, ClientError> {
        self.bootstrap_with_params(BootstrapParams {
            session_id: Some(session_id.to_string()),
            read_only: true,
            ..BootstrapParams::default()
        })
        .await
    }

    // =========================================================================
    // Sessions
    // =========================================================================

    /// List all persisted sessions.
    pub async fn list_sessions(&self) -> Result<SessionListResult, ClientError> {
        self.request_typed(method::SESSION_LIST, None).await
    }

    /// Rename a session.
    pub async fn rename_session(
        &self,
        session_id: &str,
        new_title: &str,
    ) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::SESSION_RENAME,
            Some(serde_json::to_value(SessionRenameParams {
                session_id: session_id.to_string(),
                new_title: new_title.to_string(),
            })?),
        )
        .await
    }

    /// Fork a session into a new independent copy.
    pub async fn fork_session(
        &self,
        session_id: &str,
        title: Option<String>,
        note: Option<String>,
    ) -> Result<SessionForkResult, ClientError> {
        self.request_typed(
            method::SESSION_FORK,
            Some(serde_json::to_value(SessionForkParams {
                session_id: session_id.to_string(),
                title,
                note,
            })?),
        )
        .await
    }

    pub async fn acp_load_replay_preflight(
        &self,
        session_id: &str,
    ) -> Result<AcpLoadReplayPreflight, ClientError> {
        self.request_typed(
            method::SESSION_ACP_LOAD_PREFLIGHT,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    pub async fn acp_load_setup(
        &self,
        params: BootstrapParams,
    ) -> Result<BootstrapState, ClientError> {
        self.request_typed(
            method::SESSION_ACP_LOAD_SETUP,
            Some(serde_json::to_value(params)?),
        )
        .await
    }

    pub async fn acp_resume_setup(
        &self,
        params: BootstrapParams,
    ) -> Result<BootstrapState, ClientError> {
        self.request_typed(
            method::SESSION_ACP_RESUME_SETUP,
            Some(serde_json::to_value(params)?),
        )
        .await
    }

    pub async fn acp_delete_session(
        &self,
        params: AcpDeleteSessionParams,
    ) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::SESSION_ACP_DELETE,
            Some(serde_json::to_value(params)?),
        )
        .await
    }

    pub async fn acp_close_session(&self, session_id: &str) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::SESSION_ACP_CLOSE,
            Some(serde_json::to_value(SessionIdParams {
                session_id: session_id.to_string(),
            })?),
        )
        .await
    }

    /// Clear a session and start fresh.
    pub async fn clear_session(
        &self,
        previous_session_id: &str,
    ) -> Result<BootstrapState, ClientError> {
        self.request_typed(
            method::SESSION_CLEAR,
            Some(serde_json::to_value(SessionIdParams {
                session_id: previous_session_id.to_string(),
            })?),
        )
        .await
    }

    /// Record a system message in a session transcript.
    pub async fn record_system_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::SESSION_RECORD_MESSAGE,
            Some(serde_json::to_value(SessionRecordMessageParams {
                session_id: session_id.to_string(),
                message: message.to_string(),
            })?),
        )
        .await
    }

    /// Rewind a session to keep only the first `keep_messages` messages.
    pub async fn rewind_session(
        &self,
        session_id: &str,
        keep_messages: usize,
    ) -> Result<BootstrapState, ClientError> {
        self.request_typed(
            method::SESSION_REWIND,
            Some(serde_json::to_value(SessionRewindParams {
                session_id: session_id.to_string(),
                keep_messages,
            })?),
        )
        .await
    }

    /// Compact a session (summarize older messages to free context).
    pub async fn compact_session(
        &self,
        session_id: &str,
    ) -> Result<CompactSessionResult, ClientError> {
        self.request_typed(
            method::SESSION_COMPACT,
            Some(serde_json::to_value(SessionIdParams {
                session_id: session_id.to_string(),
            })?),
        )
        .await
    }

    /// Evaluate whether a manual compact is recommended for this session.
    pub async fn compact_decision(&self, session_id: &str) -> Result<CompactDecision, ClientError> {
        self.request_typed(
            method::SESSION_COMPACT_DECISION,
            Some(serde_json::to_value(SessionIdParams {
                session_id: session_id.to_string(),
            })?),
        )
        .await
    }

    // =========================================================================
    // Turns
    // =========================================================================

    /// Submit a new conversational turn. Returns a `TurnSubscription` with the
    /// subscription ID. Stream events arrive via the notification receiver
    /// (obtained from `transport().take_notification_receiver()`).
    pub async fn submit_turn(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<TurnSubmitResult, ClientError> {
        self.request_typed(
            method::TURN_SUBMIT,
            Some(serde_json::to_value(TurnSubmitParams {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
            })?),
        )
        .await
    }

    /// Inject a steering prompt into an active turn.
    pub async fn steer_turn(&self, session_id: &str, prompt: &str) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::TURN_STEER,
            Some(serde_json::to_value(TurnSubmitParams {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
            })?),
        )
        .await
    }

    /// Cancel the active turn for a session. Returns `true` if a turn was
    /// actually cancelled, `false` if no turn was active.
    pub async fn cancel_turn(&self, session_id: &str) -> Result<bool, ClientError> {
        let result: TurnCancelResult = self
            .request_typed(
                method::TURN_CANCEL,
                Some(serde_json::to_value(SessionIdParams {
                    session_id: session_id.to_string(),
                })?),
            )
            .await?;
        Ok(result.cancelled)
    }

    /// Interrupt the active turn (tool-level cancellation). Returns `true` if
    /// a turn was actually interrupted, `false` if no turn was active.
    pub async fn interrupt_turn(&self, session_id: &str) -> Result<bool, ClientError> {
        let result: TurnInterruptResult = self
            .request_typed(
                method::TURN_INTERRUPT,
                Some(serde_json::to_value(SessionIdParams {
                    session_id: session_id.to_string(),
                })?),
            )
            .await?;
        Ok(result.interrupted)
    }

    /// Respond to a pending permission request from a tool execution.
    ///
    /// The `decision` must be a [`PermissionDecisionWire`] which serializes as
    /// `{"decision": "approve"}` (internally tagged). Sending a plain string
    /// would fail server-side deserialization.
    pub async fn respond_to_permission_request(
        &self,
        request_id: &str,
        decision: orbcode_app_server_protocol::PermissionDecisionWire,
    ) -> Result<SentResult, ClientError> {
        let params = orbcode_app_server_protocol::PermissionResponseParams {
            request_id: request_id.to_string(),
            decision,
        };
        self.request_typed(
            method::PERMISSION_RESPOND,
            Some(serde_json::to_value(params)?),
        )
        .await
    }

    /// Respond through the client-request fallback using the core decision type.
    pub async fn respond_to_permission_request_decision(
        &self,
        request_id: &str,
        decision: PermissionDecision,
    ) -> Result<SentResult, ClientError> {
        self.respond_to_permission_request(request_id, permission_decision_to_wire(decision))
            .await
    }

    /// Respond to a server-initiated request (e.g. a permission prompt
    /// received as a server-request rather than via the fallback
    /// `permission/respond` method).
    pub async fn respond_to_server_request(
        &self,
        id: String,
        result: ResponseResult,
    ) -> Result<(), ClientError> {
        self.transport.respond_to_server_request(id, result).await
    }

    /// Reject a server-initiated request that this client surface cannot serve.
    pub async fn reject_server_request(
        &self,
        id: String,
        message: impl Into<String>,
    ) -> Result<(), ClientError> {
        self.respond_to_server_request(
            id,
            ResponseResult::Error(orbcode_app_server_protocol::ProtocolError {
                code: orbcode_app_server_protocol::ErrorCode::InvalidRequest,
                message: message.into(),
                data: None,
            }),
        )
        .await
    }

    // =========================================================================
    // Context / Usage / Cost
    // =========================================================================

    /// Preview the current turn context.
    pub async fn context_preview(&self) -> Result<TurnContext, ClientError> {
        self.request_typed(method::CONTEXT_PREVIEW, None).await
    }

    /// Full context overview including token usage and diagnostics.
    pub async fn context_overview(&self, session_id: &str) -> Result<ContextOverview, ClientError> {
        self.request_typed(
            method::CONTEXT_OVERVIEW,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    /// Full context overview without caller-side JSON field extraction.
    pub async fn context_overview_typed(
        &self,
        session_id: &str,
    ) -> Result<ContextOverview, ClientError> {
        self.request_typed(
            method::CONTEXT_OVERVIEW,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    /// Token usage overview for a session.
    pub async fn usage_overview(&self, session_id: &str) -> Result<UsageOverview, ClientError> {
        self.request_typed(
            method::USAGE_OVERVIEW,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    /// Cost overview for a session.
    pub async fn cost_overview(&self, session_id: &str) -> Result<CostOverview, ClientError> {
        self.request_typed(
            method::USAGE_COST,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    pub async fn cost_overview_typed(&self, session_id: &str) -> Result<CostOverview, ClientError> {
        self.request_typed(
            method::USAGE_COST,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    /// Aggregate statistics across all sessions.
    pub async fn stats_overview(&self) -> Result<StatsOverview, ClientError> {
        self.request_typed(method::USAGE_STATS, None).await
    }

    // =========================================================================
    // Permissions
    // =========================================================================

    /// Full permission overview with rule breakdowns.
    pub async fn permission_overview(&self) -> Result<PermissionOverview, ClientError> {
        self.request_typed(method::PERMISSION_OVERVIEW, None).await
    }

    pub async fn permission_overview_typed(&self) -> Result<PermissionOverview, ClientError> {
        self.request_typed(method::PERMISSION_OVERVIEW, None).await
    }

    /// Current permission mode.
    pub async fn permission_mode(&self) -> Result<PermissionModeResult, ClientError> {
        self.request_typed(method::PERMISSION_MODE, None).await
    }

    pub async fn permission_mode_name(&self) -> Result<String, ClientError> {
        let value: PermissionModeValue = self.request_typed(method::PERMISSION_MODE, None).await?;
        Ok(value.mode)
    }

    /// Set the runtime permission mode.
    pub async fn set_permission_mode(&self, mode: &str) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::PERMISSION_SET_MODE,
            Some(serde_json::to_value(PermissionSetModeParams {
                mode: mode.to_string(),
            })?),
        )
        .await
    }

    /// Add a permission rule (allow or deny).
    pub async fn add_permission_rule(
        &self,
        kind: &str,
        rule: &str,
    ) -> Result<PermissionRuleUpdateResult, ClientError> {
        self.request_typed(
            method::PERMISSION_ADD_RULE,
            Some(serde_json::to_value(PermissionRuleParams {
                kind: kind.to_string(),
                rule: rule.to_string(),
            })?),
        )
        .await
    }

    /// Remove a permission rule (allow or deny).
    pub async fn remove_permission_rule(
        &self,
        kind: &str,
        rule: &str,
    ) -> Result<PermissionRuleUpdateResult, ClientError> {
        self.request_typed(
            method::PERMISSION_REMOVE_RULE,
            Some(serde_json::to_value(PermissionRuleParams {
                kind: kind.to_string(),
                rule: rule.to_string(),
            })?),
        )
        .await
    }

    /// Add a session-scoped permission rule.
    pub async fn add_session_permission_rule(
        &self,
        session_id: &str,
        kind: &str,
        rule: &str,
    ) -> Result<PermissionRuleUpdateResult, ClientError> {
        self.request_typed(
            method::PERMISSION_ADD_SESSION_RULE,
            Some(serde_json::to_value(SessionPermissionRuleParams {
                session_id: session_id.to_string(),
                kind: kind.to_string(),
                rule: rule.to_string(),
            })?),
        )
        .await
    }

    /// Remove a session-scoped permission rule.
    pub async fn remove_session_permission_rule(
        &self,
        session_id: &str,
        kind: &str,
        rule: &str,
    ) -> Result<PermissionRuleUpdateResult, ClientError> {
        self.request_typed(
            method::PERMISSION_REMOVE_SESSION_RULE,
            Some(serde_json::to_value(SessionPermissionRuleParams {
                session_id: session_id.to_string(),
                kind: kind.to_string(),
                rule: rule.to_string(),
            })?),
        )
        .await
    }

    /// Validate a directory path before adding it as an additional directory.
    pub async fn validate_add_directory(
        &self,
        path: &str,
    ) -> Result<AddDirectoryCandidate, ClientError> {
        self.request_typed(
            method::PERMISSION_VALIDATE_DIRECTORY,
            Some(serde_json::to_value(ValidateDirectoryParams {
                path: path.to_string(),
            })?),
        )
        .await
    }

    /// Add an additional directory to the session's permitted directories.
    pub async fn add_directory(
        &self,
        session_id: &str,
        directory: &str,
    ) -> Result<AddedDirectory, ClientError> {
        self.request_typed(
            method::PERMISSION_ADD_DIRECTORY,
            Some(serde_json::to_value(AddDirectoryParams {
                session_id: session_id.to_string(),
                directory: directory.into(),
            })?),
        )
        .await
    }

    // =========================================================================
    // Settings
    // =========================================================================

    /// Current model name.
    pub async fn model_name(&self) -> Result<ModelNameResult, ClientError> {
        self.request_typed(method::SETTINGS_MODEL_NAME, None).await
    }

    /// Available model options for the model picker.
    pub async fn model_options(&self) -> Result<ModelOptionsResult, ClientError> {
        self.request_typed(method::SETTINGS_MODEL_OPTIONS, None)
            .await
    }

    /// List of supported providers and their model resolutions.
    pub async fn supported_providers(&self) -> Result<ProvidersResult, ClientError> {
        self.request_typed(method::SETTINGS_PROVIDERS, None).await
    }

    /// Set or clear the model override. Returns the new display name.
    pub async fn set_model_override(
        &self,
        model: Option<String>,
    ) -> Result<SetModelResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_MODEL,
            Some(serde_json::to_value(SetModelParams {
                session_id: None,
                model,
            })?),
        )
        .await
    }

    /// Set or clear the validated model override for a loaded session.
    pub async fn set_session_model_override(
        &self,
        session_id: &str,
        model: Option<String>,
    ) -> Result<ModelChangeResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_MODEL,
            Some(serde_json::to_value(SetModelParams {
                session_id: Some(session_id.to_string()),
                model,
            })?),
        )
        .await
    }

    /// Set or clear the numeric thinking budget used by subsequent requests.
    pub async fn set_max_thinking_tokens(
        &self,
        session_id: &str,
        max_thinking_tokens: Option<u32>,
    ) -> Result<ThinkingBudgetResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_THINKING_BUDGET,
            Some(json!({
                "session_id": session_id,
                "max_thinking_tokens": max_thinking_tokens,
            })),
        )
        .await
    }

    /// Current theme setting.
    pub async fn theme_setting(&self) -> Result<ThemeResult, ClientError> {
        self.request_typed(method::SETTINGS_THEME, None).await
    }

    /// Update the theme setting.
    pub async fn set_theme_setting(&self, theme: &str) -> Result<ThemeResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_THEME,
            Some(serde_json::to_value(ThemeParams {
                theme: theme.to_string(),
            })?),
        )
        .await
    }

    /// Current effort level override, if any.
    pub async fn effort_level(&self) -> Result<EffortResult, ClientError> {
        self.request_typed(method::SETTINGS_EFFORT, None).await
    }

    /// Set or clear the effort level override for a session.
    pub async fn set_effort_override(
        &self,
        session_id: &str,
        effort: Option<orbcode_protocol::EffortLevel>,
    ) -> Result<EffortResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_EFFORT,
            Some(serde_json::to_value(EffortParams {
                session_id: session_id.to_string(),
                effort,
            })?),
        )
        .await
    }

    /// Current output style setting.
    pub async fn output_style_setting(&self) -> Result<OutputStyleResult, ClientError> {
        self.request_typed(method::SETTINGS_OUTPUT_STYLE, None)
            .await
    }

    /// Update the output style setting.
    pub async fn set_output_style_setting(
        &self,
        style: &str,
    ) -> Result<OutputStyleResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_OUTPUT_STYLE,
            Some(serde_json::to_value(OutputStyleParams {
                style: style.to_string(),
            })?),
        )
        .await
    }

    /// Current sandbox local settings.
    pub async fn sandbox_local_settings(&self) -> Result<SandboxLocalSettings, ClientError> {
        self.request_typed(method::SETTINGS_SANDBOX, None).await
    }

    /// Update sandbox settings.
    pub async fn update_sandbox_settings(
        &self,
        update: SandboxSettingsUpdate,
    ) -> Result<PathResult, ClientError> {
        self.request_typed(
            method::SETTINGS_UPDATE_SANDBOX,
            Some(serde_json::to_value(update)?),
        )
        .await
    }

    /// Ensure the keybindings file exists and return its path.
    pub async fn keybindings(&self) -> Result<KeybindingsFileResult, ClientError> {
        self.request_typed(method::SETTINGS_KEYBINDINGS, None).await
    }

    /// Load merged keybindings and return any warnings.
    pub async fn load_keybindings(&self) -> Result<KeybindingsLoadResult, ClientError> {
        self.request_typed(method::SETTINGS_LOAD_KEYBINDINGS, None)
            .await
    }

    // =========================================================================
    // Tools
    // =========================================================================

    /// List all registered tool specs.
    pub async fn list_tools(&self) -> Result<ToolsListResult, ClientError> {
        self.request_typed(method::TOOLS_LIST, None).await
    }

    pub async fn list_tool_names(&self) -> Result<Vec<String>, ClientError> {
        let values: Vec<ToolNameValue> = self.request_typed(method::TOOLS_LIST, None).await?;
        Ok(values.into_iter().map(|value| value.name).collect())
    }

    /// Seed one file into the same stale-write state used by Read/Edit/Write.
    pub async fn seed_read_state(
        &self,
        session_id: &str,
        path: &str,
        mtime: u64,
    ) -> Result<SeedReadStateResult, ClientError> {
        self.request_typed(
            method::TOOLS_SEED_READ_STATE,
            Some(json!({ "session_id": session_id, "path": path, "mtime": mtime })),
        )
        .await
    }

    /// Load skill definitions from the workspace.
    pub async fn skill_definitions(&self) -> Result<SkillDefinitionsResult, ClientError> {
        self.request_typed(
            method::TOOLS_SKILLS,
            Some(serde_json::to_value(SkillDefinitionsParams::default())?),
        )
        .await
    }

    /// Load skill definitions visible to a session.
    pub async fn skill_definitions_for_session(
        &self,
        session_id: &str,
    ) -> Result<SkillDefinitionsResult, ClientError> {
        self.request_typed(
            method::TOOLS_SKILLS,
            Some(serde_json::to_value(SkillDefinitionsParams {
                session_id: Some(session_id.to_string()),
            })?),
        )
        .await
    }

    /// List loaded agent definitions.
    pub async fn agent_definitions(&self) -> Result<AgentDefinitionsResult, ClientError> {
        self.request_typed(method::TOOLS_AGENTS, None).await
    }

    /// Invoke a tool by name with a JSON input string.
    pub async fn invoke_tool(
        &self,
        name: &str,
        input: &str,
    ) -> Result<ToolInvokeResult, ClientError> {
        self.request_typed(
            method::TOOLS_INVOKE,
            Some(serde_json::to_value(ToolInvokeParams {
                name: name.to_string(),
                input: input.to_string(),
            })?),
        )
        .await
    }

    /// Get plan overview (plan file, state file, mode).
    pub async fn plan_overview(&self) -> Result<PlanOverview, ClientError> {
        self.request_typed(method::TOOLS_PLAN, None).await
    }

    /// Load a task list snapshot by its ID.
    pub async fn load_task_list_snapshot(
        &self,
        task_list_id: &str,
    ) -> Result<TaskListResult, ClientError> {
        self.request_typed(
            method::TOOLS_TASK_LIST,
            Some(serde_json::to_value(TaskListParams {
                task_list_id: task_list_id.to_string(),
            })?),
        )
        .await
    }

    // =========================================================================
    // MCP
    // =========================================================================

    /// List all configured MCP servers.
    pub async fn list_mcp_servers(&self) -> Result<McpListServersResult, ClientError> {
        self.request_typed("mcp/list_servers", None).await
    }

    /// Return the secret-free MCP status projection used by SDK controls.
    pub async fn mcp_status(&self) -> Result<McpStatusResult, ClientError> {
        self.request_typed(method::MCP_STATUS, None).await
    }

    /// Get the trust level for a specific MCP server.
    pub async fn mcp_server_trust(
        &self,
        server_id: &str,
    ) -> Result<McpServerTrustResult, ClientError> {
        self.request_typed(
            method::MCP_SERVER_TRUST,
            Some(serde_json::to_value(McpServerIdParams {
                server_id: server_id.to_string(),
            })?),
        )
        .await
    }

    /// Set the trust level for an MCP server.
    pub async fn set_mcp_server_trust(
        &self,
        server_id: &str,
        trust: McpServerTrust,
    ) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::MCP_SET_TRUST,
            Some(serde_json::to_value(McpSetTrustParams {
                server_id: server_id.to_string(),
                trust,
            })?),
        )
        .await
    }

    /// List tools provided by a specific MCP server.
    pub async fn list_mcp_tools(&self, server_id: &str) -> Result<McpListToolsResult, ClientError> {
        self.request_typed(
            method::MCP_LIST_TOOLS,
            Some(serde_json::to_value(McpServerIdParams {
                server_id: server_id.to_string(),
            })?),
        )
        .await
    }

    /// List resources provided by a specific MCP server.
    pub async fn list_mcp_resources(
        &self,
        server_id: &str,
    ) -> Result<McpListResourcesResult, ClientError> {
        self.request_typed(
            method::MCP_LIST_RESOURCES,
            Some(serde_json::to_value(McpServerIdParams {
                server_id: server_id.to_string(),
            })?),
        )
        .await
    }

    /// Read a resource from an MCP server by URI.
    pub async fn read_mcp_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<McpResourceContent, ClientError> {
        self.request_typed(
            method::MCP_READ_RESOURCE,
            Some(serde_json::to_value(McpReadResourceParams {
                server_id: server_id.to_string(),
                uri: uri.to_string(),
                session_id: None,
            })?),
        )
        .await
    }

    /// List prompts provided by a specific MCP server.
    pub async fn list_mcp_prompts(
        &self,
        server_id: &str,
    ) -> Result<McpListPromptsResult, ClientError> {
        self.request_typed(
            method::MCP_LIST_PROMPTS,
            Some(serde_json::to_value(McpServerIdParams {
                server_id: server_id.to_string(),
            })?),
        )
        .await
    }

    /// Get a prompt from an MCP server by name with optional arguments.
    pub async fn get_mcp_prompt(
        &self,
        server_id: &str,
        name: &str,
        arguments: Value,
    ) -> Result<McpPromptResult, ClientError> {
        self.request_typed(
            method::MCP_GET_PROMPT,
            Some(serde_json::to_value(McpGetPromptParams {
                server_id: server_id.to_string(),
                name: name.to_string(),
                arguments,
                session_id: None,
            })?),
        )
        .await
    }

    /// Invoke a tool on an MCP server.
    pub async fn invoke_mcp_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &str,
    ) -> Result<McpToolResult, ClientError> {
        self.request_typed(
            method::MCP_INVOKE_TOOL,
            Some(serde_json::to_value(McpInvokeToolParams {
                server_id: server_id.to_string(),
                tool_name: tool_name.to_string(),
                input: input.to_string(),
            })?),
        )
        .await
    }

    /// Run diagnostics on an MCP server.
    pub async fn diagnose_mcp_server(
        &self,
        server_id: &str,
    ) -> Result<McpDiagnoseResult, ClientError> {
        self.request_typed(
            method::MCP_DIAGNOSE,
            Some(serde_json::to_value(McpServerIdParams {
                server_id: server_id.to_string(),
            })?),
        )
        .await
    }

    /// Upsert (add or update) an MCP server configuration.
    pub async fn upsert_mcp_server(&self, config: McpServerInput) -> Result<NoData, ClientError> {
        self.request_no_data(
            method::MCP_UPSERT_SERVER,
            Some(serde_json::to_value(config)?),
        )
        .await
    }

    /// Remove an MCP server by ID.
    pub async fn remove_mcp_server(
        &self,
        server_id: &str,
    ) -> Result<McpRemoveServerResult, ClientError> {
        self.request_typed(
            method::MCP_REMOVE_SERVER,
            Some(serde_json::to_value(McpServerIdParams {
                server_id: server_id.to_string(),
            })?),
        )
        .await
    }

    /// Get MCP capabilities overview.
    pub async fn mcp_capabilities(&self) -> Result<McpCapabilitiesResult, ClientError> {
        self.request_typed(method::MCP_CAPABILITIES, None).await
    }

    /// Get MCP slash command suggestions (servers, tools, resources).
    pub async fn mcp_slash_suggestions(&self) -> Result<McpSlashSuggestionCatalog, ClientError> {
        self.request_typed(method::MCP_SLASH_SUGGESTIONS, None)
            .await
    }

    // =========================================================================
    // Background Jobs
    // =========================================================================

    /// List all background jobs.
    pub async fn list_background_jobs(&self) -> Result<BackgroundTaskListResult, ClientError> {
        self.request_typed(method::BACKGROUND_LIST, None).await
    }

    pub async fn list_workflows(&self) -> Result<WorkflowListResult, ClientError> {
        self.request_typed(method::WORKFLOW_LIST, None).await
    }

    pub async fn start_workflow(
        &self,
        session_id: &str,
        name: &str,
        arguments: &str,
    ) -> Result<WorkflowTaskResult, ClientError> {
        self.request_typed(
            method::WORKFLOW_START,
            Some(serde_json::to_value(WorkflowStartParams {
                session_id: session_id.to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
            })?),
        )
        .await
    }

    pub async fn start_dynamic_workflow(
        &self,
        session_id: &str,
        name: &str,
        spec: Value,
        arguments: &str,
    ) -> Result<WorkflowTaskResult, ClientError> {
        self.request_typed(
            method::WORKFLOW_START_DYNAMIC,
            Some(serde_json::to_value(WorkflowStartDynamicParams {
                session_id: session_id.to_string(),
                name: name.to_string(),
                spec,
                arguments: arguments.to_string(),
            })?),
        )
        .await
    }

    pub async fn resume_workflow(&self, run_id: &str) -> Result<WorkflowTaskResult, ClientError> {
        self.request_typed(
            method::WORKFLOW_RESUME,
            Some(serde_json::to_value(WorkflowResumeParams {
                run_id: run_id.to_string(),
            })?),
        )
        .await
    }

    /// Get detailed view for a single background job.
    pub async fn background_job_detail(
        &self,
        job_id: &str,
    ) -> Result<orbcode_protocol::BackgroundTaskView, ClientError> {
        self.request_typed(
            method::BACKGROUND_DETAIL,
            Some(serde_json::to_value(BackgroundJobParams {
                job_id: job_id.to_string(),
            })?),
        )
        .await
    }

    /// Create a new background job.
    pub async fn create_background_job(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<BackgroundCreateResult, ClientError> {
        self.request_typed(
            method::BACKGROUND_CREATE,
            Some(serde_json::to_value(BackgroundCreateParams {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
            })?),
        )
        .await
    }

    /// Cancel a running background job.
    pub async fn cancel_background_job(
        &self,
        job_id: &str,
    ) -> Result<BackgroundCancelResult, ClientError> {
        self.request_typed(
            method::BACKGROUND_CANCEL,
            Some(serde_json::to_value(BackgroundJobParams {
                job_id: job_id.to_string(),
            })?),
        )
        .await
    }

    /// Read the log output of a background job.
    pub async fn read_background_log(
        &self,
        job_id: &str,
    ) -> Result<BackgroundLogResult, ClientError> {
        self.request_typed(
            method::BACKGROUND_LOG,
            Some(serde_json::to_value(BackgroundJobParams {
                job_id: job_id.to_string(),
            })?),
        )
        .await
    }

    pub async fn subscribe_background_task_stream(
        &self,
        task_id: &str,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<orbcode_protocol::StreamEvent>, ClientError>
    {
        let result: BackgroundSubscribeResult = self
            .request_typed(
                method::BACKGROUND_SUBSCRIBE,
                Some(serde_json::to_value(BackgroundSubscribeParams {
                    task_id: task_id.to_string(),
                })?),
            )
            .await?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        register_stream_route(
            &self.stream_routes,
            &self.pending_stream_events,
            result.subscription_id,
            tx,
            true,
        )
        .await;
        Ok(rx)
    }

    /// Read the stream-json event log of a background job.
    pub async fn read_background_events(
        &self,
        job_id: &str,
    ) -> Result<BackgroundEventsResult, ClientError> {
        self.request_typed(
            method::BACKGROUND_EVENTS,
            Some(serde_json::to_value(BackgroundJobParams {
                job_id: job_id.to_string(),
            })?),
        )
        .await
    }

    // =========================================================================
    // Diagnostics
    // =========================================================================

    /// Full status overview for a session.
    pub async fn status_overview(&self, session_id: &str) -> Result<StatusOverview, ClientError> {
        self.request_typed(
            method::DIAGNOSTICS_STATUS,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    /// Full typed status overview for a session.
    pub async fn status_overview_typed(
        &self,
        session_id: &str,
    ) -> Result<StatusOverview, ClientError> {
        self.request_typed(
            method::DIAGNOSTICS_STATUS,
            Some(json!({ "session_id": session_id })),
        )
        .await
    }

    /// Memory file overview (user + project memories).
    pub async fn memory_overview(&self) -> Result<MemoryOverview, ClientError> {
        self.request_typed(method::DIAGNOSTICS_MEMORY, None).await
    }

    /// Run the doctor diagnostic suite.
    pub async fn doctor_report(&self) -> Result<DoctorReport, ClientError> {
        self.request_typed(method::DIAGNOSTICS_DOCTOR, None).await
    }

    /// Remove scoped orphan child-session artifacts, or preview removal when `dry_run` is true.
    pub async fn cleanup_orphan_child_sessions(
        &self,
        dry_run: bool,
        stale_running_cutoff_ms: Option<i64>,
    ) -> Result<ChildSessionOrphanCleanupResult, ClientError> {
        self.request_typed(
            method::DIAGNOSTICS_CLEANUP_CHILD_SESSIONS,
            Some(serde_json::to_value(
                DiagnosticsCleanupChildSessionsParams {
                    dry_run,
                    stale_running_cutoff_ms,
                },
            )?),
        )
        .await
    }

    /// Discover configured hooks.
    pub async fn hook_discovery(&self) -> Result<HookDiscovery, ClientError> {
        self.request_typed(method::DIAGNOSTICS_HOOKS, None).await
    }

    /// Get the workspace git diff (staged, unstaged, untracked).
    pub async fn workspace_diff(&self) -> Result<WorkspaceDiff, ClientError> {
        self.request_typed(method::DIAGNOSTICS_DIFF, None).await
    }

    /// List advanced capabilities and their status.
    pub async fn advanced_capabilities(&self) -> Result<AdvancedCapabilitiesResult, ClientError> {
        self.request_typed(method::DIAGNOSTICS_ADVANCED, None).await
    }

    // =========================================================================
    // Auth
    // =========================================================================

    /// Authentication overview for all providers.
    pub async fn auth_overview(&self) -> Result<AuthOverview, ClientError> {
        self.request_typed(method::AUTH_OVERVIEW, None).await
    }

    /// Log in to a provider with the given auth method.
    pub async fn auth_login(
        &self,
        provider: orbcode_protocol::ProviderId,
        auth_method: AuthMethod,
        token: Option<&str>,
        env_var: Option<&str>,
    ) -> Result<AuthLoginResult, ClientError> {
        self.request_typed(
            method::AUTH_LOGIN,
            Some(serde_json::to_value(AuthLoginParams {
                provider,
                method: auth_method,
                token: token.map(str::to_string),
                env_var: env_var.map(str::to_string),
            })?),
        )
        .await
    }

    /// Log out of a provider (or all providers if none specified).
    pub async fn auth_logout(
        &self,
        provider: Option<orbcode_protocol::ProviderId>,
    ) -> Result<AuthLogoutResult, ClientError> {
        self.request_typed(
            method::AUTH_LOGOUT,
            Some(serde_json::to_value(AuthLogoutParams { provider })?),
        )
        .await
    }

    // =========================================================================
    // Transport receivers
    // =========================================================================

    /// Take the notification receiver for stream events. Can only be called
    /// once; subsequent calls return `None`.
    pub async fn take_notification_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerNotificationEnvelope>> {
        self.notification_rx.lock().await.take()
    }

    /// Take the server-request receiver for permission prompts and MCP trust
    /// requests. Can only be called once; subsequent calls return `None`.
    pub async fn take_server_request_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerRequestEnvelope>> {
        self.server_request_rx.lock().await.take()
    }

    // =========================================================================
    // Extended settings (TUI migration)
    // =========================================================================

    /// Get whether all permissions are bypassed.
    pub async fn allow_all(&self) -> Result<bool, ClientError> {
        let result: AllowAllResult = self.request_typed(method::SETTINGS_ALLOW_ALL, None).await?;
        Ok(result.allow_all)
    }

    /// Set the allow-all permission bypass flag.
    pub async fn set_allow_all(&self, enabled: bool) -> Result<AllowAllResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_ALLOW_ALL,
            Some(serde_json::to_value(AllowAllParams { allow_all: enabled })?),
        )
        .await
    }

    /// Get the current editor mode setting.
    pub async fn editor_mode_setting(&self) -> Result<EditorModeResult, ClientError> {
        self.request_typed(method::SETTINGS_EDITOR_MODE, None).await
    }

    /// Set the editor mode.
    pub async fn set_editor_mode_setting(
        &self,
        mode: &str,
    ) -> Result<EditorModeResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_EDITOR_MODE,
            Some(serde_json::to_value(EditorModeParams {
                mode: mode.to_string(),
            })?),
        )
        .await
    }

    /// Get available output style options.
    pub async fn output_style_options(&self) -> Result<OutputStyleOptionsResult, ClientError> {
        self.request_typed(method::SETTINGS_OUTPUT_STYLE_OPTIONS, None)
            .await
    }

    /// Get the active output style name and whether it matched.
    pub async fn active_output_style(&self) -> Result<ActiveOutputStyleResult, ClientError> {
        self.request_typed(method::SETTINGS_ACTIVE_OUTPUT_STYLE, None)
            .await
    }

    /// Check whether a setting key is locked by managed policy.
    pub async fn is_setting_locked(&self, key: &str) -> Result<bool, ClientError> {
        let result: SettingLockedResult = self
            .request_typed(
                method::SETTINGS_IS_LOCKED,
                Some(serde_json::to_value(SettingKeyParams {
                    key: key.to_string(),
                })?),
            )
            .await?;
        Ok(result.locked)
    }

    /// Enable or disable auto-memory.
    pub async fn set_auto_memory_enabled(
        &self,
        enabled: bool,
    ) -> Result<EnabledResult, ClientError> {
        self.request_typed(
            method::SETTINGS_SET_AUTO_MEMORY,
            Some(serde_json::to_value(EnabledParams { enabled })?),
        )
        .await
    }

    /// Ensure a memory file exists at the given path.
    pub async fn ensure_memory_file(&self, path: &str) -> Result<PathResult, ClientError> {
        self.request_typed(
            method::SETTINGS_ENSURE_MEMORY_FILE,
            Some(serde_json::to_value(StringPathParams {
                path: path.to_string(),
            })?),
        )
        .await
    }

    /// Add a sandbox excluded command pattern.
    pub async fn add_sandbox_excluded_command(
        &self,
        command: &str,
    ) -> Result<SandboxExcludedCommandResult, ClientError> {
        self.request_typed(
            method::SETTINGS_ADD_SANDBOX_EXCLUDED,
            Some(serde_json::to_value(SandboxExcludedCommandParams {
                command: command.to_string(),
            })?),
        )
        .await
    }

    // =========================================================================
    // Extended sessions
    // =========================================================================

    /// Find a session by its exact custom title.
    pub async fn session_id_for_exact_custom_title(
        &self,
        title: &str,
    ) -> Result<Option<String>, ClientError> {
        let result: SessionFindByTitleResult = self
            .request_typed(
                method::SESSION_FIND_BY_TITLE,
                Some(serde_json::to_value(SessionFindByTitleParams {
                    title: title.to_string(),
                })?),
            )
            .await?;
        Ok(result.session_id)
    }

    // =========================================================================
    // Extended MCP
    // =========================================================================

    /// Get MCP OAuth overview.
    pub async fn mcp_oauth_overview(
        &self,
        server_id: Option<&str>,
    ) -> Result<McpOAuthOverview, ClientError> {
        self.request_typed(
            method::MCP_OAUTH_OVERVIEW,
            Some(serde_json::to_value(McpOAuthOverviewParams {
                server_id: server_id.map(str::to_string),
            })?),
        )
        .await
    }

    /// Log out an MCP OAuth token.
    pub async fn logout_mcp_oauth_token(
        &self,
        server_id: &str,
    ) -> Result<McpLogoutOAuthTokenResult, ClientError> {
        self.request_typed(
            method::MCP_LOGOUT_OAUTH_TOKEN,
            Some(serde_json::to_value(McpServerIdParams {
                server_id: server_id.to_string(),
            })?),
        )
        .await
    }

    // =========================================================================
    // Extended diagnostics
    // =========================================================================

    /// Get the last provider request debug snapshot.
    pub async fn last_provider_request_snapshot(
        &self,
    ) -> Result<LastProviderRequestResult, ClientError> {
        self.request_typed(method::DIAGNOSTICS_LAST_REQUEST, None)
            .await
    }

    pub async fn last_provider_request_snapshot_typed(
        &self,
    ) -> Result<ProviderRequestDebugSnapshot, ClientError> {
        self.request_typed(method::DIAGNOSTICS_LAST_REQUEST, None)
            .await
    }

    /// Get pre-user instructions preview for a session.
    pub async fn pre_user_instructions_preview(
        &self,
        session_id: &str,
    ) -> Result<PreUserInstructionsResult, ClientError> {
        self.request_typed(
            method::DIAGNOSTICS_PRE_USER_INSTRUCTIONS,
            Some(serde_json::to_value(SessionIdParams {
                session_id: session_id.to_string(),
            })?),
        )
        .await
    }

    // =========================================================================
    // Extended tools
    // =========================================================================

    /// Enter plan mode.
    pub async fn enter_plan_mode(&self) -> Result<EnterPlanModeResult, ClientError> {
        self.request_typed(method::TOOLS_ENTER_PLAN, None).await
    }

    // =========================================================================
    // Extended background
    // =========================================================================

    /// List background jobs in summary form.
    pub async fn list_background_jobs_summary(
        &self,
    ) -> Result<BackgroundTaskListResult, ClientError> {
        self.request_typed(method::BACKGROUND_LIST_SUMMARY, None)
            .await
    }

    /// Cancel one async message with explicit signalled/terminal/missing outcome.
    pub async fn cancel_async_task(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<CancelAsyncTaskResult, ClientError> {
        self.request_typed(
            method::BACKGROUND_CANCEL_ASYNC,
            Some(json!({ "session_id": session_id, "task_id": task_id })),
        )
        .await
    }

    // =========================================================================
    // Additional protocol methods (Phase 4 migration)
    // =========================================================================

    /// Get agent definitions with load warnings.
    pub async fn agent_definitions_with_warnings(
        &self,
    ) -> Result<AgentDefinitionsWithWarningsResult, ClientError> {
        self.request_typed(method::TOOLS_AGENTS_WITH_WARNINGS, None)
            .await
    }

    /// Ensure the keybindings file exists; returns path and whether it was created.
    pub async fn ensure_keybindings_file(&self) -> Result<KeybindingsFileResult, ClientError> {
        self.keybindings().await
    }

    /// Get the active output style name.
    pub async fn active_output_style_name(&self) -> Result<String, ClientError> {
        Ok(self.active_output_style().await?.name)
    }

    /// Get whether the active output style matched a known style.
    pub async fn active_output_style_matched(&self) -> Result<bool, ClientError> {
        Ok(self.active_output_style().await?.matched)
    }

    /// Respond to a permission request using the core PermissionDecision type.
    /// Returns `true` if the response was delivered to the pending request.
    pub async fn respond_to_pending_permission_request(
        &self,
        request_id: &str,
        decision: PermissionDecision,
    ) -> bool {
        let Some(protocol_request_id) = self
            .pending_server_requests
            .lock()
            .await
            .permissions
            .remove(request_id)
        else {
            return false;
        };
        let params = PermissionResponseParams {
            request_id: request_id.to_string(),
            decision: permission_decision_to_wire(decision),
        };
        let Ok(data) = serde_json::to_value(params) else {
            return false;
        };
        self.respond_to_server_request(
            protocol_request_id,
            ResponseResult::Success { data: Some(data) },
        )
        .await
        .is_ok()
    }

    /// Snapshot the currently unanswered permission request IDs.
    pub async fn pending_permission_request_ids(&self) -> Vec<String> {
        self.pending_server_requests
            .lock()
            .await
            .permissions
            .keys()
            .cloned()
            .collect()
    }

    /// Respond to an MCP trust server-request.
    pub async fn respond_to_mcp_trust_request(
        &self,
        server_id: &str,
        decision: McpTrustDecisionWire,
    ) -> bool {
        let Some(protocol_request_id) = self
            .pending_server_requests
            .lock()
            .await
            .mcp_trust
            .remove(server_id)
        else {
            return false;
        };
        let Ok(data) = serde_json::to_value(decision) else {
            return false;
        };
        self.respond_to_server_request(
            protocol_request_id,
            ResponseResult::Success { data: Some(data) },
        )
        .await
        .is_ok()
    }

    /// Respond to an ask-user-question server-request.
    pub async fn respond_to_ask_user_question(
        &self,
        request_id: &str,
        answer: Option<String>,
    ) -> bool {
        let Some(protocol_request_id) = self
            .pending_server_requests
            .lock()
            .await
            .ask_user
            .remove(request_id)
        else {
            return false;
        };
        let Ok(data) = serde_json::to_value(AskUserQuestionResponse {
            request_id: request_id.to_string(),
            answer,
        }) else {
            return false;
        };
        self.respond_to_server_request(
            protocol_request_id,
            ResponseResult::Success { data: Some(data) },
        )
        .await
        .is_ok()
    }

    /// Submit a turn and return a typed `StreamEvent` receiver.
    pub async fn submit_turn_stream(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<orbcode_protocol::StreamEvent>, ClientError>
    {
        let sub = self.submit_turn(session_id, &prompt.into()).await?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        register_stream_route(
            &self.stream_routes,
            &self.pending_stream_events,
            sub.subscription_id,
            tx,
            false,
        )
        .await;
        Ok(rx)
    }
}

/// Drain any events buffered in `pending_stream_events` for `subscription_id`
/// into `tx`, then (unless a terminal event was already drained) install a live
/// route so subsequent events go straight to `tx`.
///
/// `stream_routes` is held across the whole drain+insert. The producer
/// ([`route_stream_event`]) takes `stream_routes` first and only then
/// `pending_stream_events`, holding both across its check-and-enqueue; using the
/// same routes→pending order here serializes the two so an event cannot slip
/// into `pending` between the drain and the insert (which would strand it — and
/// hang the consumer if terminal). `close_on_terminal_background_task` selects
/// which notion of "terminal" applies to the drained events (turn-terminal for a
/// turn subscription, background-task-terminal for a background subscription).
async fn register_stream_route(
    stream_routes: &Arc<Mutex<HashMap<String, StreamRoute>>>,
    pending_stream_events: &Arc<Mutex<HashMap<String, VecDeque<orbcode_protocol::StreamEvent>>>>,
    subscription_id: String,
    tx: mpsc::UnboundedSender<orbcode_protocol::StreamEvent>,
    close_on_terminal_background_task: bool,
) {
    let mut routes = stream_routes.lock().await;
    let mut terminal_seen = false;
    if let Some(mut pending) = pending_stream_events.lock().await.remove(&subscription_id) {
        while let Some(event) = pending.pop_front() {
            terminal_seen |= if close_on_terminal_background_task {
                background_task_update_is_terminal(&event)
            } else {
                event.is_terminal()
            };
            if tx.send(event).is_err() {
                return;
            }
        }
    }

    if !terminal_seen {
        routes.insert(
            subscription_id,
            StreamRoute {
                tx,
                close_on_terminal_background_task,
            },
        );
    }
}

async fn route_notifications(
    mut rx: mpsc::Receiver<ServerNotificationEnvelope>,
    external_tx: mpsc::Sender<ServerNotificationEnvelope>,
    stream_routes: Arc<Mutex<HashMap<String, StreamRoute>>>,
    pending_stream_events: Arc<Mutex<HashMap<String, VecDeque<orbcode_protocol::StreamEvent>>>>,
) {
    while let Some(envelope) = rx.recv().await {
        if envelope.method == method::NOTIFICATION_STREAM_EVENT
            && let Ok(notification) =
                serde_json::from_value::<StreamEventNotification>(envelope.params.clone())
        {
            route_stream_event(notification, &stream_routes, &pending_stream_events).await;
        }
        let _ = external_tx.try_send(envelope);
    }
}

async fn route_stream_event(
    notification: StreamEventNotification,
    stream_routes: &Arc<Mutex<HashMap<String, StreamRoute>>>,
    pending_stream_events: &Arc<Mutex<HashMap<String, VecDeque<orbcode_protocol::StreamEvent>>>>,
) {
    let subscription_id = notification.subscription_id;
    let event = notification.event;

    // Hold `stream_routes` across the pending enqueue below. The registrar
    // (`submit_turn_stream` / `subscribe_*`) locks `stream_routes` first and only
    // then `pending_stream_events`, draining pending and inserting the route
    // while holding `stream_routes`. If we dropped `stream_routes` after the
    // "no route" check and re-acquired only `pending_stream_events` to enqueue,
    // a registrar could slip in between — drain the (still empty) pending queue,
    // insert the route — and our subsequent enqueue would strand the event in
    // `pending_stream_events` forever (hanging the consumer if it was terminal).
    // Keeping `stream_routes` held, in the same routes→pending order, makes the
    // check-and-enqueue atomic against the registrar without risking deadlock.
    let mut routes = stream_routes.lock().await;
    if let Some(route) = routes.get(&subscription_id) {
        let is_terminal = event.is_terminal()
            || (route.close_on_terminal_background_task
                && background_task_update_is_terminal(&event));
        let send_failed = route.tx.send(event).is_err();
        if send_failed || is_terminal {
            routes.remove(&subscription_id);
        }
        return;
    }

    pending_stream_events
        .lock()
        .await
        .entry(subscription_id)
        .or_default()
        .push_back(event);
    drop(routes);
}

fn background_task_update_is_terminal(event: &orbcode_protocol::StreamEvent) -> bool {
    match event {
        orbcode_protocol::StreamEvent::BackgroundTaskUpdated { task, .. } => {
            !task.status.is_active()
        }
        _ => false,
    }
}

async fn route_server_requests(
    mut rx: mpsc::Receiver<ServerRequestEnvelope>,
    external_tx: mpsc::Sender<ServerRequestEnvelope>,
    pending_server_requests: Arc<Mutex<PendingServerRequests>>,
) {
    while let Some(envelope) = rx.recv().await {
        index_server_request(&envelope, &pending_server_requests).await;
        let _ = external_tx.try_send(envelope);
    }
}

async fn index_server_request(
    envelope: &ServerRequestEnvelope,
    pending_server_requests: &Arc<Mutex<PendingServerRequests>>,
) {
    match envelope.method.as_str() {
        method::SERVER_REQUEST_PERMISSION => {
            if let Ok(request) = serde_json::from_value::<orbcode_protocol::PermissionRequest>(
                envelope.params.clone(),
            ) {
                pending_server_requests
                    .lock()
                    .await
                    .permissions
                    .insert(request.request_id, envelope.id.clone());
            }
        }
        method::SERVER_REQUEST_MCP_TRUST => {
            if let Ok(request) = serde_json::from_value::<orbcode_protocol::McpTrustApprovalRequest>(
                envelope.params.clone(),
            ) {
                pending_server_requests
                    .lock()
                    .await
                    .mcp_trust
                    .insert(request.server_id, envelope.id.clone());
            }
        }
        method::SERVER_REQUEST_ASK_USER => {
            if let Ok(request) =
                serde_json::from_value::<AskUserQuestionRequest>(envelope.params.clone())
            {
                pending_server_requests
                    .lock()
                    .await
                    .ask_user
                    .insert(request.request_id, envelope.id.clone());
            }
        }
        _ => {}
    }
}

fn permission_decision_to_wire(decision: PermissionDecision) -> PermissionDecisionWire {
    match decision {
        PermissionDecision::Approve => PermissionDecisionWire::Approve,
        PermissionDecision::Deny => PermissionDecisionWire::Deny,
        PermissionDecision::ApproveAlways(rule) => {
            PermissionDecisionWire::ApproveAlways { rules: vec![rule] }
        }
        PermissionDecision::ApproveAlwaysMany(rules) => {
            PermissionDecisionWire::ApproveAlways { rules }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use orbcode_app_server_protocol::ErrorCode;
    use orbcode_config::{AppConfigOverrides, sanitize_path};
    use orbcode_protocol::{BackgroundTaskViewStatus, StreamEvent};
    use orbcode_tools::{
        BackgroundTaskRecord, BackgroundTaskStatus, background_log_path, register_progress_stream,
        task_record_to_view, unregister_progress_stream, write_background_task_record,
    };

    use super::{AppClient, ClientError};

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-app-client-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    async fn write_project_transcript(
        home: &std::path::Path,
        cwd: &std::path::Path,
        session_id: &str,
        body: &str,
    ) {
        let project_dir = home
            .join("projects")
            .join(sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        tokio::fs::write(project_dir.join(format!("{session_id}.jsonl")), body)
            .await
            .expect("write transcript");
    }

    // -----------------------------------------------------------------------
    // 1. Bootstrap creates a session (protocol round trip)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn bootstrap_creates_session() {
        let home = test_path("bootstrap-home");
        let cwd = test_path("bootstrap-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let state = client.bootstrap(None).await.expect("bootstrap");
        assert!(
            !state.session.session_id.is_empty(),
            "bootstrap should return a session_id"
        );
        let sid = state.session.session_id.as_str();
        assert!(!sid.is_empty(), "session_id should not be empty");
        assert!(
            !state.model_display_name.is_empty(),
            "bootstrap should include model_display_name"
        );
    }

    #[tokio::test]
    async fn subscribe_background_task_stream_receives_initial_progress_and_terminal_update() {
        let home = test_path("bg-sub-home");
        let cwd = test_path("bg-sub-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let task_id = "workflow-subscribe-test";
        let log_path = background_log_path(&home, task_id);
        let mut record = BackgroundTaskRecord::new_workflow(
            task_id.to_string(),
            "session-1".to_string(),
            "generated workflow".to_string(),
            cwd.display().to_string(),
            log_path.display().to_string(),
        );
        write_background_task_record(&home, &record)
            .await
            .expect("write running record");
        let progress_tx = register_progress_stream(task_id, 16);

        let mut rx = client
            .subscribe_background_task_stream(task_id)
            .await
            .expect("subscribe");
        let initial = rx.recv().await.expect("initial task snapshot");
        match initial {
            StreamEvent::BackgroundTaskUpdated { task, .. } => {
                assert_eq!(task.task_id, task_id);
                assert_eq!(task.status, BackgroundTaskViewStatus::Running);
            }
            other => panic!("unexpected initial event: {other:?}"),
        }

        record.status = BackgroundTaskStatus::Completed;
        record.finished_at = Some(chrono::Utc::now().to_rfc3339());
        write_background_task_record(&home, &record)
            .await
            .expect("write completed record");
        progress_tx
            .send(StreamEvent::BackgroundTaskUpdated {
                session_id: "session-1".to_string(),
                task: task_record_to_view(&record),
            })
            .expect("send terminal task snapshot");

        let terminal = rx.recv().await.expect("terminal task snapshot");
        match terminal {
            StreamEvent::BackgroundTaskUpdated { task, .. } => {
                assert_eq!(task.task_id, task_id);
                assert_eq!(task.status, BackgroundTaskViewStatus::Completed);
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
        let closed = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("terminal update should close the client route");
        assert!(closed.is_none());

        unregister_progress_stream(task_id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_register_and_terminal_event_is_never_stranded() {
        use std::collections::VecDeque;
        use std::sync::Arc;

        use tokio::sync::{Mutex, mpsc};

        use super::{StreamRoute, register_stream_route, route_stream_event};
        use orbcode_app_server_protocol::StreamEventNotification;
        use orbcode_tools::{BackgroundTaskRecord, BackgroundTaskStatus, task_record_to_view};

        fn terminal_bg_event() -> StreamEvent {
            let mut record = BackgroundTaskRecord::new_workflow(
                "task-race".to_string(),
                "session-1".to_string(),
                "prompt".to_string(),
                "/tmp".to_string(),
                "/tmp/log".to_string(),
            );
            record.status = BackgroundTaskStatus::Completed;
            StreamEvent::BackgroundTaskUpdated {
                session_id: "session-1".to_string(),
                task: task_record_to_view(&record),
            }
        }

        // Race the producer (`route_stream_event`) against the registrar
        // (`register_stream_route`) many times. Before the fix, a terminal event
        // arriving in the window between the registrar's drain and its route
        // insert was stranded in `pending_stream_events` and never delivered to
        // the consumer's `rx` — hanging it forever. The consumer only ever reads
        // `rx`, so a correct implementation must ALWAYS deliver the event there,
        // regardless of interleaving.
        let subscription_id = "sub-race".to_string();
        for _ in 0..5000 {
            let stream_routes: Arc<Mutex<HashMap<String, StreamRoute>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let pending: Arc<Mutex<HashMap<String, VecDeque<StreamEvent>>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let (tx, mut rx) = mpsc::unbounded_channel();

            let producer = tokio::spawn({
                let stream_routes = Arc::clone(&stream_routes);
                let pending = Arc::clone(&pending);
                let subscription_id = subscription_id.clone();
                async move {
                    route_stream_event(
                        StreamEventNotification {
                            subscription_id,
                            event: terminal_bg_event(),
                        },
                        &stream_routes,
                        &pending,
                    )
                    .await;
                }
            });
            let registrar = tokio::spawn({
                let stream_routes = Arc::clone(&stream_routes);
                let pending = Arc::clone(&pending);
                let subscription_id = subscription_id.clone();
                async move {
                    register_stream_route(&stream_routes, &pending, subscription_id, tx, true)
                        .await;
                }
            });
            let _ = tokio::join!(producer, registrar);

            assert!(
                rx.try_recv().is_ok(),
                "terminal event was stranded (registration TOCTOU regression)"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 2. List sessions finds a persisted session
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_sessions_finds_persisted_session() {
        let home = test_path("list-sess-home");
        let cwd = test_path("list-sess-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        // Pre-create a persisted transcript so list_sessions can discover it.
        let project_dir = home
            .join("projects")
            .join(orbcode_config::sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        let session_id = "list-sess-test-session";
        let payload = serde_json::json!({
            "type": "user",
            "uuid": "uuid-1",
            "timestamp": "2026-06-01T00:00:00.000Z",
            "message": { "role": "user", "content": "hello" },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        });
        tokio::fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!("{}\n", serde_json::to_string(&payload).expect("serialize")),
        )
        .await
        .expect("write transcript");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let sessions = client.list_sessions().await.expect("list sessions");
        assert!(
            sessions.iter().any(|s| s.session_id == session_id),
            "persisted session should appear in list"
        );
    }

    #[tokio::test]
    async fn acp_load_replay_preflight_allows_safe_transcript() {
        let home = test_path("acp-preflight-safe-home");
        let cwd = test_path("acp-preflight-safe-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let session_id = "acp-preflight-safe-session";
        let payload = serde_json::json!({
            "type": "user",
            "uuid": "uuid-1",
            "timestamp": "2026-06-01T00:00:00.000Z",
            "message": { "role": "user", "content": "hello" },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        });
        write_project_transcript(
            &home,
            &cwd,
            session_id,
            &format!("{}\n", serde_json::to_string(&payload).expect("serialize")),
        )
        .await;

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let preflight = client
            .acp_load_replay_preflight(session_id)
            .await
            .expect("preflight");
        assert_eq!(preflight.session.session_id, session_id);
        assert!(preflight.replay_allowed);
        assert!(preflight.blockers.is_empty());
    }

    #[tokio::test]
    async fn acp_load_replay_preflight_reports_replay_blockers() {
        let home = test_path("acp-preflight-blocked-home");
        let cwd = test_path("acp-preflight-blocked-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let session_id = "acp-preflight-blocked-session";
        let user = serde_json::json!({
            "type": "user",
            "uuid": "uuid-1",
            "timestamp": "2026-06-01T00:00:00.000Z",
            "message": { "role": "user", "content": "hello" },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        });
        let api_error = serde_json::json!({
            "type": "system",
            "subtype": "api_error",
            "uuid": "uuid-2",
            "timestamp": "2026-06-01T00:00:01.000Z",
            "content": "provider failed",
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        });
        write_project_transcript(
            &home,
            &cwd,
            session_id,
            &format!(
                "{}\n{}\n",
                serde_json::to_string(&user).expect("serialize user"),
                serde_json::to_string(&api_error).expect("serialize error"),
            ),
        )
        .await;

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let preflight = client
            .acp_load_replay_preflight(session_id)
            .await
            .expect("preflight");
        assert!(!preflight.replay_allowed);
        assert!(
            preflight
                .blockers
                .iter()
                .any(|reason| reason.contains("system/API-error provenance")),
            "expected API-error blocker, got {:?}",
            preflight.blockers
        );
    }

    #[tokio::test]
    async fn acp_load_replay_preflight_reports_corrupt_and_relative_cwd_blockers() {
        let home = test_path("acp-preflight-invalid-home");
        let cwd = test_path("acp-preflight-invalid-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let session_id = "acp-preflight-invalid-session";
        let user = serde_json::json!({
            "type": "user",
            "uuid": "uuid-1",
            "timestamp": "2026-06-01T00:00:00.000Z",
            "message": { "role": "user", "content": "hello" },
            "cwd": "relative/project",
            "sessionId": session_id,
        });
        write_project_transcript(
            &home,
            &cwd,
            session_id,
            &format!(
                "{}\nnot-json\n",
                serde_json::to_string(&user).expect("serialize user")
            ),
        )
        .await;

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let preflight = client
            .acp_load_replay_preflight(session_id)
            .await
            .expect("preflight");
        assert!(!preflight.replay_allowed);
        assert!(
            preflight
                .blockers
                .iter()
                .any(|reason| reason.contains("corrupt transcript lines")),
            "expected corrupt-transcript blocker, got {:?}",
            preflight.blockers
        );
        assert!(
            preflight
                .blockers
                .iter()
                .any(|reason| reason.contains("relative cwd")),
            "expected relative-cwd blocker, got {:?}",
            preflight.blockers
        );
    }

    #[tokio::test]
    async fn acp_load_replay_preflight_errors_for_missing_session() {
        let home = test_path("acp-preflight-missing-home");
        let cwd = test_path("acp-preflight-missing-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let err = client
            .acp_load_replay_preflight("missing-session")
            .await
            .expect_err("missing session should error");
        match err {
            ClientError::Protocol(err) => assert_eq!(err.code, ErrorCode::SessionNotFound),
            other => panic!("expected protocol error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // 3. Model name returns non-empty
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn model_name_returns_non_empty() {
        let home = test_path("model-name-home");
        let cwd = test_path("model-name-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let result = client.model_name().await.expect("model_name");
        assert!(
            !result.model_name.is_empty(),
            "model name should not be empty"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Permission mode defaults to default
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn permission_mode_defaults_to_default() {
        let home = test_path("perm-mode-home");
        let cwd = test_path("perm-mode-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let result = client.permission_mode().await.expect("permission_mode");
        assert_eq!(result.mode, "default", "should default to 'default' mode");
    }

    // -----------------------------------------------------------------------
    // 5. List tools includes foundation tools
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_tools_includes_foundation_tools() {
        let home = test_path("tools-home");
        let cwd = test_path("tools-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let tools = client.list_tools().await.expect("list_tools");
        assert!(!tools.is_empty(), "should have foundation tools");
    }

    // -----------------------------------------------------------------------
    // 6. Effort level defaults to None
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn effort_level_defaults_to_none() {
        let home = test_path("effort-home");
        let cwd = test_path("effort-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let result = client.effort_level().await.expect("effort_level");
        // Fresh server should have no effort override set
        assert!(result.effort.is_none(), "effort should default to none");
    }

    // -----------------------------------------------------------------------
    // 7. Full lifecycle: bootstrap and rename
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn full_lifecycle_bootstrap_and_rename() {
        let home = test_path("full-lifecycle-home");
        let cwd = test_path("full-lifecycle-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        // Pre-create a persisted transcript.
        let project_dir = home
            .join("projects")
            .join(orbcode_config::sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        let session_id = "full-lifecycle-session";
        let payload = serde_json::json!({
            "type": "user",
            "uuid": "uuid-fl",
            "timestamp": "2026-06-01T00:00:00.000Z",
            "message": { "role": "user", "content": "lifecycle test" },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        });
        tokio::fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!("{}\n", serde_json::to_string(&payload).expect("serialize")),
        )
        .await
        .expect("write transcript");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        // Bootstrap the persisted session
        let state = client.bootstrap(Some(session_id)).await.expect("bootstrap");
        assert_eq!(state.session.session_id, session_id);

        // The session should appear in the session list
        let sessions = client.list_sessions().await.expect("list sessions");
        assert!(
            sessions.iter().any(|s| s.session_id == session_id),
            "bootstrapped session should appear in list_sessions"
        );

        // Rename the session
        client
            .rename_session(session_id, "Renamed Session")
            .await
            .expect("rename");

        // Verify the rename took effect
        let sessions = client.list_sessions().await.expect("list after rename");
        let our = sessions
            .iter()
            .find(|s| s.session_id == session_id)
            .expect("session should be in list");
        assert_eq!(
            our.custom_title.as_deref(),
            Some("Renamed Session"),
            "custom title should be updated"
        );
    }

    // -----------------------------------------------------------------------
    // 8. Session fork creates a second session
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn session_fork_creates_second_session() {
        let home = test_path("fork-home");
        let cwd = test_path("fork-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        // Pre-create a persisted transcript.
        let project_dir = home
            .join("projects")
            .join(orbcode_config::sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        let session_id = "fork-source-session";
        let payload = serde_json::json!({
            "type": "user",
            "uuid": "uuid-fork",
            "timestamp": "2026-06-01T00:00:00.000Z",
            "message": { "role": "user", "content": "fork test" },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        });
        tokio::fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!("{}\n", serde_json::to_string(&payload).expect("serialize")),
        )
        .await
        .expect("write transcript");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        // Bootstrap the persisted session first
        let _state = client.bootstrap(Some(session_id)).await.expect("bootstrap");

        // Fork the session
        let forked = client
            .fork_session(session_id, Some("forked".into()), None)
            .await
            .expect("fork session");

        let forked_id = forked.session_id.clone();
        assert_ne!(
            forked_id, session_id,
            "forked session should have a different id"
        );

        // Both should appear in the list
        let sessions = client.list_sessions().await.expect("list sessions");
        assert!(
            sessions.iter().any(|s| s.session_id == session_id),
            "original session should be in list"
        );
        assert!(
            sessions.iter().any(|s| s.session_id == forked_id),
            "forked session should be in list"
        );
    }

    // -----------------------------------------------------------------------
    // 9. Context preview has cwd
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn context_preview_has_cwd() {
        let home = test_path("ctx-preview-home");
        let cwd = test_path("ctx-preview-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let ctx = client.context_preview().await.expect("context_preview");
        assert!(
            !ctx.cwd.is_empty(),
            "context_preview cwd should not be empty"
        );
        let cwd_name = cwd
            .file_name()
            .and_then(|n| n.to_str())
            .expect("cwd filename");
        assert!(
            ctx.cwd.contains(cwd_name),
            "context_preview cwd should reference our working directory"
        );
    }

    // -----------------------------------------------------------------------
    // 10. Permission overview returns data
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn permission_overview_returns_data() {
        let home = test_path("perm-ov-home");
        let cwd = test_path("perm-ov-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let overview = client
            .permission_overview()
            .await
            .expect("permission_overview");
        assert!(
            !overview.allow_all,
            "fresh server should not have allow_all"
        );
    }

    // -----------------------------------------------------------------------
    // 11. Protocol round-trip test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn protocol_round_trip() {
        let home = test_path("protocol-rt-home");
        let cwd = test_path("protocol-rt-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        // Pre-create a transcript so the bootstrapped session is discoverable.
        let project_dir = home
            .join("projects")
            .join(orbcode_config::sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        let session_id = "protocol-rt-session";
        let payload = serde_json::json!({
            "type": "user",
            "uuid": "uuid-rt",
            "timestamp": "2026-06-01T00:00:00.000Z",
            "message": { "role": "user", "content": "round trip test" },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
        });
        tokio::fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!("{}\n", serde_json::to_string(&payload).expect("serialize")),
        )
        .await
        .expect("write transcript");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client should initialize via protocol");

        // Bootstrap the persisted session
        let state = client.bootstrap(Some(session_id)).await.expect("bootstrap");
        assert_eq!(state.session.session_id, session_id);

        // List sessions and find it
        let sessions = client.list_sessions().await.expect("list");
        assert!(
            sessions.iter().any(|s| s.session_id == session_id),
            "bootstrap session should appear in list"
        );

        // Check model name through protocol
        let model = client.model_name().await.expect("model_name");
        assert!(!model.model_name.is_empty());

        // List tools through protocol
        let tools = client.list_tools().await.expect("tools");
        assert!(!tools.is_empty());
    }

    // =========================================================================
    // Mock-provider helpers
    // =========================================================================

    fn mock_overrides(scenario: &str) -> HashMap<String, String> {
        let mut env = orbcode_app_server::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            format!("mock://anthropic?scenario={scenario}"),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());
        env
    }

    async fn client_with_mock(label: &str, scenario: &str) -> (AppClient, String) {
        let home = test_path(&format!("{label}-home"));
        let cwd = test_path(&format!("{label}-cwd"));
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                env_overrides: mock_overrides(scenario),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        let state = client.bootstrap(None).await.expect("bootstrap");
        let session_id = state.session.session_id;
        (client, session_id)
    }

    // =========================================================================
    // 12. submit_turn + stream event delivery (P0-1)
    // =========================================================================

    #[tokio::test]
    async fn submit_turn_delivers_stream_events() {
        let (client, session_id) = client_with_mock("stream-events", "hello").await;

        let mut notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");

        let sub = client
            .submit_turn(&session_id, "hello")
            .await
            .expect("submit_turn");
        assert!(!sub.subscription_id.is_empty());

        // StreamEvent uses #[serde(tag = "event")] so the discriminator is at
        // params["event"]["event"] (StreamEventNotification wraps StreamEvent).
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, notif_rx.recv()).await {
                Ok(Some(notif)) => {
                    if notif.method == "stream/event" {
                        events.push(notif.params.clone());
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
            if let Some(last) = events.last() {
                let et = last["event"]["event"].as_str().unwrap_or("");
                if et == "turn_complete" || et == "turn_finished" {
                    break;
                }
            }
        }

        assert!(
            !events.is_empty(),
            "should receive at least one stream event notification"
        );

        let event_types: Vec<&str> = events
            .iter()
            .map(|p| p["event"]["event"].as_str().unwrap_or("unknown"))
            .collect();

        let has_content = events
            .iter()
            .any(|p| p["event"]["event"].as_str() == Some("assistant_delta"));
        assert!(
            has_content,
            "stream should contain assistant_delta events, got: {event_types:?}"
        );
    }

    // =========================================================================
    // 13. Concurrent turn rejection (P0-3)
    // =========================================================================

    #[tokio::test]
    async fn concurrent_turn_rejected_with_active_turn_error() {
        use orbcode_app_server_protocol::ErrorCode;

        use super::ClientError;

        let (client, session_id) = client_with_mock("concurrent-turn", "hang").await;

        let _notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");

        // First turn starts and hangs
        let _sub = client
            .submit_turn(&session_id, "first")
            .await
            .expect("first submit_turn");

        // Give the turn time to register in ActiveTurnRegistry
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Second turn should be rejected
        let result = client.submit_turn(&session_id, "second").await;
        match result {
            Err(ClientError::Protocol(e)) => {
                assert_eq!(
                    e.code,
                    ErrorCode::ActiveTurn,
                    "second turn should get ActiveTurn error, got: {e:?}"
                );
            }
            other => panic!("expected Protocol(ActiveTurn) error, got: {other:?}"),
        }
    }

    // =========================================================================
    // 14. Permission round-trip via server request (P0-2)
    // =========================================================================

    #[tokio::test]
    async fn permission_round_trip_via_server_request() {
        use orbcode_app_server_protocol::ResponseResult;
        use serde_json::json;

        let (client, session_id) =
            client_with_mock("perm-roundtrip", "tool_use&key=bash&command=echo+hi").await;

        let mut notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");
        let mut srv_req_rx = client
            .take_server_request_receiver()
            .await
            .expect("server request receiver");

        let _sub = client
            .submit_turn(&session_id, "run echo hi")
            .await
            .expect("submit_turn");

        // Wait for a permission server-request
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let perm_req = loop {
            tokio::select! {
                    req = srv_req_rx.recv() => {
                        match req {
                            Some(r) if r.method == "permission/request" => break r,
                            Some(_) => {}
                            None => panic!("server request channel closed without permission request"),
                        }
                    }
                    _ = notif_rx.recv() => {}
                _ = tokio::time::sleep_until(deadline) => {
                    panic!("timed out waiting for permission server-request");
                }
            }
        };

        let perm_request_id = perm_req.params["request_id"]
            .as_str()
            .expect("permission request should have request_id")
            .to_string();

        // Respond with approve
        client
            .respond_to_server_request(
                perm_req.id.clone(),
                ResponseResult::Success {
                    data: Some(json!({
                        "request_id": perm_request_id,
                        "decision": { "decision": "approve" }
                    })),
                },
            )
            .await
            .expect("respond_to_server_request");

        // After approval, the turn proceeds. Cancel to avoid infinite tool_use loop.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let cancel_result = client.cancel_turn(&session_id).await;
        assert!(
            cancel_result.is_ok(),
            "cancel_turn should succeed after permission approval"
        );
    }

    // =========================================================================
    // 15. turn/cancel via protocol dispatch (P1-5a)
    // =========================================================================

    #[tokio::test]
    async fn turn_cancel_via_protocol_dispatch() {
        let (client, session_id) = client_with_mock("turn-cancel", "hang").await;

        let _notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");

        let _sub = client
            .submit_turn(&session_id, "hang")
            .await
            .expect("submit_turn");

        tokio::time::sleep(Duration::from_millis(300)).await;

        let result = client.cancel_turn(&session_id).await.expect("cancel_turn");
        assert!(result, "cancel should return true");
    }

    // =========================================================================
    // 16. turn/interrupt via protocol dispatch (P1-5b)
    // =========================================================================

    #[tokio::test]
    async fn turn_interrupt_via_protocol_dispatch() {
        let (client, session_id) = client_with_mock("turn-interrupt", "hang").await;

        let _notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");

        let _sub = client
            .submit_turn(&session_id, "hang")
            .await
            .expect("submit_turn");

        tokio::time::sleep(Duration::from_millis(300)).await;

        let result = client
            .interrupt_turn(&session_id)
            .await
            .expect("interrupt_turn");
        assert!(result, "interrupt should return true");
    }

    // =========================================================================
    // 17. permission/respond sent=true via protocol dispatch
    // =========================================================================

    #[tokio::test]
    async fn permission_respond_sent_true_via_public_api() {
        use orbcode_app_server_protocol::PermissionDecisionWire;

        let (client, session_id) =
            client_with_mock("perm-sent-true", "tool_use&key=bash&command=echo+hi").await;

        let mut notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");
        let _srv_req_rx = client
            .take_server_request_receiver()
            .await
            .expect("server request receiver");

        let _sub = client
            .submit_turn(&session_id, "echo hi")
            .await
            .expect("submit_turn");

        // Wait for the PermissionRequested stream event to learn the request_id.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let perm_request_id = loop {
            match tokio::time::timeout_at(deadline, notif_rx.recv()).await {
                Ok(Some(notif)) if notif.method == "stream/event" => {
                    let evt = &notif.params["event"];
                    if evt["event"].as_str() == Some("permission_requested") {
                        break evt["request"]["request_id"]
                            .as_str()
                            .expect("request_id in PermissionRequested")
                            .to_string();
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("channel closed without PermissionRequested"),
                Err(_) => panic!("timed out waiting for PermissionRequested event"),
            }
        };

        // Call the public API — this exercises the PermissionDecisionWire
        // serialization path end-to-end.
        let result = client
            .respond_to_permission_request(&perm_request_id, PermissionDecisionWire::Approve)
            .await
            .expect("respond_to_permission_request");
        assert!(
            result.sent,
            "respond_to_permission_request with real pending id should return sent=true"
        );

        let _ = client.cancel_turn(&session_id).await;
    }

    // =========================================================================
    // 18. permission/respond sent=false for unknown id
    // =========================================================================

    #[tokio::test]
    async fn permission_respond_sent_false_for_unknown_id() {
        use orbcode_app_server_protocol::PermissionDecisionWire;

        let (client, _session_id) = client_with_mock("perm-sent-false", "hello").await;

        let result = client
            .respond_to_permission_request("nonexistent-id", PermissionDecisionWire::Approve)
            .await
            .expect("respond_to_permission_request");
        assert!(
            !result.sent,
            "respond_to_permission_request with unknown id should return sent=false"
        );
    }

    // =========================================================================
    // 19. Full permission flow: approve → tool result arrives (P2)
    // =========================================================================

    #[tokio::test]
    async fn full_permission_flow_approve_produces_tool_result() {
        use orbcode_app_server_protocol::ResponseResult;
        use serde_json::json;

        let (client, session_id) =
            client_with_mock("perm-full-flow", "tool_use&key=bash&command=echo+hi").await;

        let mut notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");
        let mut srv_req_rx = client
            .take_server_request_receiver()
            .await
            .expect("server request receiver");

        let _sub = client
            .submit_turn(&session_id, "echo hi")
            .await
            .expect("submit_turn");

        // Wait for the first permission server-request
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let perm_req = loop {
            tokio::select! {
                    req = srv_req_rx.recv() => {
                        match req {
                            Some(r) if r.method == "permission/request" => break r,
                            Some(_) => {}
                            None => panic!("channel closed without permission request"),
                        }
                    }
                    _ = notif_rx.recv() => {}
                _ = tokio::time::sleep_until(deadline) => {
                    panic!("timed out waiting for permission server-request");
                }
            }
        };

        let perm_request_id = perm_req.params["request_id"]
            .as_str()
            .expect("request_id")
            .to_string();

        // Approve the permission via server-request response
        client
            .respond_to_server_request(
                perm_req.id.clone(),
                ResponseResult::Success {
                    data: Some(json!({
                        "request_id": perm_request_id,
                        "decision": { "decision": "approve" }
                    })),
                },
            )
            .await
            .expect("respond_to_server_request");

        // After approval, wait for tool-related events (tool_use_started or
        // tool_use_completed) to confirm the tool actually executed.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut saw_tool_event = false;
        loop {
            match tokio::time::timeout_at(deadline, notif_rx.recv()).await {
                Ok(Some(notif)) if notif.method == "stream/event" => {
                    let et = notif.params["event"]["event"].as_str().unwrap_or("");
                    if et == "tool_use_started" || et == "tool_use_completed" {
                        saw_tool_event = true;
                        break;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            saw_tool_event,
            "should receive tool execution event after permission approval"
        );

        let _ = client.cancel_turn(&session_id).await;
    }

    // =========================================================================
    // 20. Turn lifecycle: submit → steer → cancel → TurnCancelled (P2)
    // =========================================================================

    #[tokio::test]
    async fn turn_lifecycle_submit_steer_cancel() {
        let (client, session_id) = client_with_mock("turn-lifecycle", "hang").await;

        let mut notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");

        let _sub = client
            .submit_turn(&session_id, "hanging")
            .await
            .expect("submit_turn");

        // Wait for turn to start
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Steer the turn (inject additional prompt)
        client
            .steer_turn(&session_id, "additional context")
            .await
            .expect("steer_turn");

        // Cancel the turn
        let cancelled = client.cancel_turn(&session_id).await.expect("cancel_turn");
        assert!(cancelled, "cancel_turn should return true");

        // Drain notifications and look for turn_cancelled event
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut saw_cancelled = false;
        loop {
            match tokio::time::timeout_at(deadline, notif_rx.recv()).await {
                Ok(Some(notif)) if notif.method == "stream/event" => {
                    let et = notif.params["event"]["event"].as_str().unwrap_or("");
                    if et == "turn_cancelled" {
                        saw_cancelled = true;
                        break;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            saw_cancelled,
            "should receive turn_cancelled event after cancel"
        );
    }

    // =========================================================================
    // 21. turn/steer E2E (P2)
    // =========================================================================

    #[tokio::test]
    async fn turn_steer_via_protocol_dispatch() {
        let (client, session_id) = client_with_mock("turn-steer", "hang").await;

        let _notif_rx = client
            .take_notification_receiver()
            .await
            .expect("notification receiver");

        let _sub = client
            .submit_turn(&session_id, "hanging")
            .await
            .expect("submit_turn");

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Steer should succeed while turn is active
        client
            .steer_turn(&session_id, "follow-up")
            .await
            .expect("steer_turn should succeed for active turn");

        let _ = client.cancel_turn(&session_id).await;
    }

    // =========================================================================
    // 22. session/rewind E2E (P2)
    // =========================================================================

    #[tokio::test]
    async fn session_rewind_via_protocol_dispatch() {
        let home = test_path("rewind-home");
        let cwd = test_path("rewind-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        // Pre-create a transcript with multiple messages
        let project_dir = home
            .join("projects")
            .join(orbcode_config::sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        let session_id = "rewind-test-session";
        let mut transcript = String::new();
        for i in 0..3 {
            let msg = serde_json::json!({
                "type": "user",
                "uuid": format!("uuid-rewind-{i}"),
                "timestamp": format!("2026-06-01T00:0{i}:00.000Z"),
                "message": { "role": "user", "content": format!("message {i}") },
                "cwd": cwd.display().to_string(),
                "sessionId": session_id,
            });
            transcript.push_str(&serde_json::to_string(&msg).expect("serialize"));
            transcript.push('\n');
        }
        tokio::fs::write(project_dir.join(format!("{session_id}.jsonl")), &transcript)
            .await
            .expect("write transcript");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        // Bootstrap the session
        let state = client.bootstrap(Some(session_id)).await.expect("bootstrap");
        assert_eq!(state.session.session_id, session_id);

        // Rewind to keep only 1 message
        let result = client
            .rewind_session(session_id, 1)
            .await
            .expect("rewind_session");
        assert_eq!(result.session.session_id, session_id);
    }

    // =========================================================================
    // 23. permission/set_mode E2E (P2)
    // =========================================================================

    #[tokio::test]
    async fn permission_set_mode_valid_and_invalid() {
        use orbcode_app_server_protocol::ErrorCode;

        use super::ClientError;

        let home = test_path("set-mode-home");
        let cwd = test_path("set-mode-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        // Default: allow_all should be false
        let overview = client.permission_overview().await.expect("overview");
        assert!(!overview.allow_all);

        // Set to bypassPermissions mode (sets allow_all = true)
        client
            .set_permission_mode("bypassPermissions")
            .await
            .expect("set_permission_mode bypassPermissions");

        // Verify allow_all changed
        let overview = client
            .permission_overview()
            .await
            .expect("overview after set");
        assert!(
            overview.allow_all,
            "bypass_permissions should set allow_all=true"
        );

        // Invalid mode should return InvalidParams
        let result = client.set_permission_mode("nonexistent_mode").await;
        match result {
            Err(ClientError::Protocol(e)) => {
                assert_eq!(e.code, ErrorCode::InvalidParams);
            }
            other => panic!("expected InvalidParams error, got: {other:?}"),
        }
    }

    // =========================================================================
    // 24. Error chain: CoreError → protocol → ClientError::Protocol (P2)
    // =========================================================================

    #[tokio::test]
    async fn error_chain_session_not_found_propagates() {
        use orbcode_app_server_protocol::ErrorCode;

        use super::ClientError;

        let home = test_path("err-chain-home");
        let cwd = test_path("err-chain-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let client = AppClient::from_cwd(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("client");

        // Rewind on a nonexistent session triggers SessionNotFound in core,
        // which propagates through the protocol handler as
        // ErrorCode::SessionNotFound, and surfaces at the client as
        // ClientError::Protocol.
        let result = client.rewind_session("no-such-session", 0).await;
        match result {
            Err(ClientError::Protocol(e)) => {
                assert_eq!(
                    e.code,
                    ErrorCode::SessionNotFound,
                    "should propagate SessionNotFound, got: {e:?}"
                );
            }
            other => panic!("expected Protocol(SessionNotFound), got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn model_and_thinking_controls_change_the_next_provider_request() {
        let (client, session_id) = client_with_mock("sdk-model-thinking", "hello").await;
        let model = "claude-haiku-4-5-20251001";
        let changed = client
            .set_session_model_override(&session_id, Some(model.to_string()))
            .await
            .expect("set model");
        assert_eq!(changed.model, model);
        let thinking = client
            .set_max_thinking_tokens(&session_id, Some(4096))
            .await
            .expect("set thinking");
        assert_eq!(thinking.max_thinking_tokens, Some(4096));

        let mut stream = client
            .submit_turn_stream(&session_id, "first request".to_string())
            .await
            .expect("submit first");
        while let Some(event) = stream.recv().await {
            if event.is_terminal() {
                break;
            }
        }
        let snapshot = client
            .last_provider_request_snapshot_typed()
            .await
            .expect("debug snapshot");
        assert_eq!(snapshot.model, model);
        let body: serde_json::Value =
            serde_json::from_str(&snapshot.body_json).expect("provider request body JSON");
        assert_eq!(body["thinking"]["budget_tokens"], 4096);

        client
            .set_max_thinking_tokens(&session_id, None)
            .await
            .expect("clear thinking");
        let mut stream = client
            .submit_turn_stream(&session_id, "second request".to_string())
            .await
            .expect("submit second");
        while let Some(event) = stream.recv().await {
            if event.is_terminal() {
                break;
            }
        }
        let cleared = client
            .last_provider_request_snapshot_typed()
            .await
            .expect("cleared snapshot");
        assert_eq!(cleared.model, model);
        let cleared_body: serde_json::Value =
            serde_json::from_str(&cleared.body_json).expect("cleared provider request body JSON");
        assert_ne!(cleared_body["thinking"]["budget_tokens"], 4096);
        assert_eq!(
            client
                .status_overview_typed(&session_id)
                .await
                .expect("status")
                .max_thinking_tokens,
            None
        );
    }

    #[tokio::test]
    async fn invalid_thinking_control_does_not_partially_mutate_state() {
        let (client, session_id) = client_with_mock("sdk-thinking-invalid", "hello").await;
        client
            .set_max_thinking_tokens(&session_id, Some(4096))
            .await
            .expect("set valid budget");
        assert!(
            client
                .set_max_thinking_tokens(&session_id, Some(1))
                .await
                .is_err()
        );
        assert_eq!(
            client
                .status_overview_typed(&session_id)
                .await
                .expect("status")
                .max_thinking_tokens,
            Some(4096)
        );
    }
}
