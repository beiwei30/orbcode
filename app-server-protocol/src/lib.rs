mod auth;
mod contracts;
mod envelope;
mod error;
mod extensions;
mod initialize;
mod mcp;
pub mod method;
mod notification;
mod permission;
mod request;
mod response;
mod server_request;
mod settings;

pub use auth::{
    AuthMethod, AuthOverview, AuthStatusEntry, StatusAuthOverview, StatusAuthStatusEntry,
};
pub use contracts::*;

pub use envelope::{
    ClientMessage, ClientRequestEnvelope, RequestId, ResponseResult, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse,
    ServerResponseEnvelope,
};
pub use error::{ErrorCode, ProtocolError};
pub use extensions::{
    AgentDefinition, AgentHookCommand, AgentHookMatcher, AgentLoadWarning, AgentPermissionMode,
    AgentSource, AgentWarningKind, DiscoveredHook, HookDiscovery, HookDiscoveryWarning, HookLayer,
    HookProvenance, HookValidationStatus, SkillDefinition, SkillSource,
};
pub use initialize::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult,
    InteractiveQuestionsCapability, ServerCapabilities, ServerInfo,
};
pub use mcp::*;
pub use notification::StreamEventNotification;
pub use permission::{
    ApprovalPolicy, ApprovalReviewer, EffectivePermissionRules, ModelPermissionPolicy,
    ModelPermissionPreset, PermissionContext, PermissionDecision, PermissionMode,
    PermissionPresetOption, PermissionRuleEffect, PermissionRuleGroup, PermissionRuleOverview,
    SourcedPermissionRuleGroup,
};
pub use request::{
    BootstrapParams, CancelAsyncTaskParams, SeedReadStateParams, SetThinkingBudgetParams,
};
pub use response::{
    AcpDeleteSessionParams, AcpLoadReplayPreflight, AddDirectoryCandidate, AddedDirectory,
    AsyncCancellationResultKind, BootstrapState, CancelAsyncTaskResult, ContextOverview,
    DoctorCheck, DoctorReport, DoctorStatus, McpResourceSlashSuggestion, McpServerSlashSuggestion,
    McpServerStatusOverview, McpSlashSuggestionCatalog, McpStatusResult, McpToolSlashSuggestion,
    MemoryFileOverview, MemoryOverview, PermissionOverview, PermissionPresetsResult, PlanOverview,
    PolicyConflictOverview, PolicyOverview, PolicySourceOverview, SeedReadStateResult,
    SessionControlState, SessionModelOption, StatusOverview, ThinkingBudgetResult, WorkspaceDiff,
};
pub use server_request::{
    AskUserQuestionRequest, AskUserQuestionResponse, McpTrustDecisionWire, McpTrustResponseParams,
    PermissionDecisionWire, PermissionResponseParams,
};
pub use settings::{
    ClientPreferences, ContextWindowOptions, EditorModeSetting, EffectiveModelSelection,
    MaxOutputTokenOptions, ModelSelectionSource, PersistedModelSetting, ProviderModelSelection,
    RuntimeModelOverride, SandboxFilesystemLocalSettings, SandboxLocalSettings,
    SandboxNetworkLocalSettings, SandboxSettingsUpdate, SettingSource, StatuslineConfig,
    ThemeSetting, TokenWarningOptions,
};

// Re-export protocol types used by consumers of this crate.
pub use orbcode_protocol::{
    AskUserAnswerValue, AskUserCancellationReason, AskUserOption, AskUserQuestionSpec,
    AskUserResponseOutcome, AskUserValidationCode, AskUserValidationError, BillingBasis,
    CompactDecision, CompactSessionResult, ContextDiagnosticsReport, ContextTokenSource,
    ContextUsageOverview, CostOverview, EffortLevel, McpTrustApprovalRequest, PermissionRequest,
    ProviderRequestDebugSnapshot, SessionGoal, SessionGoalStatus, StatsActivityDay, StatsOverview,
    StreamEvent, TurnContext, UsageOverview, WorkflowCommand, WorkflowSource,
};

pub fn format_cost(cost: f64) -> String {
    if cost > 0.5 {
        format!("${:.2}", (cost * 100.0).round() / 100.0)
    } else {
        format!("${cost:.4}")
    }
}
