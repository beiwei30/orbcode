use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::SandboxMode;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolUseCompletionKind {
    Success,
    ExecutionFailed,
    PermissionDenied,
    Interrupted,
    Cancelled,
    UnknownTool,
}

impl ToolUseCompletionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::ExecutionFailed => "execution_failed",
            Self::PermissionDenied => "permission_denied",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::UnknownTool => "unknown_tool",
        }
    }
}

/// Standardized, cross-tool result metadata.
///
/// Every built-in tool fills the subset of fields relevant to it so the
/// transcript, TUI cards, and headless output read one shape instead of a
/// per-tool bag of keys. Serializes to camelCase; legacy field names are
/// accepted as deserialize aliases so transcripts written before the schema
/// was unified still load.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMetadata {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "duration",
        alias = "durationMillis"
    )]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<OutputTruncation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ToolArtifact>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "fileChange",
        alias = "workspaceImpact"
    )]
    pub file_changes: Option<FileChangeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermissionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

impl ToolResultMetadata {
    pub fn is_empty(&self) -> bool {
        self.duration_ms.is_none()
            && self.truncation.is_none()
            && self.artifacts.is_empty()
            && self.file_changes.is_none()
            && self.permissions.is_none()
            && self.sandbox.is_none()
            && self.diagnostics.is_empty()
    }

    /// Serialize to a JSON object. Returns an empty object on the unreachable
    /// serialization-failure path so callers can merge unconditionally.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| Value::Object(Default::default()))
    }
}

/// Output truncation accounting shared by every tool that caps result size.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputTruncation {
    pub truncated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "outputChars",
        alias = "originalChars"
    )]
    pub original_chars: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "omittedChars"
    )]
    pub omitted_chars: Option<u64>,
}

/// A side artifact produced by a tool (e.g. a spilled large-output file).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolArtifact {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "sizeBytes")]
    pub bytes: Option<u64>,
}

/// Files touched by a tool and any git working-tree impact.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeSummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "changedPaths")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<Value>,
}

/// Tool/network permission state at the time the tool ran.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSummary {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "toolsAllowed"
    )]
    pub tools_allowed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "networkAllowed"
    )]
    pub network_allowed: Option<bool>,
}

/// Sandbox configuration the tool executed under.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SandboxMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "networkAllowed"
    )]
    pub network_allowed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "sandboxEscalated"
    )]
    pub escalated: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::SandboxMode;

    #[test]
    fn tool_result_metadata_round_trips_camel_case() {
        let metadata = ToolResultMetadata {
            duration_ms: Some(1200),
            truncation: Some(OutputTruncation {
                truncated: true,
                original_chars: Some(5000),
                omitted_chars: Some(200),
            }),
            artifacts: vec![ToolArtifact {
                kind: "large_output".to_string(),
                path: Some("/tmp/out.txt".to_string()),
                description: None,
                bytes: Some(4096),
            }],
            file_changes: Some(FileChangeSummary {
                paths: vec!["src/main.rs".to_string()],
                operation: Some("modified".to_string()),
                git: None,
            }),
            permissions: Some(PermissionSummary {
                tools_allowed: Some(true),
                network_allowed: Some(false),
            }),
            sandbox: Some(SandboxSummary {
                mode: Some(SandboxMode::WorkspaceWrite),
                network_allowed: Some(false),
                escalated: Some(true),
            }),
            diagnostics: vec!["fell back to walkdir".to_string()],
        };

        let value = metadata.to_value();
        assert_eq!(value["durationMs"], serde_json::json!(1200));
        assert_eq!(value["truncation"]["omittedChars"], serde_json::json!(200));
        assert_eq!(
            value["artifacts"][0]["kind"],
            serde_json::json!("large_output")
        );
        assert_eq!(
            value["fileChanges"]["operation"],
            serde_json::json!("modified")
        );
        assert_eq!(
            value["permissions"]["toolsAllowed"],
            serde_json::json!(true)
        );
        assert_eq!(
            value["sandbox"]["mode"],
            serde_json::json!("workspace-write")
        );
        assert_eq!(value["sandbox"]["escalated"], serde_json::json!(true));
        assert_eq!(
            value["diagnostics"][0],
            serde_json::json!("fell back to walkdir")
        );

        let restored: ToolResultMetadata =
            serde_json::from_value(value).expect("round-trips through json");
        assert_eq!(restored, metadata);
    }

    #[test]
    fn tool_result_metadata_skips_empty_fields() {
        let value = ToolResultMetadata::default().to_value();
        assert_eq!(value, serde_json::json!({}));
        assert!(ToolResultMetadata::default().is_empty());
    }

    #[test]
    fn tool_result_metadata_accepts_legacy_field_aliases() {
        let legacy = serde_json::json!({
            "duration": 900,
            "truncation": { "truncated": true, "outputChars": 4000, "omittedChars": 50 },
            "artifacts": [{ "kind": "large_output", "sizeBytes": 2048 }],
            "workspaceImpact": { "git": { "postBranch": "main" } },
            "permissions": { "toolsAllowed": true, "networkAllowed": true },
            "sandbox": { "mode": "read-only", "sandboxEscalated": false },
        });

        let metadata: ToolResultMetadata =
            serde_json::from_value(legacy).expect("legacy metadata deserializes");

        assert_eq!(metadata.duration_ms, Some(900));
        let truncation = metadata.truncation.expect("truncation");
        assert_eq!(truncation.original_chars, Some(4000));
        assert_eq!(truncation.omitted_chars, Some(50));
        assert_eq!(metadata.artifacts[0].bytes, Some(2048));
        let file_changes = metadata
            .file_changes
            .expect("file changes from workspaceImpact");
        assert_eq!(
            file_changes.git.expect("git")["postBranch"],
            serde_json::json!("main")
        );
        assert_eq!(
            metadata.permissions.expect("permissions").network_allowed,
            Some(true)
        );
        let sandbox = metadata.sandbox.expect("sandbox");
        assert_eq!(sandbox.mode, Some(SandboxMode::ReadOnly));
        assert_eq!(sandbox.escalated, Some(false));
    }
}
