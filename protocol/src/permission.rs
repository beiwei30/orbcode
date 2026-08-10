use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::SessionId;

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
