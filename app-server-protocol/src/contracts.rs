//! Named method contracts for the stable app-server surface.
//!
//! Open-ended JSON is allowed only in fields explicitly documented as opaque;
//! no stable method uses an unnamed `Value` result.

use std::path::PathBuf;

use orbcode_protocol::{
    BackgroundTaskView, EffortLevel, ProviderId, SessionRecord, SessionSummary, WorkflowCommand,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentDefinition, AgentLoadWarning, AuthMethod, PermissionMode, SkillDefinition};

macro_rules! impl_list_result {
    ($name:ident, $item:ty) => {
        impl $name {
            pub fn into_inner(self) -> Vec<$item> {
                self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = [$item];

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl IntoIterator for $name {
            type Item = $item;
            type IntoIter = std::vec::IntoIter<$item>;

            fn into_iter(self) -> Self::IntoIter {
                self.0.into_iter()
            }
        }
    };
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EmptyParams {}

/// Successful method result whose response envelope intentionally has no data payload.
///
/// Transports normalize an omitted `data` field to JSON `null`, which is the
/// serde representation of this unit struct.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct NoData;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionIdParams {
    pub session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SetSessionPermissionModeParams {
    pub session_id: String,
    pub mode: PermissionMode,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SetSessionModelParams {
    pub session_id: String,
    /// `None` selects the provider's configured default.
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SetSessionEffortParams {
    pub session_id: String,
    /// `None` disables the per-session thought/effort override.
    #[schemars(with = "Option<String>")]
    pub effort: Option<EffortLevel>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionRenameParams {
    pub session_id: String,
    pub new_title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionForkParams {
    pub session_id: String,
    pub title: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct SessionListResult(pub Vec<SessionSummary>);

impl_list_result!(SessionListResult, SessionSummary);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct SessionForkResult(pub SessionRecord);

impl std::ops::Deref for SessionForkResult {
    type Target = SessionRecord;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SessionForkResult {
    pub fn into_inner(self) -> SessionRecord {
        self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionRewindParams {
    pub session_id: String,
    pub keep_messages: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionRecordMessageParams {
    pub session_id: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionFindByTitleParams {
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionFindByTitleResult {
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TurnSubmitParams {
    pub session_id: String,
    pub prompt: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TurnSubmitResult {
    pub subscription_id: String,
}

pub type TurnSteerParams = TurnSubmitParams;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TurnCancelResult {
    pub cancelled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TurnInterruptResult {
    pub interrupted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SentResult {
    pub sent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PermissionModeResult {
    pub mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PermissionSetModeParams {
    pub mode: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleKind {
    Allow,
    Deny,
}

impl PermissionRuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PermissionRuleParams {
    pub kind: String,
    pub rule: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionPermissionRuleParams {
    pub session_id: String,
    pub kind: String,
    pub rule: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PermissionRuleUpdateResult {
    pub path: PathBuf,
    pub rule: String,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AddDirectoryParams {
    pub session_id: String,
    pub directory: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ValidateDirectoryParams {
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ModelNameResult {
    pub model_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ModelOptionOverview {
    pub value: Option<String>,
    pub label: String,
    pub description: String,
    pub current: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct ModelOptionsResult(pub Vec<ModelOptionOverview>);

impl_list_result!(ModelOptionsResult, ModelOptionOverview);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SetModelParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SetModelResult {
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProviderResolutionOverview {
    pub provider: String,
    pub model: String,
    pub request_model: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProvidersResult {
    pub default_provider: String,
    pub fallback_provider: Option<String>,
    pub max_retries: usize,
    pub resolutions: Vec<ProviderResolutionOverview>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ThemeParams {
    pub theme: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ThemeResult {
    pub theme: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EffortParams {
    pub session_id: String,
    #[schemars(with = "Option<String>")]
    pub effort: Option<EffortLevel>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EffortResult {
    #[schemars(with = "Option<String>")]
    pub effort: Option<EffortLevel>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OutputStyleParams {
    pub style: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OutputStyleResult {
    pub style: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PathResult {
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct KeybindingsFileResult {
    pub path: PathBuf,
    pub created: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct KeybindingsLoadResult {
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EditorModeParams {
    pub mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EditorModeResult {
    pub editor_mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OutputStyleOptionOverview {
    pub value: String,
    pub label: String,
    pub description: String,
    pub current: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct OutputStyleOptionsResult(pub Vec<OutputStyleOptionOverview>);

impl_list_result!(OutputStyleOptionsResult, OutputStyleOptionOverview);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ActiveOutputStyleResult {
    pub name: String,
    pub matched: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SettingKeyParams {
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SettingLockedResult {
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EnabledParams {
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EnabledResult {
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StringPathParams {
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxExcludedCommandParams {
    pub command: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxExcludedCommandResult {
    pub pattern: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AllowAllParams {
    pub allow_all: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AllowAllResult {
    pub allow_all: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ToolOverview {
    pub name: String,
    pub summary: String,
    pub requires_tools_permission: bool,
    pub requires_network_permission: bool,
    pub provider_hidden: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct ToolsListResult(pub Vec<ToolOverview>);

impl_list_result!(ToolsListResult, ToolOverview);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ToolInvokeParams {
    pub name: String,
    #[serde(default = "default_empty_object_string")]
    pub input: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ToolInvokeResult {
    pub name: String,
    pub summary: String,
    pub output: String,
    /// Tool-defined metadata. Its schema is intentionally opaque.
    pub metadata: Option<Value>,
    pub changed_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SkillDefinitionsParams {
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct SkillDefinitionsResult(pub Vec<SkillDefinition>);

impl_list_result!(SkillDefinitionsResult, SkillDefinition);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AgentSummary {
    pub agent_type: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct AgentDefinitionsResult(pub Vec<AgentSummary>);

impl_list_result!(AgentDefinitionsResult, AgentSummary);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AgentDefinitionsWithWarningsResult {
    pub definitions: Vec<AgentDefinition>,
    pub warnings: Vec<AgentLoadWarning>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TaskListParams {
    pub task_list_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TaskOverview {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TaskListResult {
    pub task_list_id: String,
    pub directory: PathBuf,
    pub tasks: Vec<TaskOverview>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EnterPlanModeResult {
    pub name: String,
    pub summary: String,
    pub output: String,
    /// Tool-defined metadata. Its schema is intentionally opaque.
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundCreateParams {
    pub session_id: String,
    pub prompt: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundCreateResult {
    pub job_id: String,
    pub session_id: String,
    pub status: String,
    pub log_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundJobParams {
    pub job_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct BackgroundTaskListResult(pub Vec<BackgroundTaskView>);

impl_list_result!(BackgroundTaskListResult, BackgroundTaskView);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundCancelResult {
    pub job_id: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundLogResult {
    pub log: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundEventsResult {
    pub events: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundSubscribeParams {
    pub task_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BackgroundSubscribeResult {
    pub subscription_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct WorkflowListResult(pub Vec<WorkflowCommand>);

impl_list_result!(WorkflowListResult, WorkflowCommand);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorkflowStartParams {
    pub session_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WorkflowStartDynamicParams {
    pub session_id: String,
    pub name: String,
    /// Dynamic workflow definitions are extension-owned and intentionally opaque.
    pub spec: Value,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorkflowResumeParams {
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorkflowTaskResult {
    pub task_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AuthLoginParams {
    #[schemars(with = "String")]
    pub provider: ProviderId,
    pub method: AuthMethod,
    /// Secret-bearing mutation input. Never returned by the server.
    pub token: Option<String>,
    pub env_var: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AuthLogoutParams {
    #[schemars(with = "Option<String>")]
    pub provider: Option<ProviderId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AuthLogoutResult {
    pub removed: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AuthLoginResult {
    #[schemars(with = "String")]
    pub provider: ProviderId,
    pub method: String,
    pub source_summary: String,
    pub persisted: bool,
    pub usable: bool,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DiagnosticsCleanupChildSessionsParams {
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default)]
    pub stale_running_cutoff_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
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

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AdvancedCapabilityOverview {
    pub name: String,
    pub summary: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct AdvancedCapabilitiesResult(pub Vec<AdvancedCapabilityOverview>);

impl_list_result!(AdvancedCapabilitiesResult, AdvancedCapabilityOverview);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProviderRequestDebugOverview {
    pub provider: String,
    pub source: String,
    pub session_id: String,
    pub model: String,
    pub base_url: String,
    pub captured_at: String,
    pub recent_activity_json: String,
    pub previous_turn_json: String,
    pub body_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct LastProviderRequestResult(pub Option<ProviderRequestDebugOverview>);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PreUserInstructionsResult {
    pub preview: String,
}

fn default_empty_object_string() -> String {
    "{}".to_string()
}

fn default_true() -> bool {
    true
}
