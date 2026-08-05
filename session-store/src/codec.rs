use orbcode_protocol::{MessageRole, TokenUsage, TranscriptBlock, TranscriptMessage};
use serde_json::{Value, json};

use crate::entries::serialize_assistant_content;

pub fn normalize_tool_progress_record(tool_use_id: &str, progress: Value) -> Value {
    let mut progress = match progress {
        Value::Object(map) => Value::Object(map),
        other => json!({ "data": other }),
    };

    if let Some(object) = progress.as_object_mut() {
        object
            .entry("parentToolUseID".to_string())
            .or_insert_with(|| Value::String(tool_use_id.to_string()));
    }

    progress
}

pub fn effective_blocks(message: &TranscriptMessage) -> Vec<TranscriptBlock> {
    if message.blocks.is_empty() {
        if message.content.is_empty() {
            Vec::new()
        } else {
            vec![TranscriptBlock::Text {
                text: message.content.clone(),
            }]
        }
    } else {
        message.blocks.clone()
    }
}

pub fn assistant_message_has_visible_content(message: &TranscriptMessage) -> bool {
    effective_blocks(message)
        .into_iter()
        .any(|block| match block {
            TranscriptBlock::Text { text } | TranscriptBlock::Thinking { text, .. } => {
                !text.trim().is_empty()
            }
            TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. } => true,
            _ => false,
        })
}

pub fn deserialize_block_payload(input: &str) -> Value {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| Value::String(input.to_string()))
}

pub fn session_has_tool_result(messages: &[TranscriptMessage], tool_use_id: &str) -> bool {
    messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult {
                    tool_use_id: current_id,
                    ..
                } if current_id == tool_use_id
            )
        })
    })
}

pub fn tool_result_message(
    tool_use_id: &str,
    content: impl Into<String>,
    is_error: bool,
    metadata: Option<String>,
) -> TranscriptMessage {
    let content = content.into();
    TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: content.into(),
            is_error,
            metadata,
        }],
    )
}

pub fn agent_tool_result_metadata(
    prompt: &str,
    agent_type: Option<&str>,
    content: &str,
    total_tool_uses: u64,
    total_duration_ms: u64,
    usage: &TokenUsage,
) -> String {
    let mut metadata = json!({
        "status": "completed",
        "prompt": prompt,
        "content": [
            {
                "type": "text",
                "text": content,
            }
        ],
        "totalToolUseCount": total_tool_uses,
        "totalDurationMs": total_duration_ms,
        "totalTokens": usage.total_tokens,
    });
    if let Some(agent_type) = agent_type
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert(
            "agentType".to_string(),
            Value::String(agent_type.to_string()),
        );
    }
    metadata.to_string()
}

pub fn nested_tool_error_metadata(tool_name: &str, content: &str) -> String {
    json!({
        "status": "failed",
        "toolName": tool_name,
        "content": [
            {
                "type": "text",
                "text": content,
            }
        ],
    })
    .to_string()
}

pub fn initial_agent_progress_record(agent_id: &str, prompt: &str) -> Value {
    json!({
        "data": {
            "type": "agent_progress",
            "agentId": agent_id,
            "prompt": prompt,
            "message": {
                "type": "user",
                "message": {
                    "role": "user",
                    "content": prompt,
                }
            }
        }
    })
}

pub fn agent_tool_use_progress_record(agent_id: &str, id: &str, name: &str, input: &str) -> Value {
    let message = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: input.to_string(),
        }],
    );
    json!({
        "data": {
            "type": "agent_progress",
            "agentId": agent_id,
            "message": {
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": serialize_assistant_content(&message),
                }
            }
        }
    })
}

pub fn agent_tool_result_progress_record(
    agent_id: &str,
    tool_use_id: &str,
    content: &str,
    is_error: bool,
    metadata: &Value,
) -> Value {
    json!({
        "data": {
            "type": "agent_progress",
            "agentId": agent_id,
            "message": {
                "type": "user",
                "toolUseResult": metadata,
                "message": {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                            "is_error": is_error,
                        }
                    ]
                }
            }
        }
    })
}

pub fn attach_agent_id(progress: Value, agent_id: &str) -> Value {
    let mut progress = match progress {
        Value::Object(map) => Value::Object(map),
        other => json!({ "data": other }),
    };
    if let Some(data) = progress.get_mut("data").and_then(Value::as_object_mut) {
        data.entry("agentId".to_string())
            .or_insert_with(|| Value::String(agent_id.to_string()));
    }
    progress
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_message_builds_user_tool_result_block() {
        let message = tool_result_message(
            "tool-1",
            "result content",
            false,
            Some(r#"{"status":"completed"}"#.to_string()),
        );

        assert_eq!(message.role, MessageRole::User);
        assert_eq!(message.content, "");
        assert_eq!(
            message.blocks,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "result content".into(),
                is_error: false,
                metadata: Some(r#"{"status":"completed"}"#.to_string()),
            }]
        );
    }

    #[test]
    fn agent_tool_use_progress_record_serializes_only_the_tool_use_block() {
        let progress = agent_tool_use_progress_record(
            "agent-1",
            "file-read-1",
            "Read",
            r#"{"file_path":"/tmp/context.rs"}"#,
        );
        let content = progress["data"]["message"]["message"]["content"]
            .as_array()
            .expect("assistant content array");

        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"].as_str(), Some("tool_use"));
        assert_eq!(content[0]["id"].as_str(), Some("file-read-1"));
        assert_eq!(content[0]["name"].as_str(), Some("Read"));
        assert_eq!(
            content[0]["input"]["file_path"].as_str(),
            Some("/tmp/context.rs")
        );
    }
}
