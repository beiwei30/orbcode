use orbcode_config::{
    AppConfig, PermissionRuleMatchMode, ToolPathBoundary, bash_command_allowed_by_rules,
    canonical_tool_name, parse_tool_rule_list, tool_path_allowed_by_additional_directory,
    tool_path_boundary,
};
use orbcode_protocol::{ApprovalPolicy, ApprovalReviewer, ModelPermissionPolicy, ProviderId};
use orbcode_tools::{ToolCapability, ToolSpec, bash_input_requests_sandbox_escalation};
use std::path::PathBuf;

use crate::CoreError;

pub use orbcode_config::{
    PermissionRule, mcp_permission_target, normalize_permission_rule_for_edit,
    suggested_bash_permission_rules,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PermissionBoundaryReason {
    Network,
    OutsideWorkspace { targets: Vec<PathBuf> },
    InvalidPath,
    SandboxEscalation,
    ExternalSideEffect,
    LegacyToolPermission,
    ExplicitAskRule,
    ExplicitHookAsk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PermissionGrantSource {
    Internal,
    WorkspaceBoundary,
    ConfiguredRule,
    RememberedRule,
    Hook,
    FullAccess,
    LegacyAmbient,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PermissionEvaluation {
    Allow { source: PermissionGrantSource },
    AskUser { reason: PermissionBoundaryReason },
    AutoReview { reason: PermissionBoundaryReason },
    Deny { reason: String },
}

impl PermissionEvaluation {
    pub(crate) fn is_explicit_allow(&self) -> bool {
        matches!(
            self,
            Self::Allow {
                source: PermissionGrantSource::ConfiguredRule
                    | PermissionGrantSource::RememberedRule
                    | PermissionGrantSource::Hook
                    | PermissionGrantSource::FullAccess
            }
        )
    }
}

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

    /// Whether executing this particular call requires leaving the configured
    /// filesystem or command sandbox. An allow rule can suppress approval
    /// without automatically granting this stronger execution capability.
    pub(crate) fn requires_sandbox_boundary_override(
        &self,
        spec: &ToolSpec,
        tool_name: &str,
        tool_input: &str,
    ) -> bool {
        match spec.capability {
            ToolCapability::WorkspaceRead | ToolCapability::WorkspaceWrite => matches!(
                tool_path_boundary(
                    &self.cwd,
                    &self.additional_directories,
                    tool_name,
                    tool_input,
                ),
                ToolPathBoundary::OutsideAllowedRoots { .. }
            ),
            ToolCapability::SandboxedCommand => bash_input_requests_sandbox_escalation(tool_input),
            ToolCapability::Internal
            | ToolCapability::Network
            | ToolCapability::ExternalSideEffect => false,
        }
    }

    pub(crate) fn evaluate_tool_call(
        &self,
        policy: Option<ModelPermissionPolicy>,
        spec: &ToolSpec,
        tool_name: &str,
        tool_input: &str,
        hook_allowed: bool,
        remembered_allowed: bool,
    ) -> PermissionEvaluation {
        if let Some(rule) = self.tool_denied(tool_name, tool_input) {
            return PermissionEvaluation::Deny {
                reason: format!("permission denied by configured rule `{}`", rule.raw),
            };
        }
        if hook_allowed {
            return PermissionEvaluation::Allow {
                source: PermissionGrantSource::Hook,
            };
        }

        // An explicit ask remains a user decision even under Full Access or
        // Approve for me. This lets advanced settings retain a hard prompt.
        if self.tool_should_ask(tool_name, tool_input) {
            return PermissionEvaluation::AskUser {
                reason: PermissionBoundaryReason::ExplicitAskRule,
            };
        }
        if self.tool_allowed_without_prompt(tool_name, tool_input) {
            return PermissionEvaluation::Allow {
                source: PermissionGrantSource::ConfiguredRule,
            };
        }
        if remembered_allowed {
            return PermissionEvaluation::Allow {
                source: PermissionGrantSource::RememberedRule,
            };
        }

        let Some(policy) = policy else {
            if self.allows_tool_request(
                spec.requires_tools_permission,
                spec.requires_network_permission,
            ) {
                return PermissionEvaluation::Allow {
                    source: PermissionGrantSource::LegacyAmbient,
                };
            }
            return PermissionEvaluation::AskUser {
                reason: PermissionBoundaryReason::LegacyToolPermission,
            };
        };

        let boundary = match spec.capability {
            ToolCapability::Internal => {
                return PermissionEvaluation::Allow {
                    source: PermissionGrantSource::Internal,
                };
            }
            ToolCapability::WorkspaceRead | ToolCapability::WorkspaceWrite => {
                match tool_path_boundary(
                    &self.cwd,
                    &self.additional_directories,
                    tool_name,
                    tool_input,
                ) {
                    ToolPathBoundary::InsideAllowedRoots => {
                        return PermissionEvaluation::Allow {
                            source: PermissionGrantSource::WorkspaceBoundary,
                        };
                    }
                    ToolPathBoundary::OutsideAllowedRoots { targets } => {
                        PermissionBoundaryReason::OutsideWorkspace { targets }
                    }
                    ToolPathBoundary::InvalidInput | ToolPathBoundary::NotPathAware => {
                        PermissionBoundaryReason::InvalidPath
                    }
                }
            }
            ToolCapability::SandboxedCommand => {
                if bash_input_requests_sandbox_escalation(tool_input) {
                    PermissionBoundaryReason::SandboxEscalation
                } else {
                    return PermissionEvaluation::Allow {
                        source: PermissionGrantSource::WorkspaceBoundary,
                    };
                }
            }
            ToolCapability::Network => PermissionBoundaryReason::Network,
            ToolCapability::ExternalSideEffect => PermissionBoundaryReason::ExternalSideEffect,
        };

        if policy.approval_policy == ApprovalPolicy::Never {
            return PermissionEvaluation::Allow {
                source: PermissionGrantSource::FullAccess,
            };
        }
        match policy.approval_reviewer {
            ApprovalReviewer::User => PermissionEvaluation::AskUser { reason: boundary },
            ApprovalReviewer::AutoReview => PermissionEvaluation::AutoReview { reason: boundary },
        }
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
