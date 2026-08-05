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

/// Set or clear the per-session thinking-token override.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SetThinkingBudgetParams {
    pub session_id: String,
    pub max_thinking_tokens: Option<u32>,
}

/// Seed one validated file identity into the shared stale-write guard.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SeedReadStateParams {
    pub session_id: String,
    pub path: String,
    pub mtime: u64,
}

/// Cancel one background task owned by a session.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CancelAsyncTaskParams {
    pub session_id: String,
    pub task_id: String,
}
