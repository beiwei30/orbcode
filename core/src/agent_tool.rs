use orbcode_model_provider::ProviderResponse;
use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use orbcode_session_store::nested_tool_error_metadata;
use orbcode_tools::{ToolError, ToolOutcome, tool_error_result_metadata, tool_result_metadata};

use crate::tool_flow::tool_result_content;

pub(crate) fn agent_provider_response_message(response: ProviderResponse) -> TranscriptMessage {
    let message = if response.blocks.is_empty() {
        TranscriptMessage::new(MessageRole::Assistant, response.content)
    } else {
        TranscriptMessage::from_blocks(MessageRole::Assistant, response.blocks)
    };
    message
        .with_stop_reason(
            response
                .stop_reason
                .unwrap_or_else(|| "end_turn".to_string()),
        )
        .with_usage(response.usage)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentNestedToolUse {
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) tool_input: String,
}

pub(crate) fn agent_nested_tool_uses(message: &TranscriptMessage) -> Vec<AgentNestedToolUse> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolUse { id, name, input } => Some(AgentNestedToolUse {
                tool_use_id: id.clone(),
                tool_name: name.clone(),
                tool_input: input.clone(),
            }),
            _ => None,
        })
        .collect()
}

pub(crate) fn agent_final_text(description: &str, assistant_content: &str) -> String {
    if assistant_content.trim().is_empty() {
        format!("{description} completed.")
    } else {
        assistant_content.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentNestedToolResult {
    pub(crate) content: String,
    pub(crate) is_error: bool,
    pub(crate) metadata: String,
}

pub(crate) fn agent_nested_tool_success_result(outcome: &ToolOutcome) -> AgentNestedToolResult {
    AgentNestedToolResult {
        content: tool_result_content(outcome),
        is_error: false,
        metadata: tool_result_metadata(outcome),
    }
}

pub(crate) fn agent_nested_tool_error_result(
    tool_name: &str,
    error: &ToolError,
) -> AgentNestedToolResult {
    let content = error.to_string();
    let metadata = tool_error_result_metadata(tool_name, error)
        .unwrap_or_else(|| nested_tool_error_metadata(tool_name, &content));
    AgentNestedToolResult {
        content,
        is_error: true,
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::{ProviderId, TokenUsage, TranscriptBlock};
    use serde_json::Value;

    #[test]
    fn agent_provider_response_message_preserves_blocks_and_defaults_stop_reason() {
        let usage = TokenUsage {
            input_tokens: 3,
            output_tokens: 5,
            ..TokenUsage::default()
        };
        let message = agent_provider_response_message(ProviderResponse {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            content: "ignored when blocks exist".to_string(),
            blocks: vec![TranscriptBlock::Text {
                text: "agent answer".to_string(),
            }],
            stop_reason: None,
            usage: usage.clone(),
            deltas: Vec::new(),
        });

        assert_eq!(message.role, MessageRole::Assistant);
        assert_eq!(message.content, "agent answer");
        assert_eq!(message.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(message.usage, Some(usage));
    }

    #[test]
    fn agent_nested_tool_uses_collects_tool_blocks_only() {
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Text {
                    text: "checking".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "bash".to_string(),
                    input: r#"{"command":"pwd"}"#.to_string(),
                },
            ],
        );

        assert_eq!(
            agent_nested_tool_uses(&message),
            vec![AgentNestedToolUse {
                tool_use_id: "tool-1".to_string(),
                tool_name: "bash".to_string(),
                tool_input: r#"{"command":"pwd"}"#.to_string(),
            }]
        );
    }

    #[test]
    fn agent_final_text_falls_back_to_description_for_blank_content() {
        assert_eq!(
            agent_final_text("Explore repo", " \n\t"),
            "Explore repo completed."
        );
        assert_eq!(agent_final_text("Explore repo", "done"), "done");
    }

    #[test]
    fn agent_nested_tool_success_result_uses_summary_for_blank_output() {
        let result = agent_nested_tool_success_result(&ToolOutcome {
            name: "bash".to_string(),
            summary: "ran command".to_string(),
            output: " \n".to_string(),
            metadata: None,
            changed_paths: Vec::new(),
        });
        let metadata = serde_json::from_str::<Value>(&result.metadata).expect("parse metadata");

        assert_eq!(result.content, "ran command");
        assert!(!result.is_error);
        assert_eq!(
            metadata.get("status").and_then(Value::as_str),
            Some("completed")
        );
        assert_eq!(
            metadata.get("toolName").and_then(Value::as_str),
            Some("bash")
        );
    }

    #[test]
    fn agent_nested_tool_error_result_builds_fallback_metadata() {
        let result = agent_nested_tool_error_result(
            "bash",
            &ToolError::ExecutionFailed("exit 1".to_string()),
        );
        let metadata = serde_json::from_str::<Value>(&result.metadata).expect("parse metadata");

        assert!(result.content.contains("exit 1"));
        assert!(result.is_error);
        assert_eq!(
            metadata.get("status").and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            metadata.get("toolName").and_then(Value::as_str),
            Some("bash")
        );
    }
}
