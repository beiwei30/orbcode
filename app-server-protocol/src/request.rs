use std::path::PathBuf;

use orbcode_config::PermissionMode;
use orbcode_mcp::McpServerConfig;
use orbcode_protocol::EffortLevel;
use schemars::JsonSchema;

#[derive(
    Clone, Debug, Default, serde::Serialize, serde::Deserialize, JsonSchema, PartialEq, Eq,
)]
pub struct BootstrapParams {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub additional_directories: Vec<PathBuf>,
    #[serde(default)]
    pub session_mcp_servers: Vec<McpServerConfig>,
    /// When true, load the session record for read-only viewing only (e.g. a
    /// workflow agent step's output). This must NOT mutate the live session's
    /// runtime context (cwd/permissions/effort) or the live-session registry.
    #[serde(default)]
    pub read_only: bool,
}

/// Select a live session without relying on process-global runtime state.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SessionIdParams {
    pub session_id: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SetSessionPermissionModeParams {
    pub session_id: String,
    pub mode: PermissionMode,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SetSessionModelParams {
    pub session_id: String,
    /// `None` selects the provider's configured default.
    pub model: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SetSessionEffortParams {
    pub session_id: String,
    /// `None` disables the per-session thought/effort override.
    pub effort: Option<EffortLevel>,
}
