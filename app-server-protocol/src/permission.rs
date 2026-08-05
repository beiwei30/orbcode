use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Protocol-owned permission mode used by session-scoped client controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    #[serde(alias = "accept-edits")]
    AcceptEdits,
    #[serde(alias = "bypass-permissions")]
    BypassPermissions,
    #[serde(alias = "dont-ask")]
    DontAsk,
    Plan,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRuleOverview {
    pub raw: String,
    pub tool_name: String,
    pub rule_content: Option<String>,
}

/// Data-only view of the effective permission context.
///
/// Permission evaluation and structured bash/path matching remain internal to
/// core and config; this DTO exposes only the values clients render.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionContext {
    pub cwd: PathBuf,
    pub allow_network: bool,
    pub provider_allow_network: bool,
    pub allow_tools: bool,
    pub allowed_rules: Vec<PermissionRuleOverview>,
    pub denied_rules: Vec<PermissionRuleOverview>,
    pub ask_rules: Vec<PermissionRuleOverview>,
    pub additional_directories: Vec<PathBuf>,
}

impl PermissionContext {
    pub fn describe(&self) -> String {
        format!(
            "network={} provider_network={} tools={} allow_rules={} deny_rules={} additional_dirs={}",
            self.allow_network,
            self.provider_allow_network,
            self.allow_tools,
            self.allowed_rules.len(),
            self.denied_rules.len(),
            self.additional_directories.len()
        )
    }
}

/// Client-side permission choice used by interactive adapters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    Approve,
    ApproveAlways(String),
    ApproveAlwaysMany(Vec<String>),
    Deny,
}
