mod envelope;
mod error;
mod initialize;
pub mod method;
mod notification;
mod request;
mod response;
mod server_request;

pub use envelope::{
    ClientMessage, ClientRequestEnvelope, RequestId, ResponseResult, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse,
    ServerResponseEnvelope,
};
pub use error::{ErrorCode, ProtocolError};
pub use initialize::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, ServerCapabilities,
    ServerInfo,
};
pub use notification::StreamEventNotification;
pub use request::BootstrapParams;
pub use response::{
    AcpDeleteSessionParams, AcpLoadReplayPreflight, AddDirectoryCandidate, AddedDirectory,
    BootstrapState, ContextOverview, DoctorCheck, DoctorReport, DoctorStatus,
    McpResourceSlashSuggestion, McpServerSlashSuggestion, McpSlashSuggestionCatalog,
    McpToolSlashSuggestion, MemoryFileOverview, MemoryOverview, PermissionOverview, PlanOverview,
    PolicyConflictOverview, PolicyOverview, PolicySourceOverview, StatusOverview, WorkspaceDiff,
};
pub use server_request::{
    AskUserQuestionRequest, AskUserQuestionResponse, McpTrustDecisionWire, McpTrustResponseParams,
    PermissionDecisionWire, PermissionResponseParams,
};

// Re-export protocol types used by consumers of this crate.
pub use orbcode_config::{
    AgentDefinition, AgentLoadWarning, AuthOverview, AuthStatusEntry, HookDiscovery,
    SandboxLocalSettings, SandboxSettingsUpdate,
};
pub use orbcode_core::{
    BillingBasis, CompactDecision, CompactSessionResult, ContextDiagnosticsReport,
    ContextTokenSource, ContextUsageOverview, CostOverview, PermissionContext, PermissionDecision,
    ProviderRequestDebugSnapshot, StatsActivityDay, StatsOverview, UsageOverview, WorkflowCommand,
    WorkflowSource, format_cost,
};
pub use orbcode_mcp::{
    McpAuth, McpOAuthOverview, McpOAuthStatusEntry, McpPromptResult, McpServerConfig,
    McpServerStatus, McpServerTrust, McpTransport,
};
pub use orbcode_protocol::{McpTrustApprovalRequest, PermissionRequest, StreamEvent};
pub use orbcode_tools::SkillDefinition;
