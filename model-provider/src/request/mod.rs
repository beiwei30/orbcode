pub(crate) mod anthropic;
mod openai;
mod openai_responses;

use orbcode_protocol::{MessageRole, ProviderId, TranscriptBlock, TranscriptMessage};
use serde_json::Value;

use crate::{ProviderRequest, ProviderRequestDebugSnapshot, ProviderResponse};

pub use anthropic::{
    build_anthropic_count_tokens_request_body, build_anthropic_request_body,
    build_bedrock_count_tokens_request_body, strip_search_extra_tools_fields,
};
pub use openai::build_openai_request_body;
pub use openai_responses::build_openai_responses_request_body;

use anthropic::{anthropic_message, anthropic_messages, anthropic_user_context_message};
use openai::openai_message;

const MAX_PROVIDER_TOOL_RESULT_CHARS: usize = 100_000;
const MAX_PROVIDER_TRUNCATED_BASH_TOOL_RESULT_CHARS: usize = 8_000;
const BASH_TRANSCRIPT_TRUNCATION_MARKER: &str = "Bash output truncated for transcript safety.";

pub fn provider_request_debug_snapshot(
    provider: ProviderId,
    source: impl Into<String>,
    request: &ProviderRequest,
    captured_at: impl Into<String>,
) -> ProviderRequestDebugSnapshot {
    let previous_turn_messages = previous_turn_messages(request);
    let previous_turn = provider_visible_messages_value(provider, &previous_turn_messages);
    let body = match provider {
        ProviderId::OpenAi => match request.options.openai_wire_mode {
            crate::OpenAiWireMode::ChatCompletions => build_openai_request_body(request),
            crate::OpenAiWireMode::Responses => build_openai_responses_request_body(request),
        },
        ProviderId::Anthropic | ProviderId::Gemini | ProviderId::Grok => {
            build_anthropic_request_body(request)
        }
        _ => build_anthropic_request_body(request),
    };
    ProviderRequestDebugSnapshot {
        provider,
        source: source.into(),
        session_id: request.session_id.clone(),
        model: request.model.clone(),
        base_url: request.base_url.clone(),
        captured_at: captured_at.into(),
        recent_activity_json: "[]".to_string(),
        previous_turn_json: serde_json::to_string_pretty(&previous_turn)
            .unwrap_or_else(|_| previous_turn.to_string()),
        body_json: serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
    }
}

pub fn provider_visible_messages_value(
    provider: ProviderId,
    messages: &[TranscriptMessage],
) -> Value {
    match provider {
        ProviderId::OpenAi => Value::Array(messages.iter().flat_map(openai_message).collect()),
        ProviderId::Anthropic | ProviderId::Gemini | ProviderId::Grok => {
            Value::Array(messages.iter().filter_map(anthropic_message).collect())
        }
        _ => Value::Array(messages.iter().filter_map(anthropic_message).collect()),
    }
}

fn previous_turn_messages(request: &ProviderRequest) -> Vec<TranscriptMessage> {
    let mut messages = request.messages.clone();
    if let Some(index) = messages
        .iter()
        .rposition(|message| is_user_prompt_message(message, &request.prompt))
    {
        messages.remove(index);
    }
    messages
}

fn is_user_prompt_message(message: &TranscriptMessage, prompt: &str) -> bool {
    message.role == MessageRole::User && message.content.trim() == prompt.trim()
}

pub fn render_pre_user_instructions(request: &ProviderRequest) -> String {
    let mut sections = Vec::new();

    if !request.system_prompt.trim().is_empty() {
        sections.push(format!(
            "# System prompt\n{}",
            request.system_prompt.trim_end()
        ));
    }

    if let Some(context_message) = anthropic_user_context_message(&request.context)
        && let Some(context_text) = context_message
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| content.first())
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)
    {
        sections.push(format!("# Context message\n{}", context_text.trim_end()));
    }

    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                format!(
                    "## {}\nDescription:\n{}\n\nInput schema:\n{}",
                    tool.name,
                    tool.description,
                    serde_json::to_string_pretty(&tool.input_schema)
                        .unwrap_or_else(|_| tool.input_schema.to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        sections.push(format!("# Tools\n{tools}"));
    }

    sections.join("\n\n")
}

pub fn debug_request_summary(request: &ProviderRequest) -> String {
    let messages = anthropic_messages(&request.messages, &request.prompt);
    let mut lines = vec![format!(
        "[debug:provider-request] provider={} model={} messages={} tools={}",
        ProviderId::Anthropic,
        request.model,
        messages.len(),
        request.tools.len()
    )];

    for (index, message) in messages.iter().enumerate() {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let content = message
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let block_types = content
            .iter()
            .filter_map(|block| block.get("type").and_then(Value::as_str))
            .collect::<Vec<_>>();
        let preview = content
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .or_else(|| block.get("thinking"))
                    .and_then(Value::as_str)
            })
            .find(|text| !text.trim().is_empty())
            .map_or_else(|| "".to_string(), |text| preview_text(text, 120));
        lines.push(format!(
            "  m{index} role={role} block_types=[{}] preview={preview}",
            block_types.join(",")
        ));
    }

    if !request.tools.is_empty() {
        lines.push(format!(
            "  tools=[{}]",
            request
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    lines.join("\n")
}

pub fn debug_response_summary(response: &ProviderResponse) -> String {
    let block_types = response
        .blocks
        .iter()
        .map(|block| match block {
            TranscriptBlock::Text { .. } => "text",
            TranscriptBlock::Thinking { .. } => "thinking",
            TranscriptBlock::ToolUse { .. } => "tool_use",
            TranscriptBlock::ToolResult { .. } => "tool_result",
            _ => "unknown",
        })
        .collect::<Vec<_>>();
    let text_preview = preview_text(&response.content, 160);
    format!(
        "[debug:provider-response] provider={} stop_reason={} blocks=[{}] deltas={} preview={}",
        response.provider,
        response.stop_reason.as_deref().unwrap_or("null"),
        block_types.join(","),
        response.deltas.len(),
        text_preview
    )
}

fn apply_extra_body(body: &mut Value, extra: &serde_json::Map<String, Value>) {
    if extra.is_empty() {
        return;
    }
    if let Value::Object(map) = body {
        for (key, value) in extra {
            map.insert(key.clone(), value.clone());
        }
    }
}

fn truncate_tool_result_for_provider(content: &str) -> String {
    let total_chars = content.chars().count();
    let max_chars = if content.contains(BASH_TRANSCRIPT_TRUNCATION_MARKER) {
        MAX_PROVIDER_TRUNCATED_BASH_TOOL_RESULT_CHARS
    } else {
        MAX_PROVIDER_TOOL_RESULT_CHARS
    };
    if total_chars <= max_chars {
        return content.to_string();
    }

    let note = "Orb Code truncated an oversized earlier tool result before sending this request to stay under the provider request-size limit. Re-run the tool with a narrower scope if you need the omitted portion.";
    let reserve = note.chars().count() + 64;
    let preview_len = max_chars.saturating_sub(reserve).max(1);
    let tail_chars = (preview_len / 4).max(1);
    let head_chars = preview_len.saturating_sub(tail_chars).max(1);
    let (head, tail, omitted) = split_preview_on_line_boundaries(content, head_chars, tail_chars);
    format!("{head}\n\n[{note} Omitted {omitted} middle characters.]\n\n{tail}")
}

fn split_preview_on_line_boundaries(
    content: &str,
    head_chars: usize,
    tail_chars: usize,
) -> (String, String, usize) {
    let chars = content.chars().collect::<Vec<_>>();
    let total_chars = chars.len();
    let initial_head_end = head_chars.min(total_chars);
    let head_end = if initial_head_end >= total_chars
        || chars.get(initial_head_end.saturating_sub(1)) == Some(&'\n')
    {
        initial_head_end
    } else {
        chars[..initial_head_end]
            .iter()
            .rposition(|ch| *ch == '\n')
            .map_or(initial_head_end, |index| index + 1)
    };
    let initial_tail_start = total_chars.saturating_sub(tail_chars);
    let tail_start = if initial_tail_start == 0
        || chars.get(initial_tail_start.saturating_sub(1)) == Some(&'\n')
    {
        initial_tail_start
    } else {
        chars[initial_tail_start..]
            .iter()
            .position(|ch| *ch == '\n')
            .map_or(initial_tail_start, |index| initial_tail_start + index + 1)
    };
    let head = chars[..head_end].iter().collect::<String>();
    let tail = chars[tail_start..].iter().collect::<String>();
    let omitted = tail_start.saturating_sub(head_end);
    (head, tail, omitted)
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let preview = compact.chars().take(max_chars).collect::<String>();
    format!("{preview}…")
}

fn deserialize_block_payload(input: &str) -> Value {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| Value::String(input.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use serde_json::json;

    use crate::ProviderRequestOptions;
    use orbcode_protocol::{
        MessageRole, ProviderToolDefinition, TranscriptBlock, TranscriptMessage,
    };

    use super::anthropic::{
        build_anthropic_count_tokens_request_body, build_bedrock_count_tokens_request_body,
        strip_search_extra_tools_fields,
    };

    #[test]
    fn anthropic_schema_sanitizer_strips_top_level_combinators_but_keeps_nested() {
        use super::anthropic::build_anthropic_request_body;

        let schema = json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "items": {
                    "type": "array",
                    "items": { "anyOf": [{ "type": "string" }, { "type": "object" }] }
                }
            },
            "anyOf": [{ "required": ["command"] }, { "required": ["cmd"] }],
            "oneOf": [{ "required": ["command"] }],
            "allOf": [{ "required": ["command"] }],
            "additionalProperties": false,
        });

        // Exercise via a full request to confirm the schema sanitizer runs.
        let request = ProviderRequest {
            session_id: "test".to_string(),
            prompt: "test".to_string(),
            context: make_turn_context(),
            messages: vec![TranscriptMessage::new(MessageRole::User, "hi".to_string())],
            system_prompt: "sys".to_string(),
            tools: vec![ProviderToolDefinition {
                name: "test_tool".to_string(),
                description: "d".to_string(),
                input_schema: schema,
            }],
            model: "test-model".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: None,
            auth_token: None,
            disable_thinking: true,
            effort: None,
            options: ProviderRequestOptions::default(),
        };

        let body = build_anthropic_request_body(&request);
        let tool_schema = &body["tools"][0]["input_schema"];

        assert!(tool_schema.get("anyOf").is_none());
        assert!(tool_schema.get("oneOf").is_none());
        assert!(tool_schema.get("allOf").is_none());
        // Nested combinators (valid for Anthropic) must survive untouched.
        assert!(
            tool_schema["properties"]["items"]["items"]
                .get("anyOf")
                .is_some()
        );
        assert!(tool_schema["properties"]["command"].is_object());
    }

    fn make_turn_context() -> orbcode_protocol::TurnContext {
        orbcode_protocol::TurnContext {
            cwd: "/tmp/perf-fixture".to_string(),
            additional_directories: Vec::new(),
            additional_directory_details: Vec::new(),
            repo_root: Some("/tmp/perf-fixture".to_string()),
            cwd_relative_to_repo: Some(".".to_string()),
            current_date: "2026-05-28".to_string(),
            git_branch: Some("main".to_string()),
            git_default_branch: Some("main".to_string()),
            git_user: Some("test-user".to_string()),
            git_status: Some("On branch main\nnothing to commit".to_string()),
            git_recent_commits: Some("abc1234 initial commit".to_string()),
            git_remote: Some("origin".to_string()),
            git_worktree_state: None,
            trusted_project: Some(true),
            memory_sources: Vec::new(),
            claude_md: Some("# Test project\nPerformance fixture.".to_string()),
        }
    }

    fn make_provider_request(
        message_count: usize,
        tool_count: usize,
        tool_result_chars: usize,
    ) -> ProviderRequest {
        let mut messages = Vec::with_capacity(message_count);
        for index in 0..message_count {
            if index % 3 == 0 {
                let tool_input = serde_json::json!({
                    "file_path": format!("/tmp/fixture/file_{index}.rs"),
                    "description": format!("perf fixture tool input {index}")
                })
                .to_string();
                messages.push(TranscriptMessage::from_blocks(
                    MessageRole::Assistant,
                    vec![
                        TranscriptBlock::Text {
                            text: format!("perf fixture assistant response {index}"),
                        },
                        TranscriptBlock::ToolUse {
                            id: format!("tool-use-{index}"),
                            name: "Read".to_string(),
                            input: tool_input,
                        },
                    ],
                ));
            } else if index % 3 == 1 {
                let content = "x".repeat(tool_result_chars);
                messages.push(TranscriptMessage::from_blocks(
                    MessageRole::User,
                    vec![TranscriptBlock::ToolResult {
                        tool_use_id: format!("tool-use-{}", index - 1),
                        content: content.into(),
                        is_error: false,
                        metadata: None,
                    }],
                ));
            } else {
                messages.push(TranscriptMessage::new(
                    MessageRole::User,
                    format!("perf fixture user message {index}"),
                ));
            }
        }

        let tools: Vec<ProviderToolDefinition> = (0..tool_count)
            .map(|index| ProviderToolDefinition {
                name: format!("tool_{index}"),
                description: format!("Perf fixture tool definition {index}"),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "file path" },
                        "content": { "type": "string", "description": "file content" }
                    },
                    "required": ["path"]
                }),
            })
            .collect();

        ProviderRequest {
            session_id: "perf-fixture-session".to_string(),
            prompt: "perf fixture fallback prompt".to_string(),
            context: make_turn_context(),
            messages,
            system_prompt: "You are a performance fixture assistant.".to_string(),
            tools,
            model: "claude-sonnet-4-20250514".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: None,
            auth_token: None,
            disable_thinking: true,
            effort: None,
            options: ProviderRequestOptions::default(),
        }
    }

    #[test]
    fn regression_budget_anthropic_body_message_count_matches_input() {
        let request = make_provider_request(100, 5, 200);
        let body = build_anthropic_request_body(&request);

        let messages = body["messages"].as_array().expect("messages array");
        let non_system_input = request
            .messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .count();
        assert!(
            !messages.is_empty(),
            "output should contain at least one message"
        );
        assert!(
            messages.len() <= non_system_input + 1,
            "output messages ({}) should not exceed input non-system messages ({}) + context",
            messages.len(),
            non_system_input
        );

        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 5);

        assert!(
            body.get("system").is_some(),
            "body should include system prompt"
        );
    }

    #[test]
    fn regression_budget_anthropic_body_grows_with_message_count() {
        let small_request = make_provider_request(10, 5, 200);
        let large_request = make_provider_request(100, 5, 200);

        let small_body = build_anthropic_request_body(&small_request);
        let large_body = build_anthropic_request_body(&large_request);

        let small_bytes = serde_json::to_string(&small_body)
            .expect("serialize small")
            .len();
        let large_bytes = serde_json::to_string(&large_body)
            .expect("serialize large")
            .len();

        assert!(
            large_bytes > small_bytes,
            "100-message body ({large_bytes} bytes) should be larger than 10-message body ({small_bytes} bytes)"
        );
    }

    #[test]
    fn regression_budget_truncate_bounds_output_below_max_chars() {
        let large_content = "a".repeat(200_000);
        let truncated = truncate_tool_result_for_provider(&large_content);
        let truncated_chars = truncated.chars().count();
        assert!(
            truncated_chars <= MAX_PROVIDER_TOOL_RESULT_CHARS + 300,
            "truncated output ({truncated_chars} chars) should be bounded by max + overhead"
        );
        assert!(truncated.contains("Orb Code truncated an oversized earlier tool result"));

        let bash_content = format!(
            "{}\n{}",
            "b".repeat(200_000),
            BASH_TRANSCRIPT_TRUNCATION_MARKER
        );
        let bash_truncated = truncate_tool_result_for_provider(&bash_content);
        let bash_truncated_chars = bash_truncated.chars().count();
        assert!(
            bash_truncated_chars <= MAX_PROVIDER_TRUNCATED_BASH_TOOL_RESULT_CHARS + 300,
            "bash-truncated output ({bash_truncated_chars} chars) should use stricter limit"
        );
    }

    #[test]
    fn regression_budget_split_preview_preserves_line_boundaries() {
        let content = (0..1000)
            .map(|i| format!("line {i:04}: stable content for preview boundary testing\n"))
            .collect::<String>();
        let total_chars = content.chars().count();

        let (head, tail, omitted) = split_preview_on_line_boundaries(&content, 5000, 2000);
        assert!(
            head.is_empty() || head.ends_with('\n'),
            "head should end at a line boundary"
        );
        assert_eq!(
            head.chars().count() + tail.chars().count() + omitted,
            total_chars,
            "head + tail + omitted should equal total"
        );
    }

    #[test]
    fn regression_budget_tool_use_inputs_round_trip_as_json_objects() {
        let mut messages = Vec::new();
        for index in 0..20 {
            let input_json = serde_json::json!({
                "file_path": format!("/tmp/file_{index}.rs"),
                "nested": { "depth": index, "values": [1, 2, 3] }
            })
            .to_string();
            messages.push(TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: format!("tool-{index}"),
                    name: "Read".to_string(),
                    input: input_json,
                }],
            ));
            messages.push(TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: format!("tool-{index}"),
                    content: format!("result for tool {index}").into(),
                    is_error: false,
                    metadata: None,
                }],
            ));
        }

        let serialized = anthropic_messages(&messages, "fallback");

        let tool_use_blocks: Vec<&Value> = serialized
            .iter()
            .filter_map(|msg| msg.get("content"))
            .filter_map(Value::as_array)
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            .collect();

        assert_eq!(tool_use_blocks.len(), 20);
        for block in &tool_use_blocks {
            assert!(
                block.get("input").and_then(Value::as_object).is_some(),
                "tool_use input should deserialize to a JSON object, not a string"
            );
        }
    }

    #[test]
    #[ignore = "manual stress test for large-history provider request serialization"]
    fn request_build_stress_serializes_long_session_history() {
        const MESSAGE_COUNT: usize = 2_000;
        const TOOL_COUNT: usize = 30;
        const TOOL_RESULT_CHARS: usize = 5_000;
        const ITERATIONS: usize = 10;

        let request = make_provider_request(MESSAGE_COUNT, TOOL_COUNT, TOOL_RESULT_CHARS);

        let started = Instant::now();
        let mut body_bytes = 0;
        for _ in 0..ITERATIONS {
            let body = build_anthropic_request_body(&request);
            body_bytes = serde_json::to_string(&body).expect("serialize").len();
        }
        let duration = started.elapsed();

        eprintln!(
            "messages={MESSAGE_COUNT} tools={TOOL_COUNT} tool_result_chars={TOOL_RESULT_CHARS} \
             body_bytes={body_bytes} iterations={ITERATIONS} \
             total_us={} avg_us={}",
            duration.as_micros(),
            duration.as_micros() / ITERATIONS as u128
        );
    }

    #[test]
    #[ignore = "manual stress test for provider tool result truncation at scale"]
    fn truncate_tool_result_stress_processes_oversized_results() {
        const RESULT_CHARS: usize = 500_000;
        const ITERATIONS: usize = 1_000;

        let content = "x".repeat(RESULT_CHARS);

        let started = Instant::now();
        let mut output_chars = 0;
        for _ in 0..ITERATIONS {
            let truncated = truncate_tool_result_for_provider(&content);
            output_chars = truncated.chars().count();
        }
        let duration = started.elapsed();

        eprintln!(
            "result_chars={RESULT_CHARS} output_chars={output_chars} \
             iterations={ITERATIONS} total_us={} avg_us={}",
            duration.as_micros(),
            duration.as_micros() / ITERATIONS as u128
        );
    }

    #[test]
    fn strip_search_extra_tools_fields_removes_caller_from_tool_use() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "tu_1",
                    "name": "Read",
                    "input": { "path": "/tmp/x" },
                    "caller": "search_extra_tools"
                },
                { "type": "text", "text": "hello" }
            ]
        })];

        strip_search_extra_tools_fields(&mut messages);

        let tool_use = &messages[0]["content"][0];
        assert_eq!(tool_use["type"], json!("tool_use"));
        assert_eq!(tool_use["id"], json!("tu_1"));
        assert_eq!(tool_use["name"], json!("Read"));
        assert_eq!(tool_use["input"], json!({ "path": "/tmp/x" }));
        assert!(
            tool_use.get("caller").is_none(),
            "caller must be stripped from tool_use blocks"
        );
        // Sibling text block is untouched.
        assert_eq!(messages[0]["content"][1]["text"], json!("hello"));
    }

    #[test]
    fn strip_search_extra_tools_fields_filters_tool_reference_blocks() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tu_1",
                    "content": [
                        { "type": "text", "text": "result" },
                        { "type": "tool_reference", "tool_name": "Search" }
                    ]
                }
            ]
        })];

        strip_search_extra_tools_fields(&mut messages);

        let inner = messages[0]["content"][0]["content"]
            .as_array()
            .expect("tool_result content stays an array");
        assert_eq!(inner.len(), 1, "tool_reference block is removed");
        assert_eq!(inner[0]["type"], json!("text"));
    }

    #[test]
    fn strip_search_extra_tools_fields_replaces_all_reference_content() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tu_1",
                    "content": [
                        { "type": "tool_reference", "tool_name": "A" },
                        { "type": "tool_reference", "tool_name": "B" }
                    ]
                }
            ]
        })];

        strip_search_extra_tools_fields(&mut messages);

        let inner = messages[0]["content"][0]["content"]
            .as_array()
            .expect("placeholder content is an array");
        assert_eq!(inner.len(), 1);
        assert_eq!(
            inner[0],
            json!({ "type": "text", "text": "[tool references]" })
        );
    }

    #[test]
    fn count_tokens_body_has_no_anthropic_only_request_fields() {
        let request = make_provider_request(4, 2, 16);
        let body = build_anthropic_count_tokens_request_body(&request);
        let map = body.as_object().expect("body is an object");

        // count-tokens must not carry streaming-turn-only fields.
        for forbidden in [
            "max_tokens",
            "stream",
            "temperature",
            "metadata",
            "tool_choice",
        ] {
            assert!(
                !map.contains_key(forbidden),
                "count-tokens body must not contain `{forbidden}`"
            );
        }

        // Every tool_use block is reduced to the canonical four keys.
        for message in body["messages"].as_array().unwrap_or(&Vec::new()) {
            let Some(content) = message.get("content").and_then(Value::as_array) else {
                continue;
            };
            for block in content {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let keys: Vec<&String> = block
                        .as_object()
                        .expect("tool_use is an object")
                        .keys()
                        .collect();
                    assert!(
                        keys.iter()
                            .all(|k| ["type", "id", "name", "input"].contains(&k.as_str())),
                        "tool_use retains only canonical keys, got {keys:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn bedrock_count_tokens_body_uses_bedrock_version_and_dummy_message() {
        let mut request = make_provider_request(0, 0, 0);
        request.prompt = String::new();
        let body = build_bedrock_count_tokens_request_body(&request);

        assert_eq!(body["anthropic_version"], json!("bedrock-2023-05-31"));
        assert_eq!(body["max_tokens"], json!(1));
        let messages = body["messages"].as_array().expect("messages present");
        assert!(
            !messages.is_empty(),
            "a dummy message is injected so tool-only counts are accurate"
        );
        assert!(
            body.get("model").is_none(),
            "Bedrock count-tokens body carries no `model` field (it goes in the URL)"
        );
    }
}
