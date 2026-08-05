use std::path::PathBuf;

use schemars::JsonSchema;

use crate::McpServerInput;

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
    pub session_mcp_servers: Vec<McpServerInput>,
    /// When true, load the session record for read-only viewing only (e.g. a
    /// workflow agent step's output). This must NOT mutate the live session's
    /// runtime context (cwd/permissions/effort) or the live-session registry.
    #[serde(default)]
    pub read_only: bool,
}
