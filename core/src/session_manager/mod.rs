#[cfg(test)]
use std::sync::atomic::Ordering;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::PathBuf,
    sync::{Arc, RwLock, atomic::AtomicBool},
};

#[cfg(test)]
use chrono::Duration as ChronoDuration;
use chrono::{Local, TimeZone, Utc};
use orbcode_config::{
    AgentDefinition, AppConfig, AuthManager, OutputStyleDefinition, ResolvedOutputStyle,
    RuntimeSessionState, built_in_agent_definitions, built_in_output_style_definitions,
    load_agent_definitions, load_output_style_definitions, load_output_style_setting,
    parse_forced_login_method, resolve_active_output_style, sanitize_path,
};
#[cfg(test)]
use orbcode_config::{HookCommand, HookMatcher};
use orbcode_mcp::McpRegistry;
use orbcode_model_provider::{
    CountTokensCache, ProviderDebugTrace, ProviderRequestDebugSnapshot,
    render_pre_user_instructions,
};
#[cfg(test)]
use orbcode_model_provider::{
    ProviderContentBlockDelta, ProviderContentBlockStart, ProviderStreamEvent, ProviderStreamSink,
};
#[cfg(test)]
use orbcode_protocol::PermissionResolutionKind;
#[cfg(test)]
use orbcode_protocol::TurnCancellationKind;
use orbcode_protocol::{
    MessageRole, ProviderId, SessionRecord, SessionSummary, StreamEvent, ToolResultContent,
    TranscriptBlock, TranscriptMessage, TurnContext, visible_content_from_blocks,
};
#[cfg(test)]
use orbcode_session_store::PERSISTED_OUTPUT_TAG;
use orbcode_session_store::{
    ChildSessionStore, LiveSessionRegistryStore, PromptHistoryStore, RawContentBlock, SessionStore,
    raw_content_blocks,
};
use orbcode_tools::{FileReadState, LocalShellTaskRegistry, ToolRegistry};
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use tokio::process::Command as TokioCommand;
use tokio::sync::{Mutex, mpsc};
#[cfg(test)]
use tokio::time::Duration;
use uuid::Uuid;

#[cfg(test)]
use crate::agent_loop::messages::MISSING_TOOL_RESULT;
#[cfg(test)]
use crate::agent_loop::tool_round::ToolRoundToolUse;
#[cfg(test)]
use crate::agent_loop::{
    no_tool::NoToolTurnReason,
    tool_round::{SequentialToolRoundOutcome, ToolRoundResponse, ToolRoundScheduler},
};
#[cfg(test)]
use crate::tool_flow::{INTERRUPTED_TOOL_RESULT, ToolUseOutcome};
#[cfg(test)]
use crate::turn_loop::TurnLoopOutcome;
use crate::{
    BillingBasis, CoreError,
    agent_loop::messages::repair_missing_tool_results,
    context::build_turn_context_with_memory_home,
    context_estimation::estimate_category_breakdown,
    overview::{
        ContextDiagnosticsReport, ContextTokenSource, ContextUsageOverview, CostOverview,
        StatsOverview, UsageOverview, build_context_diagnostics_report,
        build_context_usage_overview, build_stats_overview, build_usage_overview,
    },
    permission_state::{PermissionDecision, PermissionRuntimeState},
    turn_loop::ActiveTurnRegistry,
};

#[cfg(test)]
use orbcode_model_provider::ProviderResponse;

async fn git_worktree_paths(cwd: &PathBuf) -> Vec<PathBuf> {
    let output = TokioCommand::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await;
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

async fn canonical_session_directory(label: &str, path: PathBuf) -> Result<PathBuf, CoreError> {
    if !path.is_absolute() {
        return Err(CoreError::Config(format!(
            "session setup {label} must be absolute: {}",
            path.display()
        )));
    }
    let path = tokio::fs::canonicalize(&path).await?;
    let metadata = tokio::fs::metadata(&path).await?;
    if !metadata.is_dir() {
        return Err(CoreError::Config(format!(
            "session setup {label} is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn datetime_from_millis(value: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(value)
        .single()
        .unwrap_or_else(Utc::now)
}

fn parse_datetime(value: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn workflow_source_step(source_tool_use_id: &str) -> Option<(&str, &str)> {
    let rest = source_tool_use_id.strip_prefix("workflow:")?;
    rest.split_once(':')
}

fn child_messages_from_parent_progress(
    parent_contents: &str,
    source_tool_use_id: &str,
) -> Vec<TranscriptMessage> {
    parent_contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| {
            value.get("type").and_then(Value::as_str) == Some("progress")
                && value.get("parentToolUseID").and_then(Value::as_str) == Some(source_tool_use_id)
                && value.pointer("/data/type").and_then(Value::as_str) == Some("agent_progress")
        })
        .filter_map(|value| {
            let wrapper = value.pointer("/data/message")?;
            let message = wrapper.get("message")?;
            let role = match message.get("role").and_then(Value::as_str) {
                Some("user") => MessageRole::User,
                Some("assistant") => MessageRole::Assistant,
                Some("system") => MessageRole::System,
                _ => return None,
            };
            let content = message.get("content")?;
            let blocks = blocks_from_json_content(content);
            if blocks.is_empty() {
                return None;
            }
            let mut transcript_message =
                TranscriptMessage::from_parts(role, visible_content_from_blocks(&blocks), blocks)
                    .with_synthetic(true);
            if let Some(uuid) = value.get("uuid").and_then(Value::as_str) {
                transcript_message.id = uuid.to_string();
            }
            if let Some(created_at) = value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_datetime)
            {
                transcript_message.created_at = created_at;
            }
            Some(transcript_message)
        })
        .collect()
}

fn blocks_from_json_content(content: &Value) -> Vec<TranscriptBlock> {
    raw_content_blocks(content)
        .into_iter()
        .filter_map(|block| match block {
            RawContentBlock::Text(text) => text
                .text
                .filter(|text| !text.is_empty())
                .map(|text| TranscriptBlock::Text { text }),
            RawContentBlock::Thinking(thinking) => {
                let text = thinking.thinking.or(thinking.text).unwrap_or_default();
                if text.is_empty() {
                    None
                } else {
                    Some(TranscriptBlock::Thinking {
                        text,
                        signature: thinking.signature.filter(|value| !value.is_empty()),
                    })
                }
            }
            RawContentBlock::ToolUse(tool_use) => {
                let input = tool_use
                    .input
                    .as_ref()
                    .and_then(|value| serde_json::to_string(value).ok())
                    .unwrap_or_default();
                Some(TranscriptBlock::ToolUse {
                    id: tool_use.id.unwrap_or_else(|| "tool-use".to_string()),
                    name: tool_use.name.unwrap_or_else(|| "tool".to_string()),
                    input,
                })
            }
            RawContentBlock::ToolResult(tool_result) => Some(TranscriptBlock::ToolResult {
                tool_use_id: tool_result
                    .tool_use_id
                    .unwrap_or_else(|| "tool-result".to_string()),
                content: ToolResultContent::from_loaded(tool_result.content),
                is_error: tool_result.is_error.unwrap_or(false),
                metadata: None,
            }),
            RawContentBlock::Other(value) => value
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(|text| TranscriptBlock::Text {
                    text: text.to_string(),
                }),
            RawContentBlock::RedactedThinking => Some(TranscriptBlock::Text {
                text: "[redacted thinking]".to_string(),
            }),
        })
        .collect()
}

#[derive(Clone)]
pub struct SessionManager {
    config: AppConfig,
    auth: AuthManager,
    tools: ToolRegistry,
    mcp: McpRegistry,
    active_turns: ActiveTurnRegistry,
    permission_runtime: PermissionRuntimeState,
    runtime_state: RuntimeSessionState,
    provider_debug_trace: ProviderDebugTrace,
    queued_model_visible_contexts: QueuedModelVisibleContextStore,
    queued_user_commands: QueuedUserCommandStore,
    live_session_registry: LiveSessionRegistryStore,
    prompt_history_store: PromptHistoryStore,
    transcript_store: SessionStore,
    child_session_store: ChildSessionStore,
    agents: Arc<Vec<AgentDefinition>>,
    output_styles: Arc<RwLock<Arc<Vec<OutputStyleDefinition>>>>,
    active_output_style: Arc<RwLock<Arc<ResolvedOutputStyle>>>,
    count_tokens_cache: Arc<CountTokensCache>,
    live_cost: LiveCostStore,
    local_shell_tasks: LocalShellTaskRegistry,
    read_state: Arc<FileReadState>,
    ask_user_pending:
        Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>>>,
    /// Per-session mutex serializing transcript appends. `append_message` reads
    /// the current last message to compute `parent_uuid` and then writes; if two
    /// turn drivers overlap (e.g. an interrupted turn still draining while the
    /// next turn starts), an unsynchronized read-then-write would fork the
    /// `parent_uuid` chain. Holding this lock across the read+write makes appends
    /// atomic per session.
    transcript_append_locks: TranscriptAppendLocks,
}

impl fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionManager")
            .field("cwd", &self.config.cwd)
            .field("home_dir", &self.config.home_dir)
            .field("default_provider", &self.config.default_provider)
            .field("agents_len", &self.agents.len())
            .finish_non_exhaustive()
    }
}

/// Live per-session cumulative cost.
///
/// Accumulation is idempotent per persisted message id, so a usage value is
/// counted at most once regardless of whether it was first observed via the
/// transcript seed (on resume) or via the live turn loop. This is what keeps
/// the `/cost` total equal to a fresh transcript recompute and prevents double
/// counting across `--continue`/resume.
#[derive(Default)]
struct LiveCostState {
    tracker: crate::CostTracker,
    counted_message_ids: std::collections::HashSet<String>,
    seeded: bool,
}

type LiveCostStore = Arc<Mutex<HashMap<String, LiveCostState>>>;

#[derive(Clone, Debug)]
struct QueuedModelVisibleContext {
    activity_type: String,
    label: String,
    message: TranscriptMessage,
}

#[derive(Default)]
struct QueuedModelVisibleContextState {
    hold_depth: usize,
    contexts: Vec<QueuedModelVisibleContext>,
}

type QueuedModelVisibleContextStore = Arc<Mutex<HashMap<String, QueuedModelVisibleContextState>>>;

/// A user command or attachment submitted while a tool round is executing.
/// These are drained between tool rounds and injected into the next provider
/// request as real (non-synthetic) user messages so the model sees them.
#[derive(Clone, Debug)]
pub(crate) struct QueuedUserCommand {
    pub(crate) content: String,
}

type QueuedUserCommandStore = Arc<Mutex<HashMap<String, Vec<QueuedUserCommand>>>>;

/// Per-session locks that serialize transcript appends. See
/// [`SessionManager::transcript_append_locks`].
type TranscriptAppendLocks = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

mod session_stream;

use session_stream::LiveToolProgressReporter;
#[cfg(test)]
use session_stream::SessionProviderStreamSink;

const INTERRUPTED_TURN_MESSAGE: &str = "[Request interrupted by user]";
const INTERRUPTED_TURN_MESSAGE_FOR_TOOL_USE: &str = "[Request interrupted by user for tool use]";
const PERMISSION_DENIED_RETRY_MESSAGE: &str = "The PermissionDenied hook indicated this command is now approved. You may retry it if you would like.";
const POST_TOOL_FAILURE_RETRY_MESSAGE: &str = "The PostToolUseFailure hook indicated that this tool use may be retried. You may retry the command if you would like.";

mod session_tool_runtime;

mod session_compaction;
pub use session_compaction::CompactDecision;

mod session_tool_results;

mod session_hooks;

mod session_agent_tool;

mod session_background_agent;

mod session_workflows;
pub use session_workflows::{WorkflowCommand, WorkflowSource};

#[derive(Clone, Debug, Default, Serialize)]
pub struct ChildSessionOrphanCleanupResult {
    pub dry_run: bool,
    pub scoped_cwds: Vec<String>,
    pub inspected_metadata: usize,
    pub orphan_metadata: usize,
    pub eligible_metadata: usize,
    pub stale_running_metadata: usize,
    pub skipped_running_metadata: usize,
    pub removed_metadata: usize,
    pub removed_transcripts: usize,
    pub orphan_child_session_ids: Vec<String>,
}

mod session_turn_loop;

mod session_response;

mod session_provider_request;

mod session_transcript;

mod session_facade;

impl SessionManager {
    pub async fn new(
        config: AppConfig,
        tools: ToolRegistry,
        mcp: McpRegistry,
    ) -> Result<Self, CoreError> {
        let auth = AuthManager::new(config.home_dir.clone())
            .with_env_overrides(config.env_overrides.clone())
            .with_openai_proxy_config(config.outbound_proxy_config())
            .with_forced_login_method(
                config
                    .forced_login_method()
                    .and_then(parse_forced_login_method),
            );
        Self::new_with_auth(config, tools, mcp, auth).await
    }

    pub async fn new_with_auth(
        config: AppConfig,
        tools: ToolRegistry,
        mcp: McpRegistry,
        auth: AuthManager,
    ) -> Result<Self, CoreError> {
        auth.refresh_stored_state().await?;
        let transcript_store = SessionStore::new(
            config.current_project_dir.clone(),
            config.cwd.clone(),
            config.provider_model_name(config.default_provider),
        );
        let prompt_history_store = PromptHistoryStore::new(
            config.home_dir.clone(),
            config.history_path.clone(),
            config.cwd.clone(),
        );
        let live_session_registry =
            LiveSessionRegistryStore::new(config.sessions_dir.clone(), config.cwd.clone());
        let child_session_store = ChildSessionStore::new(config.sessions_dir.clone());
        let local_shell_tasks = LocalShellTaskRegistry::new(&config.home_dir);
        let read_state = Arc::new(FileReadState::with_persistence(
            config.home_dir.join("file-read-state.json"),
        ));
        let initial_styles = built_in_output_style_definitions();
        let initial_active =
            resolve_active_output_style(&initial_styles, orbcode_config::DEFAULT_OUTPUT_STYLE_NAME);
        Ok(Self {
            config,
            auth,
            tools,
            mcp,
            active_turns: ActiveTurnRegistry::new(),
            permission_runtime: PermissionRuntimeState::new(),
            runtime_state: RuntimeSessionState::new(),
            provider_debug_trace: ProviderDebugTrace::new(),
            queued_model_visible_contexts: QueuedModelVisibleContextStore::default(),
            queued_user_commands: QueuedUserCommandStore::default(),
            live_session_registry,
            prompt_history_store,
            transcript_store,
            child_session_store,
            agents: Arc::new(built_in_agent_definitions()),
            output_styles: Arc::new(RwLock::new(Arc::new(initial_styles))),
            active_output_style: Arc::new(RwLock::new(Arc::new(initial_active))),
            count_tokens_cache: Arc::new(CountTokensCache::new()),
            live_cost: LiveCostStore::default(),
            local_shell_tasks,
            read_state,
            ask_user_pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
            transcript_append_locks: TranscriptAppendLocks::default(),
        })
    }

    /// Resolve a pending AskUserQuestion request. Called by the protocol layer
    /// (via AppServer) when the client responds to a server-request.
    pub fn resolve_ask_user_question(&self, request_id: &str, answer: Option<String>) {
        if let Some(tx) = self.ask_user_pending.lock().unwrap().remove(request_id) {
            let _ = tx.send(answer);
        }
    }

    /// Cancel specific pending AskUserQuestion requests by request_id,
    /// resolving each with `None`. Used by per-pump cleanup guards so one
    /// pump ending doesn't cancel unrelated sessions' requests.
    pub fn cancel_pending_ask_user(&self, request_ids: &[String]) {
        let mut pending = self.ask_user_pending.lock().unwrap();
        for rid in request_ids {
            if let Some(tx) = pending.remove(rid) {
                let _ = tx.send(None);
            }
        }
    }

    #[doc(hidden)]
    pub fn ask_user_pending_for_test(
        &self,
    ) -> Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>>> {
        self.ask_user_pending.clone()
    }

    /// Shared durable registry that backs Bash subprocess execution. Exposed so
    /// the app-server can wire the same instance into one-shot tool contexts and
    /// so callers can read registry truth (status / pid / exit / kill reason)
    /// after a turn ends.
    pub fn local_shell_tasks(&self) -> &LocalShellTaskRegistry {
        &self.local_shell_tasks
    }

    pub async fn child_sessions_for(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<orbcode_session_store::ChildSessionMetadata>, CoreError> {
        self.child_session_store
            .list_for_parent(parent_session_id)
            .await
            .map_err(CoreError::from)
    }

    /// Refreshes agent definitions from disk (built-in + user + project).
    /// Falls back to built-in only if disk loading fails.
    pub async fn refresh_agent_definitions(&mut self) {
        let config = self.effective_config();
        match load_agent_definitions(&config.home_dir, &config.cwd).await {
            Ok(definitions) => self.agents = Arc::new(definitions),
            Err(_) => self.agents = Arc::new(built_in_agent_definitions()),
        }
    }

    pub fn agent_definitions(&self) -> Arc<Vec<AgentDefinition>> {
        Arc::clone(&self.agents)
    }

    pub(super) fn lookup_agent_definition(&self, agent_type: &str) -> Option<AgentDefinition> {
        self.agents
            .iter()
            .find(|definition| definition.agent_type == agent_type)
            .cloned()
    }

    /// Reloads output-style definitions from disk (built-in + user + project +
    /// enabled plugins) and re-resolves the active style against the settings
    /// cascade. Falls back to the built-in `default` style on any error so the
    /// session always has a usable style to inject. Uses interior mutability so
    /// runtime callers (slash commands, picker overlays) can refresh without a
    /// `&mut SessionManager` handle.
    pub async fn refresh_output_styles(&self) {
        let config = self.effective_config();
        let definitions = match load_output_style_definitions(&config.home_dir, &config.cwd).await {
            Ok(definitions) => definitions,
            Err(_) => built_in_output_style_definitions(),
        };
        let requested = load_output_style_setting(&config.home_dir, &config.cwd)
            .await
            .unwrap_or_else(|_| orbcode_config::DEFAULT_OUTPUT_STYLE_NAME.to_string());
        let active = resolve_active_output_style(&definitions, &requested);
        {
            let mut guard = match self.output_styles.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Arc::new(definitions);
        }
        {
            let mut guard = match self.active_output_style.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Arc::new(active);
        }
    }

    pub fn output_style_definitions(&self) -> Arc<Vec<OutputStyleDefinition>> {
        match self.output_styles.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    pub fn active_output_style(&self) -> Arc<ResolvedOutputStyle> {
        match self.active_output_style.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    pub async fn context_preview(&self) -> orbcode_protocol::TurnContext {
        let config = self.effective_config();
        build_turn_context_with_memory_home(
            &config.cwd,
            &self.additional_directories(),
            &config.home_dir,
        )
        .await
    }

    pub async fn pre_user_instructions_preview(&self, session_id: &str) -> String {
        let context = self.context_preview().await;
        let request = self
            .provider_request_for_messages(session_id, "", context, Vec::new(), true, true)
            .await;
        render_pre_user_instructions(&request)
    }

    pub async fn last_provider_request_snapshot(&self) -> Option<ProviderRequestDebugSnapshot> {
        self.provider_debug_trace.snapshot().await
    }

    pub async fn last_provider_request_snapshot_for_source(
        &self,
        source: &str,
    ) -> Option<ProviderRequestDebugSnapshot> {
        self.provider_debug_trace.snapshot_for_source(source).await
    }

    pub async fn context_usage_overview(
        &self,
        session_id: &str,
        context: TurnContext,
    ) -> Result<ContextUsageOverview, CoreError> {
        self.context_usage_and_diagnostics(session_id, context)
            .await
            .map(|(usage, _)| usage)
    }

    pub async fn context_usage_and_diagnostics(
        &self,
        session_id: &str,
        context: TurnContext,
    ) -> Result<(ContextUsageOverview, ContextDiagnosticsReport), CoreError> {
        let config = self.effective_config();
        let messages = match self
            .transcript_store
            .load_session_if_present(session_id)
            .await?
        {
            Some(session) => {
                self.model_visible_messages_with_tool_result_budget(session_id, session.messages)
                    .await?
            }
            None => Vec::new(),
        };
        let request = self
            .provider_request_for_messages(session_id, "", context, messages, true, true)
            .await;
        let categories = estimate_category_breakdown(&request);
        let count_tokens_request =
            self.count_tokens_request_for_provider(config.default_provider, &request, &config);
        let (estimated_tokens, token_source) = match self
            .count_tokens_cached(config.default_provider, &count_tokens_request)
            .await
        {
            Ok(Some(tokens)) => (
                tokens.min(u32::MAX as usize) as u32,
                ContextTokenSource::ProviderCountTokens,
            ),
            Ok(None) | Err(_) => (
                categories.total(),
                ContextTokenSource::RoughEstimateFallback,
            ),
        };

        let usage = build_context_usage_overview(
            request.model.clone(),
            estimated_tokens,
            token_source,
            categories,
            &config.context_window_options(),
            &config.max_output_token_options(),
            &config.token_warning_options(),
        );
        let mcp_server_count = self.mcp.list_servers().await.len();
        let mcp_capability_count = self.mcp.capabilities().await.len();
        let report = build_context_diagnostics_report(
            &request,
            &config,
            &usage,
            mcp_server_count,
            mcp_capability_count,
        );

        Ok((usage, report))
    }

    pub async fn usage_overview(&self, session_id: &str) -> Result<UsageOverview, CoreError> {
        let session = self
            .transcript_store
            .load_session_if_present(session_id)
            .await?
            .unwrap_or_else(|| SessionRecord {
                session_id: session_id.to_string(),
                title: None,
                custom_title: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                cwd: None,
                git_branch: None,
                model: None,
                provider: None,
                additional_directories: Vec::new(),
                session_allowed_tools: Vec::new(),
                session_disallowed_tools: Vec::new(),
                session_effort: None,
                messages: Vec::new(),
            });

        // Use the effective config so a runtime `/model` override is reflected
        // in the reported window/model (matching `model_display_name`), not the
        // startup-configured model.
        let config = self.effective_config();
        let (api_model, context_window, max_output, billing_basis) = self.cost_pricing_params();
        Ok(build_usage_overview(
            session,
            self.model_display_name(),
            &api_model,
            config.default_provider,
            context_window,
            max_output,
            billing_basis,
        ))
    }

    /// Model/window parameters used for cost computation. Mirrors the values
    /// `usage_overview` feeds into `build_usage_overview`, so the live tracker
    /// and a transcript recompute price identical usage identically.
    ///
    /// Uses the effective config so a runtime `/model` override prices `/cost`
    /// and the budget cap against the model actually in use, not the one set at
    /// startup.
    fn cost_pricing_params(&self) -> (String, u32, u32, BillingBasis) {
        self.cost_pricing_params_for(self.effective_config().default_provider)
    }

    fn cost_pricing_params_for(&self, provider: ProviderId) -> (String, u32, u32, BillingBasis) {
        let config = self.effective_config();
        let billing_basis = if self.uses_chatgpt_subscription_for(provider) {
            BillingBasis::Subscription
        } else {
            BillingBasis::Api
        };
        let api_model = if billing_basis == BillingBasis::Subscription
            && !config.provider_model_is_explicit()
        {
            "gpt-5.6-sol".to_string()
        } else {
            config.provider_model_name(provider)
        };
        let context_window =
            orbcode_config::resolve_context_window(&api_model, &config.context_window_options());
        let max_output = orbcode_config::resolve_max_output_tokens(
            &api_model,
            &config.max_output_token_options(),
        );
        (api_model, context_window, max_output, billing_basis)
    }

    fn cost_pricing_params_for_message(
        &self,
        message: &TranscriptMessage,
    ) -> (String, u32, u32, BillingBasis) {
        let Some(attribution) = message.cost_attribution.as_ref() else {
            return self.cost_pricing_params();
        };
        let config = self.effective_config();
        let context_window = orbcode_config::resolve_context_window(
            &attribution.model,
            &config.context_window_options(),
        );
        let max_output = orbcode_config::resolve_max_output_tokens(
            &attribution.model,
            &config.max_output_token_options(),
        );
        (
            attribution.model.clone(),
            context_window,
            max_output,
            if attribution.subscription {
                BillingBasis::Subscription
            } else {
                BillingBasis::Api
            },
        )
    }

    pub(super) fn with_message_cost_attribution(
        &self,
        message: TranscriptMessage,
        provider: ProviderId,
    ) -> TranscriptMessage {
        if message.usage.is_none() || message.cost_attribution.is_some() {
            return message;
        }
        let (model, _, _, billing_basis) = self.cost_pricing_params_for(provider);
        message.with_cost_attribution(provider, model, billing_basis == BillingBasis::Subscription)
    }

    pub(super) fn uses_chatgpt_subscription(&self) -> bool {
        self.uses_chatgpt_subscription_for(self.effective_config().default_provider)
    }

    fn uses_chatgpt_subscription_for(&self, provider: ProviderId) -> bool {
        let config = self.effective_config();
        provider == ProviderId::OpenAi
            && config.resolve_env("OPENAI_API_KEY").is_none()
            && self.auth.cached_api_key(ProviderId::OpenAi).is_none()
            && self
                .auth
                .cached_chatgpt_oauth()
                .is_some_and(|credentials| credentials.is_usable())
    }

    /// Seeds the live cost tracker for `session_id` from the persisted
    /// transcript the first time it is touched in this process. This is the
    /// restore-across-resume path: every already-persisted message with usage
    /// is counted once (keyed by message id). Idempotent.
    /// Drop the cached live-cost state for a session so the next access
    /// re-seeds it from the current (possibly truncated) transcript. Used after
    /// a rewind, which discards messages that must stop counting toward `/cost`.
    pub(super) async fn reset_live_cost(&self, session_id: &str) {
        self.live_cost.lock().await.remove(session_id);
    }

    async fn ensure_live_cost_seeded(&self, session_id: &str) -> Result<(), CoreError> {
        {
            let guard = self.live_cost.lock().await;
            if guard.get(session_id).is_some_and(|state| state.seeded) {
                return Ok(());
            }
        }

        let session = self
            .transcript_store
            .load_session_if_present(session_id)
            .await?;
        let mut guard = self.live_cost.lock().await;
        let state = guard.entry(session_id.to_string()).or_default();
        if state.seeded {
            return Ok(());
        }
        if let Some(session) = session {
            for message in &session.messages {
                let Some(usage) = message.usage.as_ref() else {
                    continue;
                };
                if state.counted_message_ids.insert(message.id.clone()) {
                    let (api_model, context_window, max_output, billing_basis) =
                        self.cost_pricing_params_for_message(message);
                    match billing_basis {
                        BillingBasis::Subscription => state.tracker.add_subscription_usage(
                            &api_model,
                            usage,
                            context_window,
                            max_output,
                        ),
                        BillingBasis::Api | BillingBasis::Mixed => {
                            state
                                .tracker
                                .add_usage(&api_model, usage, context_window, max_output);
                        }
                    }
                }
            }
        }
        state.seeded = true;
        Ok(())
    }

    /// Records a single persisted message's usage in the live cost tracker.
    /// Counted at most once per message id, so calling this for a message that
    /// the transcript seed already absorbed is a no-op.
    pub(super) async fn accumulate_live_cost(&self, session_id: &str, message: &TranscriptMessage) {
        let Some(usage) = message.usage.as_ref() else {
            return;
        };
        if self.ensure_live_cost_seeded(session_id).await.is_err() {
            return;
        }
        let (api_model, context_window, max_output, billing_basis) =
            self.cost_pricing_params_for_message(message);
        let mut guard = self.live_cost.lock().await;
        let state = guard.entry(session_id.to_string()).or_default();
        if state.counted_message_ids.insert(message.id.clone()) {
            match billing_basis {
                BillingBasis::Subscription => state.tracker.add_subscription_usage(
                    &api_model,
                    usage,
                    context_window,
                    max_output,
                ),
                BillingBasis::Api | BillingBasis::Mixed => {
                    state
                        .tracker
                        .add_usage(&api_model, usage, context_window, max_output);
                }
            }
        }
    }

    /// Current accumulated session cost paired with whether all of it could be
    /// priced. Returns `(total_usd, pricing_known)`; `pricing_known` is `false`
    /// when some usage came from an unpriced model. On a seed failure it returns
    /// a conservative `(0.0, true)` so a transient error never spuriously blocks
    /// a turn.
    pub(super) async fn live_cost_total(&self, session_id: &str) -> (f64, bool) {
        if self.ensure_live_cost_seeded(session_id).await.is_err() {
            return (0.0, true);
        }
        let guard = self.live_cost.lock().await;
        guard.get(session_id).map_or((0.0, true), |state| {
            (
                state.tracker.total_cost_usd(),
                !state.tracker.has_unknown_model_cost(),
            )
        })
    }

    /// Cumulative cost for `session_id`, restored from the transcript on first
    /// access and kept current by the live turn loop.
    pub async fn cost_overview(&self, session_id: &str) -> Result<CostOverview, CoreError> {
        self.ensure_live_cost_seeded(session_id).await?;
        let guard = self.live_cost.lock().await;
        let cost = guard
            .get(session_id)
            .map(|state| state.tracker.clone().into_summary())
            .unwrap_or_default();
        Ok(CostOverview {
            session_id: session_id.to_string(),
            model: self.model_display_name(),
            provider: self.config.default_provider,
            cost,
        })
    }

    pub async fn stats_overview(&self) -> Result<StatsOverview, CoreError> {
        const WINDOW_DAYS: usize = 180;
        Ok(build_stats_overview(
            self.transcript_store.load_project_sessions().await?,
            WINDOW_DAYS,
            Local::now().date_naive(),
        ))
    }

    pub async fn prompt_history(&self, limit: usize) -> Result<Vec<String>, CoreError> {
        self.prompt_history_store
            .load_recent(limit)
            .await
            .map_err(Into::into)
    }

    /// Same as [`Self::prompt_history`] but yields entries from `session_id`
    /// before entries from other sessions for the current project — matches
    /// `getHistory` in `claude-code/src/history.ts`.
    pub async fn prompt_history_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<String>, CoreError> {
        self.prompt_history_store
            .load_recent_for_session(Some(session_id), limit)
            .await
            .map_err(Into::into)
    }

    /// Forget the most recent prompt-history append (in this process) so
    /// auto-restore-on-interrupt does not show the restored text twice.
    pub fn remove_last_prompt_history_entry(&self) {
        self.prompt_history_store.remove_last();
    }

    #[cfg(test)]
    pub(super) fn prompt_history_store(&self) -> &PromptHistoryStore {
        &self.prompt_history_store
    }

    pub async fn start_or_resume(
        &self,
        requested_session: Option<&str>,
    ) -> Result<(SessionRecord, StreamEvent), CoreError> {
        self.start_or_resume_with_setup(requested_session, None, Vec::new(), Vec::new())
            .await
    }

    pub async fn start_or_resume_with_setup(
        &self,
        requested_session: Option<&str>,
        cwd: Option<PathBuf>,
        additional_directories: Vec<PathBuf>,
        session_mcp_servers: Vec<orbcode_mcp::McpServerConfig>,
    ) -> Result<(SessionRecord, StreamEvent), CoreError> {
        if requested_session.is_some()
            && (cwd.is_some()
                || !additional_directories.is_empty()
                || !session_mcp_servers.is_empty())
        {
            return Err(CoreError::Config(
                "session setup overrides can only be used for new sessions".into(),
            ));
        }

        let session = match requested_session {
            Some(session_id) => {
                let session = self.load_and_repair_session(session_id).await?;
                self.restore_runtime_context_for_session(&session).await;
                session
            }
            None => {
                let cwd = match cwd {
                    Some(cwd) => Some(canonical_session_directory("cwd", cwd).await?),
                    None => None,
                };
                let mut resolved_additional_directories =
                    Vec::with_capacity(additional_directories.len());
                for directory in additional_directories {
                    let directory =
                        canonical_session_directory("additional directory", directory).await?;
                    if !resolved_additional_directories
                        .iter()
                        .any(|existing| existing == &directory)
                    {
                        resolved_additional_directories.push(directory);
                    }
                }

                self.runtime_state.set_cwd_override(cwd.clone());
                self.runtime_state
                    .set_runtime_additional_directories(resolved_additional_directories.clone());
                self.permission_runtime
                    .set_session_permission_rules(Vec::new(), Vec::new());
                self.runtime_state.set_effort_override(None);
                self.runtime_state.set_max_thinking_tokens(None);

                let mut session = SessionRecord::new();
                if let Some(cwd) = cwd.as_ref() {
                    session.cwd = Some(cwd.display().to_string());
                    let project_dir = self
                        .config
                        .projects_dir
                        .join(sanitize_path(&cwd.display().to_string()));
                    self.transcript_store.record_session_location(
                        &session.session_id,
                        &project_dir.join(format!("{}.jsonl", session.session_id)),
                        cwd,
                    );
                }
                session.additional_directories = resolved_additional_directories
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();

                self.mcp
                    .upsert_session_servers(&session.session_id, session_mcp_servers)
                    .await;
                session
            }
        };
        let config = self.effective_config();
        self.transcript_store
            .record_session_cwd(&session.session_id, &config.cwd);
        self.live_session_registry
            .register_with_cwd(&session.session_id, "interactive", config.cwd)
            .await?;

        let event = match requested_session {
            Some(_) => StreamEvent::SessionLoaded {
                summary: session.summary(),
            },
            None => StreamEvent::SessionStarted {
                summary: session.summary(),
            },
        };
        Ok((session, event))
    }

    pub async fn load_session_with_setup(
        &self,
        session_id: &str,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
        session_mcp_servers: Vec<orbcode_mcp::McpServerConfig>,
    ) -> Result<(SessionRecord, StreamEvent), CoreError> {
        let cwd = canonical_session_directory("cwd", cwd).await?;
        let mut resolved_additional_directories = Vec::with_capacity(additional_directories.len());
        for directory in additional_directories {
            let directory = canonical_session_directory("additional directory", directory).await?;
            if !resolved_additional_directories
                .iter()
                .any(|existing| existing == &directory)
            {
                resolved_additional_directories.push(directory);
            }
        }

        let session = self.load_and_repair_session(session_id).await?;
        self.restore_runtime_context_for_session(&session).await;
        self.runtime_state.set_cwd_override(Some(cwd));
        self.runtime_state
            .set_runtime_additional_directories(resolved_additional_directories);
        self.mcp.remove_session_servers(&session.session_id).await;
        self.mcp
            .upsert_session_servers(&session.session_id, session_mcp_servers)
            .await;

        let config = self.effective_config();
        self.transcript_store
            .record_session_cwd(&session.session_id, &config.cwd);
        self.live_session_registry
            .register_with_cwd(&session.session_id, "interactive", config.cwd)
            .await?;

        let event = StreamEvent::SessionLoaded {
            summary: session.summary(),
        };
        Ok((session, event))
    }

    pub async fn continue_latest(&self) -> Result<(SessionRecord, StreamEvent), CoreError> {
        let session_id = self
            .list_sessions()
            .await?
            .into_iter()
            .find(|summary| matches!(summary.status, orbcode_protocol::SessionStatus::Available))
            .map(|summary| summary.session_id)
            .ok_or_else(|| CoreError::SessionNotFound("latest".to_string()))?;
        let session = self.load_and_repair_session(&session_id).await?;
        self.restore_runtime_context_for_session(&session).await;
        let config = self.effective_config();
        self.transcript_store
            .record_session_cwd(&session.session_id, &config.cwd);
        self.live_session_registry
            .register_with_cwd(&session.session_id, "interactive", config.cwd)
            .await?;
        Ok((
            session.clone(),
            StreamEvent::SessionLoaded {
                summary: session.summary(),
            },
        ))
    }

    async fn load_and_repair_session(&self, session_id: &str) -> Result<SessionRecord, CoreError> {
        let session = match self
            .transcript_store
            .load_session_any_project(session_id)
            .await
        {
            Ok(session) => session,
            Err(orbcode_session_store::SessionStoreError::SessionNotFound(_)) => {
                self.load_persisted_child_session(session_id).await?
            }
            Err(error) => return Err(error.into()),
        };
        self.repair_resumed_session(session).await
    }

    async fn load_persisted_child_session(
        &self,
        child_session_id: &str,
    ) -> Result<SessionRecord, CoreError> {
        let Some(metadata) = self.child_session_store.load(child_session_id).await? else {
            return Err(CoreError::SessionNotFound(child_session_id.to_string()));
        };
        let transcript_path = self
            .child_session_store
            .transcript_path_for(child_session_id);
        if !tokio::fs::try_exists(&transcript_path).await? {
            return Err(CoreError::SessionNotFound(child_session_id.to_string()));
        }
        self.transcript_store.record_session_location(
            child_session_id,
            &transcript_path,
            &PathBuf::from(metadata.cwd),
        );
        self.transcript_store
            .load_session_from_path_as(child_session_id, &transcript_path)
            .await
            .map_err(Into::into)
    }

    /// Loads a session record for read-only viewing (e.g. opening a workflow
    /// agent step's output). Unlike resume, this NEVER restores the live
    /// session's runtime context (cwd/permissions/effort) or touches the
    /// live-session registry — viewing a step must not clobber the parent
    /// session that may still be running a turn.
    pub async fn load_session_for_view(
        &self,
        session_id: &str,
    ) -> Result<(SessionRecord, StreamEvent), CoreError> {
        let session = match self.load_and_repair_session(session_id).await {
            Ok(session) => session,
            Err(CoreError::SessionNotFound(_)) => {
                self.build_child_session_record(session_id).await?
            }
            Err(error) => return Err(error),
        };
        let event = StreamEvent::SessionLoaded {
            summary: session.summary(),
        };
        Ok((session, event))
    }

    pub async fn load_child_session_record(
        &self,
        child_session_id: &str,
    ) -> Result<(SessionRecord, StreamEvent), CoreError> {
        let session = self.build_child_session_record(child_session_id).await?;
        self.restore_runtime_context_for_session(&session).await;
        let event = StreamEvent::SessionLoaded {
            summary: session.summary(),
        };
        Ok((session, event))
    }

    async fn build_child_session_record(
        &self,
        child_session_id: &str,
    ) -> Result<SessionRecord, CoreError> {
        let Some(metadata) = self.child_session_store.load(child_session_id).await? else {
            return Err(CoreError::SessionNotFound(child_session_id.to_string()));
        };
        let parent = self
            .load_and_repair_session(&metadata.parent_session_id)
            .await?;
        let parent_path = self.transcript_store.path(&metadata.parent_session_id);
        let parent_contents = tokio::fs::read_to_string(&parent_path)
            .await
            .map_err(|error| {
                orbcode_session_store::SessionStoreError::transcript_io(
                    "read transcript",
                    &parent_path,
                    error,
                )
            })?;

        let mut session = SessionRecord::new();
        session.session_id = metadata.child_session_id.clone();
        session.title = Some(format!("Agent: {}", metadata.prompt_preview));
        session.created_at = datetime_from_millis(metadata.started_at);
        session.updated_at = datetime_from_millis(metadata.last_activity_at);
        session.cwd = Some(metadata.cwd.clone());
        session.model = metadata.model.clone().or(parent.model.clone());
        session.provider = parent.provider;

        for message in
            child_messages_from_parent_progress(&parent_contents, &metadata.source_tool_use_id)
        {
            session.push_message(message);
        }
        if session.messages.is_empty() && !metadata.prompt_preview.trim().is_empty() {
            let mut message = TranscriptMessage::new(MessageRole::User, metadata.prompt_preview);
            message.created_at = session.created_at;
            session.push_message(message.with_synthetic(true));
        }
        if let Some(message) = self
            .workflow_child_result_message(&metadata.source_tool_use_id)
            .await?
        {
            session.push_message(message);
        } else if let Some(error) = metadata.error_message.as_deref() {
            let mut message =
                TranscriptMessage::new(MessageRole::Assistant, format!("Agent failed: {error}"));
            message.created_at = datetime_from_millis(metadata.last_activity_at);
            session.push_message(message.with_synthetic(true));
        }
        if let Some(ended_at) = metadata.ended_at {
            session.updated_at = datetime_from_millis(ended_at);
        }

        Ok(session)
    }

    async fn workflow_child_result_message(
        &self,
        source_tool_use_id: &str,
    ) -> Result<Option<TranscriptMessage>, CoreError> {
        let Some((run_id, step_key)) = workflow_source_step(source_tool_use_id) else {
            return Ok(None);
        };
        let journal_path = self
            .config
            .home_dir
            .join("workflow-runs")
            .join(run_id)
            .join("journal.jsonl");
        let contents = match tokio::fs::read_to_string(&journal_path).await {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(orbcode_session_store::SessionStoreError::transcript_io(
                    "read workflow journal",
                    &journal_path,
                    error,
                )
                .into());
            }
        };
        for line in contents.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if value.get("step_key").and_then(Value::as_str) != Some(step_key) {
                continue;
            }
            let created_at = value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_datetime)
                .unwrap_or_else(Utc::now);
            match value.get("event").and_then(Value::as_str) {
                Some("step_completed") => {
                    let Some(output) = value.get("output").and_then(Value::as_str) else {
                        continue;
                    };
                    let mut message = TranscriptMessage::new(MessageRole::Assistant, output);
                    message.created_at = created_at;
                    return Ok(Some(message.with_synthetic(true)));
                }
                Some("step_failed") => {
                    let Some(error) = value.get("message").and_then(Value::as_str) else {
                        continue;
                    };
                    let mut message = TranscriptMessage::new(
                        MessageRole::Assistant,
                        format!("Step failed: {error}"),
                    );
                    message.created_at = created_at;
                    return Ok(Some(message.with_synthetic(true)));
                }
                _ => {}
            }
        }
        Ok(None)
    }

    pub async fn cwd_for_session(&self, session_id: &str) -> Result<PathBuf, CoreError> {
        match self
            .transcript_store
            .load_session_any_project(session_id)
            .await
        {
            Ok(session) => {
                if let Some(cwd) = session.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
                    return Ok(PathBuf::from(cwd));
                }
            }
            Err(orbcode_session_store::SessionStoreError::SessionNotFound(_)) => {}
            Err(error) => return Err(error.into()),
        }

        self.transcript_store
            .recorded_session_cwd(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    async fn repair_resumed_session(
        &self,
        mut session: SessionRecord,
    ) -> Result<SessionRecord, CoreError> {
        let repaired_messages = repair_missing_tool_results(session.messages.clone());
        if repaired_messages != session.messages {
            session.messages = repaired_messages;
            self.transcript_store.persist_full_session(&session).await?;
        }
        Ok(session)
    }

    async fn restore_runtime_context_for_session(&self, session: &SessionRecord) {
        self.runtime_state.set_max_thinking_tokens(None);
        self.runtime_state
            .set_runtime_additional_directories(Vec::new());
        self.permission_runtime.set_session_permission_rules(
            session.session_allowed_tools.clone(),
            session.session_disallowed_tools.clone(),
        );
        self.runtime_state
            .set_effort_override(session.session_effort);
        let Some(cwd) = session
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.trim().is_empty())
            .map(std::path::PathBuf::from)
        else {
            self.restore_session_additional_directories(session).await;
            return;
        };
        let Ok(metadata) = tokio::fs::metadata(&cwd).await else {
            self.restore_session_additional_directories(session).await;
            return;
        };
        if metadata.is_dir() {
            self.runtime_state.set_cwd_override(Some(cwd));
        }
        self.restore_session_additional_directories(session).await;
    }

    async fn restore_session_additional_directories(&self, session: &SessionRecord) {
        let mut directories = Vec::new();
        for directory in &session.additional_directories {
            let path = std::path::PathBuf::from(directory);
            let Ok(metadata) = tokio::fs::metadata(&path).await else {
                continue;
            };
            if metadata.is_dir() && !directories.iter().any(|existing| existing == &path) {
                directories.push(path);
            }
        }
        self.runtime_state
            .set_runtime_additional_directories(directories);
    }

    pub async fn clear_session(
        &self,
        previous_session_id: &str,
    ) -> Result<SessionRecord, CoreError> {
        // Signal the driver to stop (via the cancel flag) as well as freeing the
        // slot. `remove_session` alone would leave the detached driver running —
        // it would keep issuing provider requests and appending to the now
        // abandoned session id.
        self.active_turns.interrupt(previous_session_id).await;
        self.permission_runtime.clear_denied_tool_calls().await;

        let session = SessionRecord::new();
        self.runtime_state.set_cwd_override(None);
        self.runtime_state
            .set_runtime_additional_directories(Vec::new());
        self.permission_runtime
            .set_session_permission_rules(Vec::new(), Vec::new());
        self.runtime_state.set_effort_override(None);
        self.runtime_state.set_max_thinking_tokens(None);
        let config = self.effective_config();
        self.transcript_store
            .record_session_cwd(&session.session_id, &config.cwd);
        self.live_session_registry
            .register_with_cwd(&session.session_id, "interactive", config.cwd)
            .await?;
        Ok(session)
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, CoreError> {
        let prefixes = self.same_repo_project_prefixes().await;
        let mut summaries = self
            .transcript_store
            .load_project_session_summaries_for_prefixes(&prefixes)
            .await?;
        let child_session_ids = self
            .child_session_store
            .list_all()
            .await?
            .into_iter()
            .map(|metadata| metadata.child_session_id)
            .collect::<HashSet<_>>();
        summaries.retain(|summary| !child_session_ids.contains(&summary.session_id));

        // Stable ordering: most recently updated first, with session_id as a
        // deterministic tiebreaker so callers see the same order across
        // refreshes when timestamps collide (rounded mtimes for corrupt
        // entries can land on the same second).
        summaries.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Ok(summaries)
    }

    pub async fn session_id_for_exact_custom_title(
        &self,
        title: &str,
    ) -> Result<Option<String>, CoreError> {
        let normalized = title.trim().to_lowercase();
        if normalized.is_empty() {
            return Ok(None);
        }
        let matches =
            self.list_sessions()
                .await?
                .into_iter()
                .filter(|summary| {
                    matches!(summary.status, orbcode_protocol::SessionStatus::Available)
                        && summary.custom_title.as_deref().is_some_and(|candidate| {
                            candidate.trim().eq_ignore_ascii_case(&normalized)
                        })
                })
                .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [summary] => Ok(Some(summary.session_id.clone())),
            _ => Err(CoreError::Config(format!(
                "multiple sessions match title \"{}\"",
                title.trim()
            ))),
        }
    }

    pub async fn delete_acp_visible_session(
        &self,
        session_id: &str,
        cwd: PathBuf,
    ) -> Result<(), CoreError> {
        let cwd = canonical_session_directory("cwd", cwd).await?;
        if self.active_turns.has_active_session(session_id).await {
            return Err(CoreError::ActiveTurn(session_id.to_string()));
        }

        let summaries = self.list_sessions().await?;
        let Some(summary) = summaries
            .into_iter()
            .find(|summary| summary.session_id == session_id)
        else {
            return Err(CoreError::SessionNotFound(session_id.to_string()));
        };

        if let orbcode_protocol::SessionStatus::Corrupt { reason } = &summary.status {
            return Err(CoreError::Config(format!(
                "session/delete cannot delete corrupt transcript {session_id}: {reason}"
            )));
        }

        let Some(persisted_cwd) = summary.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty())
        else {
            return Err(CoreError::Config(format!(
                "session/delete session {session_id} has no persisted cwd"
            )));
        };
        let persisted_cwd = PathBuf::from(persisted_cwd);
        if !persisted_cwd.is_absolute() {
            return Err(CoreError::Config(format!(
                "session/delete session {session_id} has relative persisted cwd: {}",
                persisted_cwd.display()
            )));
        }
        let persisted_cwd = canonical_session_directory("persisted cwd", persisted_cwd).await?;
        if persisted_cwd != cwd {
            return Err(CoreError::Config(format!(
                "session/delete cwd mismatch for session {session_id}: requested {}, persisted {}",
                cwd.display(),
                persisted_cwd.display()
            )));
        }

        let path = summary
            .transcript_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| {
                CoreError::Config(format!(
                    "session/delete session {session_id} has no transcript path"
                ))
            })?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                self.child_session_store
                    .remove_for_parent(session_id)
                    .await?;
                self.discard_queued_user_commands(session_id).await;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(CoreError::SessionNotFound(session_id.to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn same_repo_project_prefixes(&self) -> Vec<String> {
        let config = self.effective_config();
        let worktrees = git_worktree_paths(&config.cwd).await;
        if worktrees.len() <= 1 {
            return Vec::new();
        }
        let mut prefixes = worktrees
            .into_iter()
            .map(|path| sanitize_path(&path.display().to_string()))
            .collect::<Vec<_>>();
        prefixes.sort();
        prefixes.dedup();
        prefixes
    }

    /// Rename a session — appends a `custom-title` entry so subsequent loads
    /// use the new title without losing the original auto-generated one. The
    /// append also materializes the transcript when called on a session that
    /// has not yet recorded any model-visible message, so the TUI can rename
    /// a freshly bootstrapped session and still see it in `/sessions`.
    pub async fn rename_session(&self, session_id: &str, new_title: &str) -> Result<(), CoreError> {
        self.transcript_store
            .rename_session(session_id, new_title)
            .await
            .map_err(Into::into)
    }

    /// Snapshot of transcript storage health for the active project dir.
    /// Used by `doctor` to surface recovery hints when transcripts are
    /// corrupt, partially written, or the directory is read-only.
    pub async fn session_storage_health(&self) -> orbcode_session_store::SessionStorageHealth {
        let mut health = self.transcript_store.storage_health().await;
        let prefixes = self.same_repo_project_prefixes().await;
        let parent_session_ids = self
            .transcript_store
            .load_project_session_summaries_for_prefixes(&prefixes)
            .await
            .map(|summaries| {
                summaries
                    .into_iter()
                    .map(|summary| summary.session_id)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let scoped_child_cwds = self.scoped_child_session_cwds().await;
        match self
            .child_session_store
            .storage_health(&parent_session_ids, Some(&scoped_child_cwds))
            .await
        {
            Ok(child_health) => health.child_sessions = child_health,
            Err(error) => health.child_session_scan_error = Some(error.to_string()),
        }
        health
    }

    async fn scoped_child_session_cwds(&self) -> HashSet<String> {
        let config = self.effective_config();
        let mut scoped_child_cwds = HashSet::new();
        scoped_child_cwds.insert(config.cwd.display().to_string());
        for worktree in git_worktree_paths(&config.cwd).await {
            scoped_child_cwds.insert(worktree.display().to_string());
        }
        scoped_child_cwds
    }

    pub async fn cleanup_orphan_child_sessions(
        &self,
        dry_run: bool,
        stale_running_cutoff_ms: Option<i64>,
    ) -> Result<ChildSessionOrphanCleanupResult, CoreError> {
        let prefixes = self.same_repo_project_prefixes().await;
        let parent_session_ids = self
            .transcript_store
            .load_project_session_summaries_for_prefixes(&prefixes)
            .await?
            .into_iter()
            .map(|summary| summary.session_id)
            .collect::<HashSet<_>>();
        let scoped_child_cwds = self.scoped_child_session_cwds().await;
        let mut scoped_cwds = scoped_child_cwds.iter().cloned().collect::<Vec<_>>();
        scoped_cwds.sort();

        let mut result = ChildSessionOrphanCleanupResult {
            dry_run,
            scoped_cwds,
            ..ChildSessionOrphanCleanupResult::default()
        };

        for child in self.child_session_store.list_all().await? {
            if !scoped_child_cwds.contains(&child.cwd) {
                continue;
            }
            result.inspected_metadata += 1;
            if parent_session_ids.contains(&child.parent_session_id) {
                continue;
            }

            result.orphan_metadata += 1;
            if child.status == orbcode_session_store::ChildSessionStatus::Running {
                if stale_running_cutoff_ms.is_some_and(|cutoff| child.last_activity_at < cutoff) {
                    result.stale_running_metadata += 1;
                } else {
                    result.skipped_running_metadata += 1;
                    continue;
                }
            }

            result.eligible_metadata += 1;
            result
                .orphan_child_session_ids
                .push(child.child_session_id.clone());
            if dry_run {
                continue;
            }

            let cleanup = self
                .child_session_store
                .remove(&child.child_session_id)
                .await?;
            result.removed_metadata += cleanup.metadata_removed;
            result.removed_transcripts += cleanup.transcripts_removed;
        }

        Ok(result)
    }

    pub async fn gc_stale_sessions(
        &self,
        threshold_days: u64,
    ) -> Result<orbcode_session_store::GcResult, CoreError> {
        let mut result = self
            .transcript_store
            .gc_stale_sessions(threshold_days)
            .await?;
        for session_id in result.removed_ids.clone() {
            let child_cleanup = self
                .child_session_store
                .remove_for_parent(&session_id)
                .await?;
            result.removed_child_metadata += child_cleanup.metadata_removed;
            result.removed_child_transcripts += child_cleanup.transcripts_removed;
        }
        Ok(result)
    }

    pub async fn submit_turn(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, CoreError> {
        let prompt = prompt.into();
        let turn_id = Uuid::new_v4();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        self.active_turns
            .insert(session_id, turn_id, cancel_flag.clone())
            .await?;

        // The active-turn entry is normally cleared by the detached driver task
        // when it exits. If any of the fallible setup below errors, that task is
        // never spawned, so the entry would leak and wedge the session forever
        // with `CoreError::ActiveTurn`. Run the setup fallibly and, on error,
        // clear the entry we just inserted before returning.
        let config = self.effective_config();
        let user_message = TranscriptMessage::new(MessageRole::User, prompt.clone());
        let setup_result = async {
            self.live_session_registry
                .register_with_cwd(session_id, "interactive", config.cwd.clone())
                .await?;
            self.prompt_history_store
                .append(session_id, &prompt)
                .await?;
            self.append_message(session_id, user_message.clone())
                .await?;
            self.provider_debug_trace.clear().await;
            Ok::<(), CoreError>(())
        }
        .await;
        if let Err(error) = setup_result {
            self.active_turns
                .clear_if_matching(session_id, turn_id)
                .await;
            return Err(error);
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(StreamEvent::UserMessage {
            message: user_message,
        });

        let manager = self.clone();
        let session_id = session_id.to_string();
        // Detached turn driver; active-turn state is cleared when the task exits.
        let _turn_handle = tokio::spawn(async move {
            manager
                .run_turn_loop(&session_id, turn_id, &prompt, &config, cancel_flag, &tx)
                .await;
            manager
                .active_turns
                .clear_if_matching(&session_id, turn_id)
                .await;
        });

        Ok(rx)
    }

    /// Enqueue a user command to be injected into the next provider request.
    /// Called by the TUI/CLI when the user submits input while a tool round is
    /// still executing.
    pub async fn enqueue_user_command(&self, session_id: &str, content: String) {
        let mut store = self.queued_user_commands.lock().await;
        store
            .entry(session_id.to_string())
            .or_default()
            .push(QueuedUserCommand { content });
    }

    pub async fn steer_turn(
        &self,
        session_id: &str,
        content: impl Into<String>,
    ) -> Result<(), CoreError> {
        if !self.active_turns.has_active_session(session_id).await {
            return Err(CoreError::NoActiveTurn(session_id.to_string()));
        }
        let content = content.into();
        if content.trim().is_empty() {
            return Ok(());
        }
        self.prompt_history_store
            .append(session_id, &content)
            .await?;
        self.enqueue_user_command(session_id, content).await;
        Ok(())
    }

    /// Drain all queued user commands for a session, returning them in
    /// submission order. Called between tool rounds to inject them into the
    /// transcript before the next provider request.
    pub(super) async fn drain_queued_user_commands(
        &self,
        session_id: &str,
    ) -> Vec<QueuedUserCommand> {
        let mut store = self.queued_user_commands.lock().await;
        store
            .get_mut(session_id)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    pub(super) async fn has_queued_user_commands(&self, session_id: &str) -> bool {
        self.queued_user_commands
            .lock()
            .await
            .get(session_id)
            .is_some_and(|commands| !commands.is_empty())
    }

    async fn discard_queued_user_commands(&self, session_id: &str) {
        self.queued_user_commands.lock().await.remove(session_id);
    }

    pub async fn respond_to_permission_request(
        &self,
        request_id: &str,
        decision: PermissionDecision,
    ) -> bool {
        self.permission_runtime
            .respond_to_permission_request(request_id, decision)
            .await
    }

    pub async fn cancel_turn(&self, session_id: &str) -> bool {
        let cancelled = self.active_turns.cancel(session_id).await;
        if cancelled {
            self.discard_queued_user_commands(session_id).await;
        }
        cancelled
    }

    pub async fn interrupt_turn(&self, session_id: &str) -> bool {
        let interrupted = self.active_turns.interrupt(session_id).await;
        if interrupted {
            self.discard_queued_user_commands(session_id).await;
        }
        interrupted
    }
}

#[cfg(test)]
mod tests;
