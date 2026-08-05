use orbcode_protocol::ToolUseCompletionKind;
use orbcode_tools::{ToolError, ToolOutcome, ToolSpec, tool_error_result_metadata};
use tokio::task::JoinHandle;

use crate::{
    CoreError,
    agent_loop::tool_round::{ToolRoundExecutionOutcome, ToolRoundStreamResult},
    permissions::PermissionContext,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolUseOutcome {
    Continue,
    Denied,
    Cancelled,
}

impl ToolUseOutcome {
    pub(crate) fn into_tool_round_outcome(self) -> ToolRoundExecutionOutcome {
        match self {
            Self::Continue => ToolRoundExecutionOutcome::Continue,
            Self::Denied => ToolRoundExecutionOutcome::Denied,
            Self::Cancelled => ToolRoundExecutionOutcome::Cancelled,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToolLookupOutcome {
    Found(ToolSpec),
    UnknownHandled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolDenyPrecedenceStage {
    OriginalInput,
    PreToolInputUpdate,
}

impl ToolDenyPrecedenceStage {
    pub(crate) fn reason_suffix(self) -> &'static str {
        match self {
            Self::OriginalInput => "",
            Self::PreToolInputUpdate => " after PreToolUse input update",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolPermissionResolutionOutcome {
    Approved,
    Denied,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpTrustResolutionOutcome {
    /// Not an MCP tool, or server is already trusted — proceed.
    Proceed,
    /// Server was untrusted, client approved trust — proceed.
    Trusted,
    /// Server was denied by client — deny tool execution.
    Denied,
    /// Cancelled (turn interrupted) before decision arrived.
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolInvocationPermissions {
    pub(crate) allow_tools: bool,
    pub(crate) allow_network: bool,
}

impl ToolInvocationPermissions {
    pub(crate) fn after_explicit_allow(permissions: &PermissionContext, spec: &ToolSpec) -> Self {
        Self {
            allow_tools: permissions.allow_tools || spec.requires_tools_permission,
            allow_network: permissions.allow_network || spec.requires_network_permission,
        }
    }

    pub(crate) fn from_permission_context(
        permissions: &PermissionContext,
        spec: &ToolSpec,
    ) -> Self {
        Self {
            allow_tools: permissions.allow_tools || !spec.requires_tools_permission,
            allow_network: permissions.allow_network || !spec.requires_network_permission,
        }
    }
}

pub(crate) struct StreamedToolUseExecution {
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    handle: Option<JoinHandle<Result<BufferedToolUseCompletion, CoreError>>>,
    cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl StreamedToolUseExecution {
    #[cfg(test)]
    pub(crate) fn new(
        tool_use_id: String,
        tool_name: String,
        handle: JoinHandle<Result<BufferedToolUseCompletion, CoreError>>,
    ) -> Self {
        Self {
            tool_use_id,
            tool_name,
            handle: Some(handle),
            cancel_flag: None,
        }
    }

    pub(crate) fn new_cancellable(
        tool_use_id: String,
        tool_name: String,
        handle: JoinHandle<Result<BufferedToolUseCompletion, CoreError>>,
        cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            tool_use_id,
            tool_name,
            handle: Some(handle),
            cancel_flag: Some(cancel_flag),
        }
    }

    pub(crate) async fn finish(mut self) -> Result<BufferedToolUseCompletion, CoreError> {
        let handle = self
            .handle
            .take()
            .expect("streamed tool execution handle should be present");
        handle
            .await
            .map_err(|error| CoreError::Tool(format!("streamed tool execution failed: {error}")))?
    }

    pub(crate) async fn interrupt(mut self) {
        let handle = self
            .handle
            .take()
            .expect("streamed tool execution handle should be present");
        if let Some(cancel_flag) = &self.cancel_flag {
            cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = handle.await;
        } else {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for StreamedToolUseExecution {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

pub(crate) struct SessionProviderStreamResult {
    pub(crate) tool_round_stream: ToolRoundStreamResult,
    pub(crate) streamed_tool_executions: Vec<StreamedToolUseExecution>,
}

pub(crate) const EMPTY_READ_TOOL_RESULT: &str =
    "<system-reminder>Read returned empty output.</system-reminder>";

pub(crate) fn tool_result_content(outcome: &ToolOutcome) -> String {
    if is_exact_content_tool(&outcome.name) {
        if outcome.output.is_empty() {
            EMPTY_READ_TOOL_RESULT.to_string()
        } else {
            outcome.output.clone()
        }
    } else if outcome.output.trim().is_empty() {
        outcome.summary.clone()
    } else {
        outcome.output.clone()
    }
}

fn is_exact_content_tool(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "read" | "file-read")
}

pub(crate) const INTERRUPTED_TOOL_RESULT: &str = "Interrupted by user";

pub(crate) struct ToolErrorResultDetails {
    pub(crate) content: String,
    pub(crate) metadata: Option<String>,
    pub(crate) completion_kind: ToolUseCompletionKind,
}

pub(crate) fn tool_error_result_details(
    tool_name: &str,
    error: &ToolError,
) -> ToolErrorResultDetails {
    let metadata = tool_error_result_metadata(tool_name, error);
    if error.is_cancelled() {
        ToolErrorResultDetails {
            content: error.to_string(),
            metadata,
            completion_kind: ToolUseCompletionKind::Cancelled,
        }
    } else if error.is_interrupted() {
        ToolErrorResultDetails {
            content: INTERRUPTED_TOOL_RESULT.to_string(),
            metadata,
            completion_kind: ToolUseCompletionKind::Interrupted,
        }
    } else {
        ToolErrorResultDetails {
            content: error.to_string(),
            metadata,
            completion_kind: ToolUseCompletionKind::ExecutionFailed,
        }
    }
}

#[derive(Debug)]
pub(crate) struct BufferedToolUseCompletion {
    pub(crate) outcome: ToolUseOutcome,
    pub(crate) result: BufferedToolResult,
}

#[derive(Debug)]
pub(crate) struct BufferedToolResult {
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) content: String,
    pub(crate) is_error: bool,
    pub(crate) metadata: Option<String>,
    pub(crate) completion_kind: ToolUseCompletionKind,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn tool_result_content_prefers_output_when_non_empty() {
        let outcome = ToolOutcome {
            name: "bash".to_string(),
            summary: "summary".to_string(),
            output: "actual output".to_string(),
            metadata: None,
            changed_paths: Vec::<PathBuf>::new(),
        };

        assert_eq!(tool_result_content(&outcome), "actual output");
    }

    #[test]
    fn tool_result_content_falls_back_to_summary_for_blank_output() {
        let outcome = ToolOutcome {
            name: "bash".to_string(),
            summary: "summary".to_string(),
            output: " \n\t".to_string(),
            metadata: None,
            changed_paths: Vec::<PathBuf>::new(),
        };

        assert_eq!(tool_result_content(&outcome), "summary");
    }

    #[test]
    fn tool_result_content_marks_empty_file_read_output() {
        let outcome = ToolOutcome {
            name: "file-read".to_string(),
            summary: "Read /tmp/empty.txt.".to_string(),
            output: String::new(),
            metadata: None,
            changed_paths: Vec::<PathBuf>::new(),
        };

        assert_eq!(tool_result_content(&outcome), EMPTY_READ_TOOL_RESULT);

        let whitespace = ToolOutcome {
            name: "Read".to_string(),
            summary: "Read /tmp/blank.txt.".to_string(),
            output: " \n".to_string(),
            metadata: None,
            changed_paths: Vec::<PathBuf>::new(),
        };

        assert_eq!(tool_result_content(&whitespace), " \n");
    }

    #[test]
    fn tool_error_result_details_marks_interrupted_errors() {
        let details = tool_error_result_details(
            "bash",
            &ToolError::InterruptedWithMetadata {
                metadata: json!({"exitCode": 130}),
            },
        );

        assert_eq!(details.content, INTERRUPTED_TOOL_RESULT);
        assert_eq!(details.completion_kind, ToolUseCompletionKind::Interrupted);
        let metadata: Value =
            serde_json::from_str(details.metadata.as_deref().expect("metadata should exist"))
                .expect("metadata should parse");
        assert_eq!(metadata["exitCode"], json!(130));
        assert_eq!(metadata["status"], json!("interrupted"));
        assert_eq!(metadata["toolName"], json!("bash"));
    }

    #[test]
    fn tool_error_result_details_marks_execution_failures() {
        let details = tool_error_result_details(
            "bash",
            &ToolError::ExecutionFailedWithMetadata {
                message: "exit 1".to_string(),
                metadata: json!({"exitCode": 1}),
            },
        );

        assert_eq!(details.content, "tool execution failed: exit 1");
        assert_eq!(
            details.completion_kind,
            ToolUseCompletionKind::ExecutionFailed
        );
        let metadata: Value =
            serde_json::from_str(details.metadata.as_deref().expect("metadata should exist"))
                .expect("metadata should parse");
        assert_eq!(metadata["exitCode"], json!(1));
        assert_eq!(metadata["status"], json!("failed"));
        assert_eq!(metadata["toolName"], json!("bash"));
    }
}
