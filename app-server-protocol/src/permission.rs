use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use orbcode_protocol::{
    ApprovalPolicy, ApprovalReviewer, ModelPermissionPolicy, ModelPermissionPreset,
};

/// Protocol-owned permission mode used by session-scoped client controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    #[serde(alias = "bypass-permissions")]
    BypassPermissions,
    Plan,
    Auto,
}

impl PermissionMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "default" => Some(Self::Default),
            "bypassPermissions" | "bypass-permissions" => Some(Self::BypassPermissions),
            "plan" => Some(Self::Plan),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::BypassPermissions => "bypassPermissions",
            Self::Plan => "plan",
            Self::Auto => "auto",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRuleOverview {
    pub raw: String,
    pub tool_name: String,
    pub rule_content: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleEffect {
    Deny,
    Ask,
    Allow,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRuleGroup {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
}

impl PermissionRuleGroup {
    pub fn rules(&self, kind: crate::PermissionRuleKind) -> &[String] {
        match kind {
            crate::PermissionRuleKind::Allow => &self.allow,
            crate::PermissionRuleKind::Deny => &self.deny,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourcedPermissionRuleGroup {
    pub source: crate::SettingSource,
    pub active: bool,
    pub mutable: bool,
    pub rules: PermissionRuleGroup,
}

/// Source-preserving permission projection. Matching remains owned by config's
/// structured parser; this DTO only describes effective rule provenance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectivePermissionRules {
    pub managed: PermissionRuleGroup,
    pub settings: Vec<SourcedPermissionRuleGroup>,
    pub startup: PermissionRuleGroup,
    pub session: PermissionRuleGroup,
    pub runtime_added: PermissionRuleGroup,
    pub remembered: PermissionRuleGroup,
    pub settings_locked: bool,
    pub allow_managed_permission_rules_only: bool,
    pub precedence: Vec<PermissionRuleEffect>,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionPresetOption {
    pub value: ModelPermissionPreset,
    pub label: String,
    pub description: String,
    pub current: bool,
    pub disabled_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::PermissionMode;

    #[test]
    fn removed_permission_modes_are_rejected() {
        for value in ["acceptEdits", "accept-edits", "dontAsk", "dont-ask"] {
            assert_eq!(PermissionMode::parse(value), None);
            assert!(serde_json::from_str::<PermissionMode>(&format!("\"{value}\"")).is_err());
        }
    }
}
