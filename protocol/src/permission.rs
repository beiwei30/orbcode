use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{SandboxMode, session::SessionId};

/// The three user-facing permission presets. These values describe a complete
/// runtime policy; they are not aliases for the legacy `PermissionMode` wire
/// values or for configured allow/ask/deny rules.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelPermissionPreset {
    AskForApproval,
    ApproveForMe,
    FullAccess,
}

impl ModelPermissionPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AskForApproval => "ask_for_approval",
            Self::ApproveForMe => "approve_for_me",
            Self::FullAccess => "full_access",
        }
    }

    /// Project this user-facing choice onto the orthogonal runtime controls.
    pub fn policy(self) -> ModelPermissionPolicy {
        match self {
            Self::AskForApproval => ModelPermissionPolicy {
                preset: self,
                sandbox_mode: SandboxMode::WorkspaceWrite,
                sandbox_allow_network: false,
                approval_policy: ApprovalPolicy::OnBoundaryRequest,
                approval_reviewer: ApprovalReviewer::User,
            },
            Self::ApproveForMe => ModelPermissionPolicy {
                preset: self,
                sandbox_mode: SandboxMode::WorkspaceWrite,
                sandbox_allow_network: false,
                approval_policy: ApprovalPolicy::OnBoundaryRequest,
                approval_reviewer: ApprovalReviewer::AutoReview,
            },
            Self::FullAccess => ModelPermissionPolicy {
                preset: self,
                sandbox_mode: SandboxMode::DangerFullAccess,
                sandbox_allow_network: true,
                approval_policy: ApprovalPolicy::Never,
                approval_reviewer: ApprovalReviewer::User,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalReviewer {
    User,
    AutoReview,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    OnBoundaryRequest,
    Never,
}

/// Effective runtime controls derived atomically from one permission preset.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ModelPermissionPolicy {
    pub preset: ModelPermissionPreset,
    pub sandbox_mode: SandboxMode,
    pub sandbox_allow_network: bool,
    pub approval_policy: ApprovalPolicy,
    pub approval_reviewer: ApprovalReviewer,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PermissionRequest {
    pub request_id: String,
    pub session_id: SessionId,
    pub tool_use_id: String,
    pub tool_name: String,
    pub tool_input: String,
    pub requires_tools_permission: bool,
    pub requires_network_permission: bool,
}

impl PermissionRequest {
    pub fn summary(&self) -> String {
        let mut needs = Vec::new();
        if self.requires_tools_permission {
            needs.push("tools");
        }
        if self.requires_network_permission {
            needs.push("network");
        }
        if needs.is_empty() {
            format!("{} {}", self.tool_name, self.tool_use_id)
        } else {
            format!(
                "{} {} ({})",
                self.tool_name,
                self.tool_use_id,
                needs.join(", ")
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PermissionResolutionKind {
    Approved,
    Denied,
    Interrupted,
}

impl PermissionResolutionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApprovalReviewResolutionKind {
    Approved,
    EscalatedToUser,
    Failed,
    TimedOut,
    Cancelled,
    LimitExceeded,
}

impl ApprovalReviewResolutionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::EscalatedToUser => "escalated_to_user",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::LimitExceeded => "limit_exceeded",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpTrustApprovalRequest {
    pub request_id: String,
    pub session_id: SessionId,
    pub server_id: String,
    pub tool_name: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpTrustResolutionKind {
    Trusted,
    Denied,
    Interrupted,
}

impl McpTrustResolutionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Denied => "denied",
            Self::Interrupted => "interrupted",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_presets_project_to_atomic_runtime_policies() {
        let ask = ModelPermissionPreset::AskForApproval.policy();
        let auto = ModelPermissionPreset::ApproveForMe.policy();
        let full = ModelPermissionPreset::FullAccess.policy();

        assert_eq!(ask.sandbox_mode, SandboxMode::WorkspaceWrite);
        assert!(!ask.sandbox_allow_network);
        assert_eq!(ask.approval_reviewer, ApprovalReviewer::User);
        assert_eq!(auto.sandbox_mode, ask.sandbox_mode);
        assert_eq!(auto.sandbox_allow_network, ask.sandbox_allow_network);
        assert_eq!(auto.approval_reviewer, ApprovalReviewer::AutoReview);
        assert_eq!(full.sandbox_mode, SandboxMode::DangerFullAccess);
        assert!(full.sandbox_allow_network);
        assert_eq!(full.approval_policy, ApprovalPolicy::Never);
    }

    #[test]
    fn permission_preset_wire_values_are_stable() {
        for (preset, wire) in [
            (
                ModelPermissionPreset::AskForApproval,
                "\"ask_for_approval\"",
            ),
            (ModelPermissionPreset::ApproveForMe, "\"approve_for_me\""),
            (ModelPermissionPreset::FullAccess, "\"full_access\""),
        ] {
            assert_eq!(serde_json::to_string(&preset).expect("serialize"), wire);
            assert_eq!(
                serde_json::from_str::<ModelPermissionPreset>(wire).expect("deserialize"),
                preset
            );
        }
    }
}
