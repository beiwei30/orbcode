use orbcode_protocol::{MessageRole, ProviderToolDefinition, TranscriptBlock, TranscriptMessage};
use serde_json::{Value, json};

use crate::ProviderRequest;

use super::anthropic::anthropic_user_context_message;
use super::{apply_extra_body, truncate_tool_result_for_provider};

pub fn build_openai_request_body(request: &ProviderRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "stream": true,
        "stream_options": {
            "include_usage": true,
        },
        "messages": openai_messages(request),
    });

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(request.tools.iter().map(openai_tool).collect());
        body["tool_choice"] = Value::String("auto".to_string());
    }

    if let Some(max_tokens) = request.options.max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = request.options.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(effort) = request.effort.filter(|_| !request.disable_thinking) {
        body["reasoning_effort"] = json!(openai_reasoning_effort(effort));
    }
    apply_extra_body(&mut body, &request.options.extra_body);

    body
}

fn openai_reasoning_effort(effort: orbcode_protocol::EffortLevel) -> &'static str {
    match effort {
        orbcode_protocol::EffortLevel::Low => "low",
        orbcode_protocol::EffortLevel::Medium => "medium",
        _ => "high",
    }
}

fn openai_messages(request: &ProviderRequest) -> Vec<Value> {
    let mut messages = Vec::new();
    if !request.system_prompt.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": request.system_prompt,
        }));
    }
    if let Some(user_context) = openai_user_context_message(&request.context) {
        messages.push(user_context);
    }
    for message in &request.messages {
        messages.extend(openai_message(message));
    }
    if messages
        .iter()
        .all(|message| message.get("role").and_then(Value::as_str) != Some("user"))
    {
        messages.push(json!({
            "role": "user",
            "content": request.prompt,
        }));
    }
    messages
}

fn openai_user_context_message(context: &orbcode_protocol::TurnContext) -> Option<Value> {
    let anthropic = anthropic_user_context_message(context)?;
    let text = anthropic
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)?;
    Some(json!({
        "role": "user",
        "content": text,
    }))
}

pub(super) fn openai_message(message: &TranscriptMessage) -> Vec<Value> {
    match message.role {
        MessageRole::System => {
            if message.content.trim().is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "role": "system",
                    "content": message.content,
                })]
            }
        }
        MessageRole::User => openai_user_message(message),
        MessageRole::Assistant => openai_assistant_message(message),
        _ => Vec::new(),
    }
}

fn openai_user_message(message: &TranscriptMessage) -> Vec<Value> {
    let mut text_parts = Vec::new();
    let mut tool_messages = Vec::new();

    if message.blocks.is_empty() {
        if !message.content.trim().is_empty() {
            text_parts.push(message.content.clone());
        }
    } else {
        for block in &message.blocks {
            match block {
                TranscriptBlock::Text { text } => text_parts.push(text.clone()),
                TranscriptBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    tool_messages.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": truncate_tool_result_for_provider(content),
                    }));
                }
                _ => {}
            }
        }
    }

    if !text_parts.is_empty() {
        tool_messages.push(json!({
            "role": "user",
            "content": text_parts.join("\n"),
        }));
    }
    tool_messages
}

fn openai_assistant_message(message: &TranscriptMessage) -> Vec<Value> {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    if message.blocks.is_empty() {
        if !message.content.trim().is_empty() {
            text_parts.push(message.content.clone());
        }
    } else {
        for block in &message.blocks {
            match block {
                TranscriptBlock::Text { text } => text_parts.push(text.clone()),
                TranscriptBlock::ToolUse { id, name, input } => {
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": compact_json_string(input),
                        }
                    }));
                }
                _ => {}
            }
        }
    }

    if text_parts.is_empty() && tool_calls.is_empty() {
        return Vec::new();
    }

    let mut message = json!({
        "role": "assistant",
        "content": if text_parts.is_empty() {
            Value::Null
        } else {
            Value::String(text_parts.join("\n"))
        },
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    vec![message]
}

fn compact_json_string(input: &str) -> String {
    serde_json::from_str::<Value>(input)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_else(|_| input.to_string())
}

fn openai_tool(tool: &ProviderToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": sanitize_openai_json_schema(tool.input_schema.clone()),
        }
    })
}

fn sanitize_openai_json_schema(schema: Value) -> Value {
    match schema {
        Value::Object(mut object) => {
            if let Some(value) = object.remove("const") {
                object.insert("enum".to_string(), Value::Array(vec![value]));
            }
            for value in object.values_mut() {
                match value {
                    Value::Object(_) => {
                        let nested = std::mem::take(value);
                        *value = sanitize_openai_json_schema(nested);
                    }
                    Value::Array(items) => {
                        for item in items {
                            let nested = std::mem::take(item);
                            *item = sanitize_openai_json_schema(nested);
                        }
                    }
                    _ => {}
                }
            }
            Value::Object(object)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(sanitize_openai_json_schema)
                .collect::<Vec<_>>(),
        ),
        value => value,
    }
}
