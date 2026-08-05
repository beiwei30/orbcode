use std::path::PathBuf;

use orbcode_config::{
    ContextWindowOptions, EditorModeSetting, MaxOutputTokenOptions, ThemeSetting,
    TokenWarningOptions,
};
use orbcode_core::{ContextDiagnosticsReport, ContextUsageOverview, PermissionContext};
use orbcode_protocol::{
    EffortLevel, MemorySourceStatus, ProviderId, SessionRecord, StreamEvent, TurnContext,
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BootstrapState {
    pub session: SessionRecord,
    pub bootstrap_event: StreamEvent,
    pub prompt_history: Vec<String>,
    pub available_tool_count: usize,
    pub configured_mcp_server_count: usize,
    pub enabled_mcp_capability_count: usize,
    pub home_dir: PathBuf,
    pub cwd: PathBuf,
    pub model_display_name: String,
    pub context_window_options: ContextWindowOptions,
    pub max_output_token_options: MaxOutputTokenOptions,
    pub token_warning_options: TokenWarningOptions,
    pub theme: ThemeSetting,
    pub editor_mode: EditorModeSetting,
    pub default_provider: ProviderId,
    pub fallback_provider: Option<ProviderId>,
    pub max_retries: usize,
    pub permissions: PermissionContext,
    pub mcp_slash_suggestions: McpSlashSuggestionCatalog,
    pub statusline_command: Option<String>,
    pub statusline_refresh_interval_secs: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AcpLoadReplayPreflight {
    pub session: SessionRecord,
    pub replay_allowed: bool,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AcpDeleteSessionParams {
    pub session_id: String,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpSlashSuggestionCatalog {
    pub servers: Vec<McpServerSlashSuggestion>,
    pub tools: Vec<McpToolSlashSuggestion>,
    pub resources: Vec<McpResourceSlashSuggestion>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpServerSlashSuggestion {
    pub id: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpToolSlashSuggestion {
    pub server_id: String,
    pub name: String,
    pub provider_name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpResourceSlashSuggestion {
    pub server_id: String,
    pub uri: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PermissionOverview {
    pub permissions: PermissionContext,
    pub allow_all: bool,
    pub settings_allowed_rules: Vec<String>,
    pub settings_denied_rules: Vec<String>,
    pub startup_allowed_rules: Vec<String>,
    pub startup_denied_rules: Vec<String>,
    pub edited_allowed_rules: Vec<String>,
    pub edited_denied_rules: Vec<String>,
    pub runtime_allowed_rules: Vec<String>,
    pub runtime_denied_rules: Vec<String>,
    pub configured_additional_directories: Vec<PathBuf>,
    pub session_additional_directories: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddedDirectory {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddDirectoryCandidate {
    pub path: PathBuf,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ContextOverview {
    pub context: TurnContext,
    pub usage: ContextUsageOverview,
    pub report: ContextDiagnosticsReport,
    pub max_thinking_tokens: Option<u32>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StatusOverview {
    pub session_id: String,
    pub cwd: PathBuf,
    pub home_dir: PathBuf,
    pub model_display_name: String,
    pub model_name: String,
    pub model_capabilities: Vec<String>,
    pub small_fast_model_display_name: String,
    pub effort_level: Option<EffortLevel>,
    pub max_thinking_tokens: Option<u32>,
    pub default_provider: ProviderId,
    pub fallback_provider: Option<ProviderId>,
    pub max_retries: usize,
    pub sandbox_mode: String,
    pub sandbox_allow_network: bool,
    pub permissions: PermissionOverview,
    pub auth: orbcode_config::AuthOverview,
    pub persisted_session_count: usize,
    pub background_job_count: usize,
    pub available_tool_count: usize,
    pub configured_mcp_server_count: usize,
    pub enabled_mcp_capability_count: usize,
    pub policy: PolicyOverview,
}

/// Secret-free status projection for a configured MCP server.
///
/// Mutation-only configuration (endpoint, arguments, environment, headers and
/// auth values) is deliberately absent from this read DTO.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpServerOverview {
    pub id: String,
    pub transport: String,
    pub enabled: bool,
    pub status: String,
    pub trust: String,
    pub summary: String,
    pub auth_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelChangeResult {
    pub provider: ProviderId,
    pub model: String,
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThinkingBudgetResult {
    pub session_id: String,
    pub max_thinking_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SeedReadStateResult {
    pub session_id: String,
    pub path: PathBuf,
    pub mtime: u64,
    pub seeded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsyncCancellationResultKind {
    Signalled,
    AlreadyTerminal,
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CancelAsyncTaskResult {
    pub session_id: String,
    pub task_id: String,
    pub outcome: AsyncCancellationResultKind,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PolicyOverview {
    pub managed_origin: Option<String>,
    pub managed_paths: Vec<PathBuf>,
    pub available_models: Option<Vec<String>>,
    pub allowed_mcp_servers: Option<usize>,
    pub denied_mcp_servers: usize,
    pub allow_managed_hooks_only: bool,
    pub allow_managed_permission_rules_only: bool,
    pub allow_managed_mcp_servers_only: bool,
    pub disable_bypass_permissions_mode: bool,
    pub strict_plugin_only_customization: Option<String>,
    pub force_login_method: Option<String>,
    pub effective_model_source: Option<String>,
    pub conflicts: Vec<PolicyConflictOverview>,
    pub settings_sources: Vec<PolicySourceOverview>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PolicyConflictOverview {
    pub source: String,
    pub source_path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PolicySourceOverview {
    pub source: String,
    pub primary_path: PathBuf,
    pub present: bool,
    pub read_only: bool,
    pub error_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryFileOverview {
    pub label: String,
    pub path: PathBuf,
    pub exists: bool,
    pub content: Option<String>,
    pub status: MemorySourceStatus,
    pub writable: bool,
    pub trust_boundary: Option<String>,
    pub scope: Option<String>,
    pub skipped_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryOverview {
    pub user_memory: MemoryFileOverview,
    pub project_memories: Vec<MemoryFileOverview>,
    pub auto_memory_enabled: bool,
    pub auto_memory_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceDiff {
    pub cwd: PathBuf,
    pub status: String,
    pub staged_diff: String,
    pub unstaged_diff: String,
    pub untracked_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanOverview {
    pub plan_file: PathBuf,
    pub state_file: PathBuf,
    pub in_plan_mode: bool,
    pub plan: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

impl std::fmt::Display for DoctorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: DoctorStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn has_failures(&self) -> bool {
        self.checks
            .iter()
            .any(|check| matches!(check.status, DoctorStatus::Fail))
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        self.checks.iter().fold((0, 0, 0), |mut counts, check| {
            match check.status {
                DoctorStatus::Pass => counts.0 += 1,
                DoctorStatus::Warn => counts.1 += 1,
                DoctorStatus::Fail => counts.2 += 1,
            }
            counts
        })
    }
}

#[cfg(test)]
mod control_dto_tests {
    use super::*;

    #[test]
    fn mcp_status_projection_has_no_mutation_secret_fields() {
        let value = serde_json::to_value(McpServerOverview {
            id: "docs".to_string(),
            transport: "http".to_string(),
            enabled: true,
            status: "ready".to_string(),
            trust: "trusted".to_string(),
            summary: "Documentation".to_string(),
            auth_mode: "header".to_string(),
            error: None,
        })
        .expect("MCP status JSON");
        for forbidden in ["endpoint", "args", "env", "headers", "auth"] {
            assert!(
                value.get(forbidden).is_none(),
                "unexpected field {forbidden}"
            );
        }
    }

    #[test]
    fn async_cancellation_outcomes_round_trip() {
        for outcome in [
            AsyncCancellationResultKind::Signalled,
            AsyncCancellationResultKind::AlreadyTerminal,
            AsyncCancellationResultKind::NotFound,
        ] {
            let value = serde_json::to_value(outcome).expect("serialize");
            let parsed = serde_json::from_value(value).expect("deserialize");
            assert_eq!(outcome, parsed);
        }
    }
}
