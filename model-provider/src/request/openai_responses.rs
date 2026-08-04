//! OpenAI Responses request mapping used by ChatGPT/Codex subscription auth.

use orbcode_protocol::{MessageRole, ProviderToolDefinition, TranscriptBlock, TranscriptMessage};
use serde_json::{Value, json};

use crate::ProviderRequest;

use super::anthropic::anthropic_user_context_message;
use super::openai::{compact_json_string, openai_reasoning_effort, sanitize_openai_json_schema};
use super::truncate_tool_result_for_provider;

pub fn build_openai_responses_request_body(request: &ProviderRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "instructions": request.system_prompt,
        "input": responses_input(request),
        "tools": request.tools.iter().map(responses_tool).collect::<Vec<_>>(),
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
    });

    if let Some(effort) = request.effort.filter(|_| !request.disable_thinking) {
        body["reasoning"] = json!({
            "effort": openai_reasoning_effort(effort),
            "summary": "auto",
        });
    }
    if let Some(max_tokens) = request.options.max_output_tokens {
        body["max_output_tokens"] = json!(max_tokens);
    }
    body
}

fn responses_input(request: &ProviderRequest) -> Vec<Value> {
    let mut input = Vec::new();
    if let Some(context) = responses_user_context(&request.context) {
        input.push(responses_message("user", "input_text", context));
    }
    for message in &request.messages {
        input.extend(responses_message_items(message));
    }
    if input.iter().all(|item| {
        item.get("role").and_then(Value::as_str) != Some("user")
            && item.get("type").and_then(Value::as_str) != Some("function_call_output")
    }) {
        input.push(responses_message(
            "user",
            "input_text",
            request.prompt.clone(),
        ));
    }
    input
}

fn responses_user_context(context: &orbcode_protocol::TurnContext) -> Option<String> {
    let anthropic = anthropic_user_context_message(context)?;
    anthropic
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn responses_message_items(message: &TranscriptMessage) -> Vec<Value> {
    match message.role {
        MessageRole::System => {
            if message.content.trim().is_empty() {
                Vec::new()
            } else {
                vec![responses_message(
                    "developer",
                    "input_text",
                    message.content.clone(),
                )]
            }
        }
        MessageRole::User => responses_user_items(message),
        MessageRole::Assistant => responses_assistant_items(message),
        _ => Vec::new(),
    }
}

fn responses_user_items(message: &TranscriptMessage) -> Vec<Value> {
    let mut items = Vec::new();
    let mut text = Vec::new();
    if message.blocks.is_empty() {
        if !message.content.trim().is_empty() {
            text.push(message.content.clone());
        }
    } else {
        for block in &message.blocks {
            match block {
                TranscriptBlock::Text { text: value } => text.push(value.clone()),
                TranscriptBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => items.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": truncate_tool_result_for_provider(content),
                })),
                _ => {}
            }
        }
    }
    if !text.is_empty() {
        items.push(responses_message("user", "input_text", text.join("\n")));
    }
    items
}

fn responses_assistant_items(message: &TranscriptMessage) -> Vec<Value> {
    let mut items = Vec::new();
    let mut text = Vec::new();
    if message.blocks.is_empty() {
        if !message.content.trim().is_empty() {
            text.push(message.content.clone());
        }
    } else {
        for block in &message.blocks {
            match block {
                TranscriptBlock::Text { text: value } => text.push(value.clone()),
                TranscriptBlock::Thinking { text, signature } => {
                    if let Some(encrypted_content) =
                        signature.as_deref().filter(|value| !value.is_empty())
                    {
                        let summary = if text.is_empty() {
                            Vec::new()
                        } else {
                            vec![json!({ "type": "summary_text", "text": text })]
                        };
                        items.push(json!({
                            "type": "reasoning",
                            "summary": summary,
                            "encrypted_content": encrypted_content,
                        }));
                    }
                }
                TranscriptBlock::ToolUse { id, name, input } => items.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": compact_json_string(input),
                })),
                _ => {}
            }
        }
    }
    if !text.is_empty() {
        items.push(responses_message(
            "assistant",
            "output_text",
            text.join("\n"),
        ));
    }
    items
}

fn responses_message(role: &str, content_type: &str, text: String) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{
            "type": content_type,
            "text": text,
        }],
    })
}

fn responses_tool(tool: &ProviderToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": sanitize_openai_json_schema(tool.input_schema.clone()),
        "strict": false,
    })
}

#[cfg(test)]
mod tests {
    use orbcode_protocol::{EffortLevel, ProviderToolDefinition, TurnContext};

    use super::*;
    use crate::{OpenAiWireMode, ProviderRequestOptions};

    fn request(messages: Vec<TranscriptMessage>) -> ProviderRequest {
        ProviderRequest {
            session_id: "session".to_string(),
            prompt: "follow up".to_string(),
            context: TurnContext::default(),
            messages,
            system_prompt: "be useful".to_string(),
            tools: vec![ProviderToolDefinition {
                name: "Read".to_string(),
                description: "read a file".to_string(),
                input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            }],
            model: "gpt-5.6-sol".to_string(),
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: Some(EffortLevel::Low),
            options: ProviderRequestOptions {
                openai_wire_mode: OpenAiWireMode::Responses,
                ..ProviderRequestOptions::default()
            },
        }
    }

    #[test]
    fn maps_tools_reasoning_and_tool_outputs() {
        let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::Thinking {
                        text: "summary".to_string(),
                        signature: Some("encrypted".to_string()),
                    },
                    TranscriptBlock::ToolUse {
                        id: "call-1".to_string(),
                        name: "Read".to_string(),
                        input: "{\"path\":\"a\"}".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "contents".to_string(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];
        let body = build_openai_responses_request_body(&request(messages));
        assert_eq!(body["store"], false);
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["tools"][0]["name"], "Read");
        let input = body["input"].as_array().expect("input");
        assert!(input.iter().any(|item| item["type"] == "reasoning" && item["encrypted_content"] == "encrypted"));
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call" && item["call_id"] == "call-1")
        );
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call_output" && item["call_id"] == "call-1")
        );
    }
}
