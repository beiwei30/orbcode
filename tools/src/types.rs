use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use orbcode_mcp::McpRegistry;
use orbcode_protocol::{
    AskUserQuestionSpec, AskUserResponseOutcome, ProviderToolDefinition, SandboxMode,
};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::time::{Duration, sleep};

use tokio::sync::{mpsc, oneshot};

use crate::local_shell_task::LocalShellTaskRegistry;
use crate::skills::SkillDefinition;

/// A request from a tool to ask the user a question. The tool sends this
/// on `ToolContext::ask_user_tx` and awaits the oneshot for the answer.
pub struct AskUserRequest {
    pub request_id: String,
    pub questions: Vec<AskUserQuestionSpec>,
    pub response_tx: oneshot::Sender<AskUserResponseOutcome>,
}

impl fmt::Debug for AskUserRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AskUserRequest")
            .field("request_id", &self.request_id)
            .field("questions", &self.questions)
            .field("response_tx", &"<oneshot sender>")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentToolInput {
    pub description: String,
    pub prompt: String,
    #[serde(default, alias = "subagentType")]
    pub subagent_type: Option<String>,
    #[serde(default, alias = "runInBackground")]
    pub run_in_background: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Available,
}

/// Static permission capability for a tool. Call-specific boundaries (for
/// example a path outside the workspace or a Bash sandbox escalation) are
/// derived by core from this capability and the tool input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCapability {
    Internal,
    WorkspaceRead,
    WorkspaceWrite,
    SandboxedCommand,
    Network,
    ExternalSideEffect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub status: ToolStatus,
    pub summary: &'static str,
    pub requires_tools_permission: bool,
    pub requires_network_permission: bool,
    pub capability: ToolCapability,
    pub provider_hidden: bool,
}

/// Per-provider-request visibility for client-owned interaction tools.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteractionToolVisibility {
    pub ask_user_question: bool,
}

type CwdChangeCallback = Arc<dyn Fn(&Path) + Send + Sync>;

#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub additional_directories: Vec<PathBuf>,
    pub home_dir: PathBuf,
    pub sandbox_mode: SandboxMode,
    pub sandbox_allow_network: bool,
    pub allow_network: bool,
    pub allow_tools: bool,
    pub mcp: McpRegistry,
    pub progress: Option<Arc<dyn ToolProgressReporter>>,
    pub cancellation: ToolCancellationToken,
    /// Per-session read-state table for edit/write freshness validation. `None`
    /// disables the check (preserving legacy behavior for callers that have not
    /// wired a session-scoped table).
    pub read_state: Option<Arc<crate::file_state::FileReadState>>,
    /// Session that owns local shell tasks spawned by this invocation. `None`
    /// falls back to a generic id so one-shot tool runs still persist a record.
    pub session_id: Option<String>,
    /// Shared durable registry that backs Bash subprocess execution. `None`
    /// makes the tool build a fresh registry rooted at `home_dir`, so the
    /// on-disk records are still produced; sharing one instance keeps the
    /// process-local live buffers and cancellation flags consistent.
    pub local_shell_tasks: Option<LocalShellTaskRegistry>,
    /// Callback invoked when the Bash tool detects that a command changed the
    /// working directory. The caller (typically `SessionManager`) wires this
    /// to update the session's effective cwd so subsequent tool calls start
    /// in the new directory.
    pub on_cwd_change: Option<CwdChangeCallback>,
    /// Optional override for the plans directory. When set (e.g. from the
    /// `plansDirectory` setting), plan tools store plan files here instead
    /// of the default `home_dir/plans/`.
    pub plans_directory_override: Option<PathBuf>,
    /// Channel for AskUserQuestion tool to send requests to the session
    /// manager, which forwards them as StreamEvents. `None` in contexts
    /// where interactive prompting is not available.
    pub ask_user_tx: Option<mpsc::UnboundedSender<AskUserRequest>>,
    /// Settings-level env overrides (`settings.json.env`). Enables tools
    /// to resolve env vars with the same canonical → legacy → settings-env
    /// fallback chain that `AppConfig::resolve_env` uses.
    pub settings_env: std::collections::BTreeMap<String, String>,
    /// Optional preloaded skill definitions. Session callers use this to pass
    /// MCP-backed skills discovered from the current registry; when absent the
    /// Skill tool falls back to filesystem/plugin discovery.
    pub skill_definitions: Option<Vec<SkillDefinition>>,
}

impl fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolContext")
            .field("cwd", &self.cwd)
            .field("additional_directories", &self.additional_directories)
            .field("home_dir", &self.home_dir)
            .field("sandbox_mode", &self.sandbox_mode)
            .field("sandbox_allow_network", &self.sandbox_allow_network)
            .field("allow_network", &self.allow_network)
            .field("allow_tools", &self.allow_tools)
            .field("mcp", &self.mcp)
            .field("has_progress", &self.progress.is_some())
            .field("has_cancellation", &self.cancellation.is_cancelled())
            .field("has_read_state", &self.read_state.is_some())
            .field("session_id", &self.session_id)
            .field("has_local_shell_tasks", &self.local_shell_tasks.is_some())
            .field("has_on_cwd_change", &self.on_cwd_change.is_some())
            .field("plans_directory_override", &self.plans_directory_override)
            .field("has_ask_user_tx", &self.ask_user_tx.is_some())
            .field("settings_env_len", &self.settings_env.len())
            .field(
                "skill_definitions_len",
                &self.skill_definitions.as_ref().map(Vec::len),
            )
            .finish()
    }
}

impl ToolContext {
    /// Resolve an env var through the alias table with process env and
    /// settings.env fallback. Equivalent to `AppConfig::resolve_env`
    /// for the env layers (without the test-only `env_overrides` seal).
    pub(crate) fn resolve_env(&self, key: &str) -> Option<String> {
        orbcode_config::resolve_env_value_with(key, &self.settings_env, |k| std::env::var(k).ok())
    }

    /// The registry that should back local shell / Bash execution: the shared
    /// instance when one was wired, otherwise a disk-rooted registry at
    /// `home_dir` so records still persist.
    pub(crate) fn local_shell_registry(&self) -> LocalShellTaskRegistry {
        self.local_shell_tasks
            .clone()
            .unwrap_or_else(|| LocalShellTaskRegistry::new(&self.home_dir))
    }

    pub(crate) fn notify_cwd_change(&self, new_cwd: &Path) {
        if let Some(ref callback) = self.on_cwd_change {
            callback(new_cwd);
        }
    }

    /// Session id local shell records are filed under. Falls back to a stable
    /// placeholder so one-shot invocations (e.g. `orbcode tool bash`) still own a
    /// record that `list_for_session` can recover.
    pub(crate) fn local_shell_session_id(&self) -> String {
        self.session_id
            .clone()
            .unwrap_or_else(|| "local-shell".to_string())
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolCancellationToken {
    flag: Option<Arc<AtomicBool>>,
}

impl ToolCancellationToken {
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self { flag: Some(flag) }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    pub(crate) async fn cancelled(&self) {
        while !self.is_cancelled() {
            sleep(Duration::from_millis(10)).await;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutcome {
    pub name: String,
    pub summary: String,
    pub output: String,
    pub metadata: Option<Value>,
    pub changed_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspacePlanSnapshot {
    pub plan_file: PathBuf,
    pub state_file: PathBuf,
    pub in_plan_mode: bool,
    pub plan: String,
}

#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    pub(crate) planned: Vec<ToolSpec>,
    pub(crate) dynamic_definitions: Arc<std::sync::RwLock<Vec<ProviderToolDefinition>>>,
    pub(crate) feature_disabled_tools: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    NotFound(String),
    #[error("tool permissions are disabled")]
    PermissionDenied,
    #[error("network permissions are disabled")]
    NetworkDenied,
    #[error("invalid tool input: {0}")]
    InvalidInput(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("tool execution failed: {0}")]
    ExecutionFailed(String),
    #[error("tool execution failed: {message}")]
    ExecutionFailedWithSource {
        message: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("tool execution failed: {message}")]
    ExecutionFailedWithMetadata { message: String, metadata: Value },
    #[error("tool interrupted")]
    Interrupted,
    #[error("tool interrupted")]
    InterruptedWithMetadata { metadata: Value },
    #[error("tool interaction cancelled")]
    CancelledWithMetadata { metadata: Value },
    #[error("{0}")]
    Mcp(String),
    #[error("plugin tool error: {0}")]
    PluginDispatch(#[from] PluginDispatchError),
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PluginDispatchError {
    #[error("plugin `{plugin}` is not installed")]
    PluginNotInstalled { plugin: String, tool: String },
    #[error("plugin `{plugin}` is disabled")]
    PluginDisabled { plugin: String, tool: String },
    #[error("plugin `{plugin}` does not expose tool `{tool}`")]
    ToolNotFound { plugin: String, tool: String },
    #[error(
        "plugin tool runtime is not available; plugin `{plugin}` tool `{tool}` cannot be executed"
    )]
    UnsupportedRuntime { plugin: String, tool: String },
}

impl ToolError {
    pub(crate) fn execution_failed_source(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::ExecutionFailedWithSource {
            message: source.to_string(),
            source: Box::new(source),
        }
    }

    pub fn is_interrupted(&self) -> bool {
        matches!(
            self,
            ToolError::Interrupted | ToolError::InterruptedWithMetadata { .. }
        )
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, ToolError::CancelledWithMetadata { .. })
    }

    pub fn metadata(&self) -> Option<Value> {
        match self {
            ToolError::ExecutionFailedWithMetadata { metadata, .. }
            | ToolError::InterruptedWithMetadata { metadata }
            | ToolError::CancelledWithMetadata { metadata } => Some(metadata.clone()),
            _ => None,
        }
    }
}

#[async_trait]
pub trait ToolProgressReporter: Send + Sync {
    async fn report(&self, progress: Value) -> Result<(), ToolError>;
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn execution_failed_source_retains_source() {
        let error = ToolError::execution_failed_source(std::io::Error::other("reader failed"));

        assert_eq!(error.to_string(), "tool execution failed: reader failed");
        assert!(error.source().is_some());
    }

    #[test]
    fn agent_tool_input_accepts_camel_case_subagent_type_alias() {
        let input = serde_json::from_str::<AgentToolInput>(
            r#"{"description":"Explore repo","prompt":"inspect","subagentType":"Explore"}"#,
        )
        .expect("parse agent tool input");

        assert_eq!(input.description, "Explore repo");
        assert_eq!(input.prompt, "inspect");
        assert_eq!(input.subagent_type.as_deref(), Some("Explore"));
        assert!(!input.run_in_background);
    }

    #[test]
    fn agent_tool_input_parses_run_in_background_flag() {
        let snake = serde_json::from_str::<AgentToolInput>(
            r#"{"description":"d","prompt":"p","run_in_background":true}"#,
        )
        .expect("parse run_in_background");
        assert!(snake.run_in_background);

        let camel = serde_json::from_str::<AgentToolInput>(
            r#"{"description":"d","prompt":"p","runInBackground":true}"#,
        )
        .expect("parse runInBackground alias");
        assert!(camel.run_in_background);
    }
}
