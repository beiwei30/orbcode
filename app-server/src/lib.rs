use std::path::{Path, PathBuf};
use std::sync::Arc;

mod advanced;
mod auth_api;
mod background;
mod background_agent;
mod background_api;
mod background_task_view;
mod bootstrap;
mod diagnostics;
mod doctor;
mod mcp_api;
pub mod message_processor;
mod permissions;
mod protocol_conversion;
pub mod protocol_handler;
mod sessions;
mod settings;
mod tools_api;
mod workflow_api;

pub use advanced::{AdvancedCapability, AdvancedCapabilityStatus};
use background::BackgroundManager;
pub(crate) use background::{
    BackgroundJobRecord, BackgroundTaskOutputResponse, CreateBackgroundJob,
};
pub use orbcode_app_server_protocol::{
    AddDirectoryCandidate, AddedDirectory, BootstrapState, ContextOverview, DoctorCheck,
    DoctorReport, DoctorStatus, McpResourceSlashSuggestion, McpServerSlashSuggestion,
    McpSlashSuggestionCatalog, McpToolSlashSuggestion, MemoryFileOverview, MemoryOverview,
    PermissionOverview, PlanOverview, PolicyConflictOverview, PolicyOverview, PolicySourceOverview,
    StatusOverview, WorkspaceDiff,
};
pub use orbcode_config::{
    AgentDefinition, AgentLoadWarning, AgentSource, AgentWarningKind, AppConfigOverrides,
    AuthMethod, AuthOverview, AuthStatusEntry, ChatGptBrowserLoginSession,
    ChatGptDeviceLoginSession, ContextWindowOptions, DiscoveredHook, EditorModeSetting,
    HookDiscovery, HookDiscoveryWarning, MaxOutputTokenOptions, ModelOption, OutputStyleOption,
    PermissionMode, PermissionRuleSettingKind, ResolvedKeybindings, SandboxFilesystemLocalSettings,
    SandboxLocalSettings, SandboxNetworkLocalSettings, SandboxSettingsUpdate, ThemeSetting,
    TokenWarningOptions, calculate_token_warning_state, parse_tool_rule_list,
    sealed_provider_env_overrides,
};
use orbcode_config::{AppConfig, AuthManager, load_plugin_registry, plugin_mcp_config_sources};
use orbcode_config::{
    PluginMcpConfigSource as ConfigPluginMcpConfigSource,
    PluginMcpConfigSourceKind as ConfigPluginMcpConfigSourceKind,
};
use orbcode_core::SessionManager;
pub use orbcode_core::{
    BillingBasis, CompactDecision, CompactSessionResult, ContextCategoryBreakdown,
    ContextDiagnosticsReport, ContextTokenSource, ContextUsageOverview, CoreError, CostOverview,
    CostSummary, ModelUsage, PermissionContext, PermissionDecision, PermissionRule,
    ProviderDescriptor, ProviderRequestDebugSnapshot, StatsActivityDay, StatsOverview,
    UsageOverview, WorkflowCommand, WorkflowSource, format_cost, mcp_permission_target,
    normalize_permission_rule_for_edit, suggested_bash_permission_rules,
};
pub use orbcode_mcp::{
    McpAuth, McpDiagnosticStatus, McpOAuthBrowserLoginInput, McpOAuthDeviceLoginInput,
    McpOAuthOverview, McpOAuthStatusEntry, McpOAuthTokenInput, McpPromptResult, McpServerConfig,
    McpServerStatus, McpServerTrust, McpTransport,
};
use orbcode_mcp::{McpLoadOptions, McpPluginConfigSource, McpPluginConfigSourceKind, McpRegistry};
pub use orbcode_protocol::{
    BackgroundTaskProgressEvent, BackgroundTaskView, BackgroundTaskViewKind,
    BackgroundTaskViewStatus, BudgetOutcome, CONTROL_REQUEST_TYPE, CONTROL_RESPONSE_TYPE,
    ControlRequest, ControlRequestEnvelope, ControlResponse, ControlResponseEnvelope, EffortLevel,
    MemorySourceStatus, MessageRole, PermissionRequest, PermissionResolutionKind, ProviderId,
    SessionRecord, SessionStatus, SessionSummary, StreamErrorCategory, StreamEvent, TokenUsage,
    ToolUseCompletionKind, TranscriptBlock, TranscriptMessage, TurnCancellationKind, TurnContext,
};
use orbcode_tools::ToolRegistry;
pub use orbcode_tools::{
    BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, SkillDefinition, SkillSource,
    TaskListSnapshot, TaskListSummary, TaskStatusKind, TaskView, ToolOutcome, ToolSpec,
    provider_facing_tool_name, session_task_list_id, task_record_to_view,
};
pub use settings::{KeybindingsFile, ModelResolutionOverview, SandboxExcludedCommand};

#[derive(Clone)]
pub struct AppServer {
    sessions: SessionManager,
    auth: AuthManager,
    background: BackgroundManager,
    tools: ToolRegistry,
    mcp: McpRegistry,
    read_state: std::sync::Arc<orbcode_tools::FileReadState>,
    active_streams: protocol_handler::turns::ActiveStreams,
}

async fn plugin_mcp_sources(home_dir: &Path, cwd: &Path) -> Vec<McpPluginConfigSource> {
    let Ok(registry) = load_plugin_registry(home_dir, cwd).await else {
        return Vec::new();
    };

    plugin_mcp_config_sources(&registry)
        .into_iter()
        .map(map_plugin_mcp_source)
        .collect()
}

fn map_plugin_mcp_source(source: ConfigPluginMcpConfigSource) -> McpPluginConfigSource {
    McpPluginConfigSource {
        plugin_id: source.plugin_id,
        plugin_name: source.plugin_name,
        label: source.label,
        kind: match source.kind {
            ConfigPluginMcpConfigSourceKind::File(path) => McpPluginConfigSourceKind::File(path),
            ConfigPluginMcpConfigSourceKind::Inline(value) => {
                McpPluginConfigSourceKind::Inline(value)
            }
        },
    }
}

impl AppServer {
    pub async fn new(
        cwd: impl Into<PathBuf>,
        overrides: AppConfigOverrides,
    ) -> Result<Self, CoreError> {
        let cwd = cwd.into();
        let config = AppConfig::load(cwd.clone(), overrides).await?;
        let home_dir = config.home_dir.clone();
        let tools = ToolRegistry::foundation();
        let plugin_mcp_sources = plugin_mcp_sources(&home_dir, &cwd).await;
        let mcp = McpRegistry::load_with_options(
            home_dir,
            cwd,
            McpLoadOptions {
                config_inputs: config.mcp_config_inputs.clone(),
                env: config.settings.env.clone(),
                plugin_sources: plugin_mcp_sources,
            },
        )
        .await
        .map_err(CoreError::from)?;
        let policy = config.policy.clone();
        if policy.allowed_mcp_servers.is_some()
            || !policy.denied_mcp_servers.is_empty()
            || policy.allow_managed_mcp_servers_only
        {
            mcp.retain_policy_allowed(|server_id| policy.mcp_server_allowed(server_id))
                .await;
        }
        let forced_login_method = config
            .forced_login_method()
            .and_then(orbcode_config::parse_forced_login_method);
        let auth = AuthManager::new(config.home_dir.clone())
            .with_env_overrides(config.env_overrides.clone())
            .with_openai_proxy_config(config.outbound_proxy_config())
            .with_forced_login_method(forced_login_method);
        let mut sessions =
            SessionManager::new_with_auth(config, tools.clone(), mcp.clone(), auth.clone()).await?;
        sessions.refresh_agent_definitions().await;
        sessions.refresh_output_styles().await;
        let background = BackgroundManager::new(sessions.config().home_dir.clone());
        let _ = background_agent::reconcile_orphaned_agents(&sessions.config().home_dir).await;
        let read_state = Arc::new(orbcode_tools::FileReadState::with_persistence(
            sessions.config().home_dir.join("file-read-state.json"),
        ));

        Ok(Self {
            sessions,
            auth,
            background,
            tools,
            mcp,
            read_state,
            active_streams: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        })
    }

    /// Returns a handle to the active stream subscription map.
    ///
    /// Phase 3 will wire up event delivery via notifications; for now the
    /// receiver is stashed here so it is not dropped prematurely.
    pub(crate) fn active_streams(&self) -> protocol_handler::turns::ActiveStreams {
        Arc::clone(&self.active_streams)
    }
}
