use orbcode_config::{
    AppConfig, PermissionRuleMatchMode, bash_command_allowed_by_rules, canonical_tool_name,
    parse_tool_rule_list, tool_path_allowed_by_additional_directory,
};
use orbcode_protocol::ProviderId;
use orbcode_tools::bash_input_requests_sandbox_escalation;
use std::path::PathBuf;

use crate::CoreError;

pub use orbcode_config::{
    PermissionRule, mcp_permission_target, normalize_permission_rule_for_edit,
    suggested_bash_permission_rules,
};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PermissionContext {
    pub cwd: PathBuf,
    pub allow_network: bool,
    pub provider_allow_network: bool,
    pub allow_tools: bool,
    pub allowed_rules: Vec<PermissionRule>,
    pub denied_rules: Vec<PermissionRule>,
    /// Rules that force an interactive prompt even when an allow rule matches or
    /// the blanket tools-permission is set. Precedence is deny > ask > allow.
    pub ask_rules: Vec<PermissionRule>,
    pub additional_directories: Vec<PathBuf>,
}

impl PermissionContext {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            cwd: config.cwd.clone(),
            allow_network: config.allow_network,
            provider_allow_network: config.provider_allow_network,
            allow_tools: config.allow_tools,
            allowed_rules: config
                .allowed_tools
                .iter()
                .flat_map(|rule| parse_tool_rule_list(std::slice::from_ref(rule)))
                .map(|rule| PermissionRule::parse(&rule))
                .collect(),
            denied_rules: config
                .disallowed_tools
                .iter()
                .flat_map(|rule| parse_tool_rule_list(std::slice::from_ref(rule)))
                .map(|rule| PermissionRule::parse(&rule))
                .collect(),
            ask_rules: config
                .ask_tools
                .iter()
                .flat_map(|rule| parse_tool_rule_list(std::slice::from_ref(rule)))
                .map(|rule| PermissionRule::parse(&rule))
                .collect(),
            additional_directories: config.additional_directories.clone(),
        }
    }

    pub fn ensure_provider_call_allowed(&self, provider: ProviderId) -> Result<(), CoreError> {
        if self.provider_allow_network {
            Ok(())
        } else {
            Err(CoreError::PermissionDenied(format!(
                "provider {provider} requires network access but ORBCODE_PROVIDER_NETWORK is disabled"
            )))
        }
    }

    pub fn allows_tool_request(
        &self,
        requires_tools_permission: bool,
        requires_network_permission: bool,
    ) -> bool {
        (!requires_tools_permission || self.allow_tools)
            && (!requires_network_permission || self.allow_network)
    }

    pub fn tool_visible(&self, tool_name: &str) -> bool {
        !self
            .denied_rules
            .iter()
            .any(|rule| rule.matches_tool_wide(tool_name))
    }

    pub fn tool_denied(&self, tool_name: &str, tool_input: &str) -> Option<&PermissionRule> {
        self.denied_rules.iter().find(|rule| {
            rule.matches_tool_call_with_mode(tool_name, tool_input, PermissionRuleMatchMode::Deny)
        })
    }

    pub fn tool_allowed(&self, tool_name: &str, tool_input: &str) -> Option<&PermissionRule> {
        if canonical_tool_name(tool_name) == "bash" {
            if bash_input_requests_sandbox_escalation(tool_input) {
                return None;
            }
            return bash_command_allowed_by_rules(&self.allowed_rules, tool_name, tool_input);
        }

        self.allowed_rules.iter().find(|rule| {
            rule.matches_tool_call_with_mode(tool_name, tool_input, PermissionRuleMatchMode::Allow)
        })
    }

    /// Whether a configured `ask` rule matches this call, forcing an
    /// interactive prompt even when an allow rule or the blanket
    /// tools-permission would otherwise auto-approve. Deny still wins over ask.
    pub fn tool_should_ask(&self, tool_name: &str, tool_input: &str) -> bool {
        self.ask_rules.iter().any(|rule| {
            rule.matches_tool_call_with_mode(tool_name, tool_input, PermissionRuleMatchMode::Allow)
        })
    }

    pub fn tool_allowed_without_prompt(&self, tool_name: &str, tool_input: &str) -> bool {
        self.tool_allowed(tool_name, tool_input).is_some()
            || self.tool_path_allowed_by_additional_directory(tool_name, tool_input)
    }

    fn tool_path_allowed_by_additional_directory(&self, tool_name: &str, tool_input: &str) -> bool {
        tool_path_allowed_by_additional_directory(
            &self.cwd,
            &self.additional_directories,
            tool_name,
            tool_input,
        )
    }

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
