mod agent_loop;
mod agent_tool;
mod compaction;
mod config_provider;
mod context;
mod context_estimation;
mod hook_runner;
mod hooks;
mod model_cost;
mod overview;
mod permission_state;
mod permissions;
mod retry;
mod session_manager;
mod system_prompt;
mod tool_flow;
mod tool_progress;
mod tool_runtime;
mod turn_loop;

use std::path::PathBuf;

pub use compaction::CompactSessionResult;
pub use config_provider::apply_provider_request_options;
pub use model_cost::{
    BillingBasis, CostSummary, CostTracker, ModelCosts, ModelUsage, calculate_usd_cost,
    format_cost, format_model_pricing, get_model_costs,
};
use orbcode_config::ConfigError;
use orbcode_mcp::McpError;
pub use orbcode_model_provider::{
    ProviderDescriptor, ProviderRequestDebugSnapshot, supported_providers,
};
use orbcode_protocol::StreamErrorCategory;
pub use orbcode_session_store::SessionStorageHealth;
use orbcode_session_store::SessionStoreError;
use orbcode_tools::ToolError;
pub use overview::{
    ContextCategoryBreakdown, ContextDiagnosticCategory, ContextDiagnosticSection,
    ContextDiagnosticStatus, ContextDiagnosticsReport, ContextTokenSource, ContextUsageOverview,
    CostOverview, StatsActivityDay, StatsOverview, UsageOverview,
};
pub use permission_state::PermissionDecision;
pub use permissions::{
    PermissionContext, PermissionRule, mcp_permission_target, normalize_permission_rule_for_edit,
    suggested_bash_permission_rules,
};
pub use session_manager::{
    ChildSessionOrphanCleanupResult, CompactDecision, SessionManager, WorkflowCommand,
    WorkflowSource,
};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ProviderFailure {
    pub message: String,
    pub category: StreamErrorCategory,
    pub suggestion: Option<String>,
}

impl std::fmt::Display for ProviderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl ProviderFailure {
    pub fn from_message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            category: StreamErrorCategory::Other,
            suggestion: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("turn already active for session: {0}")]
    ActiveTurn(String),
    #[error("no active turn for session: {0}")]
    NoActiveTurn(String),
    #[error("workflow run already active in this process: {0}")]
    ActiveWorkflow(String),
    #[error("corrupt workflow journal {path} at line {line}: {source}")]
    WorkflowJournalCorrupt {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0}")]
    PermissionDenied(String),
    #[error("request cancelled")]
    Cancelled,
    #[error("{0}")]
    ProviderFailed(ProviderFailure),
    #[error("{0}")]
    RetryExhausted(ProviderFailure),
    #[error("{0}")]
    Tool(String),
    #[error("{0}")]
    ToolErr(#[from] ToolError),
    #[error("{0}")]
    Mcp(#[from] McpError),
}

impl From<ConfigError> for CoreError {
    fn from(error: ConfigError) -> Self {
        match error {
            ConfigError::Config(message) => Self::Config(message),
            ConfigError::Io(error) => Self::Io(error),
            ConfigError::Json(error) => Self::Json(error),
        }
    }
}

impl From<SessionStoreError> for CoreError {
    fn from(error: SessionStoreError) -> Self {
        match error {
            SessionStoreError::Config(message) => Self::Config(message),
            SessionStoreError::SessionNotFound(session_id) => Self::SessionNotFound(session_id),
            SessionStoreError::Io(error) => Self::Io(error),
            // Preserve the original error kind so callers can still pattern
            // match on PermissionDenied / NotFound while keeping the path
            // and operation in the displayed message.
            SessionStoreError::TranscriptIo {
                operation,
                path,
                hint,
                source,
            } => {
                let kind = source.kind();
                let hint = hint.map(|hint| format!(" ({hint})")).unwrap_or_default();
                Self::Io(std::io::Error::new(
                    kind,
                    format!("{operation} failed for {}: {source}{hint}", path.display()),
                ))
            }
            SessionStoreError::Json(error) => Self::Json(error),
        }
    }
}
