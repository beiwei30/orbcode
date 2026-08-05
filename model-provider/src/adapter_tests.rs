use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration as StdDuration;

use super::*;
use orbcode_protocol::{
    EffortLevel, MessageRole, ProviderId, ProviderToolDefinition, TokenUsage, TranscriptBlock,
    TranscriptMessage, TurnContext,
};
use serde_json::{Value, json};

const MAX_PROVIDER_TOOL_RESULT_CHARS: usize = 100_000;
const MAX_PROVIDER_TRUNCATED_BASH_TOOL_RESULT_CHARS: usize = 8_000;
const BASH_TRANSCRIPT_TRUNCATION_MARKER: &str = "Bash output truncated for transcript safety.";

fn provider_test_request(base_url: String) -> ProviderRequest {
    ProviderRequest {
        session_id: "session-1".to_string(),
        prompt: "inspect repo".to_string(),
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            repo_root: Some("/tmp/project".to_string()),
            current_date: "2026-04-15".to_string(),
            git_branch: Some("main".to_string()),
            ..Default::default()
        },
        messages: vec![TranscriptMessage::new(MessageRole::User, "inspect repo")],
        system_prompt: "use tools".to_string(),
        tools: vec![],
        model: "stub-model".to_string(),
        base_url,
        api_key: Some("test-api-key".to_string()),
        auth_token: None,
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    }
}

fn large_bash_tool_result_content() -> String {
    format!(
        "{}\n\n[Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion. Omitted 30139 characters.]",
        "line\n".repeat(12_000),
    )
}

fn large_bash_tool_result_request(base_url: String) -> ProviderRequest {
    let mut request = provider_test_request(base_url);
    request.messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-large-bash".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"yes line | head -n 12000"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-large-bash".to_string(),
                content: large_bash_tool_result_content().into(),
                is_error: false,
                metadata: Some(
                    json!({
                        "progressMessages": [
                            {
                                "data": {
                                    "type": "bash_progress",
                                    "status": "Streaming stdout",
                                    "bytes": 53248
                                }
                            }
                        ],
                        "bash": {
                            "outputTruncated": true,
                            "omittedChars": 30139
                        }
                    })
                    .to_string(),
                ),
            }],
        ),
    ];
    request
}

#[tokio::test]
async fn stub_tool_response_keeps_small_tool_result_content() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"pwd"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "/tmp/project".into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("stub tool response");

    assert_eq!(response.content, "Tool `bash` completed.\n\n/tmp/project");
}

#[tokio::test]
async fn stub_tool_response_truncates_large_tool_result_preview() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"python3 -c 'print(\"line\\n\" * 12000)'"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "line\n".repeat(12_000).into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("stub tool response");

    assert!(response.content.starts_with("Tool `bash` completed."));
    assert!(
        response
            .content
            .contains("Stub tool result preview truncated")
    );
    assert!(response.content.contains("Omitted 58000 middle characters"));
    assert!(response.content.len() < 2_500);
    assert!(response.deltas.len() < 500);
}

#[tokio::test]
async fn stub_tool_result_preview_keeps_tail_on_line_boundary() {
    let content = format!("{}{}", "line\n".repeat(12_000), "xxx");
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"python3 -c 'print(\"line\\n\" * 12000)'"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: content.into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("stub tool response");
    let tail = response
        .content
        .split("middle characters.]\n\n")
        .nth(1)
        .expect("preview tail");

    assert!(tail.starts_with("line\n"), "{tail}");
    assert!(!tail.lines().any(|line| line == "e"), "{tail}");
    assert!(tail.ends_with("xxx"), "{tail}");
}

#[test]
fn anthropic_request_body_bounds_truncated_large_bash_tool_results() {
    let request = large_bash_tool_result_request("stub://anthropic".to_string());

    let body = build_anthropic_request_body(&request);
    let tool_result = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .flat_map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .expect("tool result block");
    let content = tool_result["content"]
        .as_str()
        .expect("tool result content should serialize as text");

    assert!(content.contains("Orb Code truncated an oversized earlier tool result"));
    assert!(content.contains(BASH_TRANSCRIPT_TRUNCATION_MARKER));
    assert!(content.contains("Omitted 30139 characters"));
    assert!(content.chars().count() <= MAX_PROVIDER_TRUNCATED_BASH_TOOL_RESULT_CHARS + 512);
    assert!(content.matches("line\n").count() < 2_000);
    let tail = content
        .split("middle characters.]\n\n")
        .nth(1)
        .expect("provider preview tail");
    assert!(tail.starts_with("line\n"), "{tail}");
    assert!(!tail.lines().any(|line| line == "e"), "{tail}");
    assert!(!content.contains("progressMessages"));
    assert!(!content.contains("bash_progress"));
}

#[test]
fn openai_request_body_bounds_truncated_large_bash_tool_results() {
    let request = large_bash_tool_result_request("stub://openai".to_string());

    let body = build_openai_request_body(&request);
    let tool_message = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        .expect("tool message");
    let content = tool_message["content"]
        .as_str()
        .expect("tool content should serialize as text");

    assert!(content.contains("Orb Code truncated an oversized earlier tool result"));
    assert!(content.contains(BASH_TRANSCRIPT_TRUNCATION_MARKER));
    assert!(content.contains("Omitted 30139 characters"));
    assert!(content.chars().count() <= MAX_PROVIDER_TRUNCATED_BASH_TOOL_RESULT_CHARS + 512);
    assert!(content.matches("line\n").count() < 2_000);
    let tail = content
        .split("middle characters.]\n\n")
        .nth(1)
        .expect("provider preview tail");
    assert!(tail.starts_with("line\n"), "{tail}");
    assert!(!tail.lines().any(|line| line == "e"), "{tail}");
    assert!(!content.contains("progressMessages"));
    assert!(!content.contains("bash_progress"));
}

fn start_collecting_anthropic_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind collecting stream server");
    let address = listener.local_addr().expect("collecting server addr");

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept collecting request");
        let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"plan\",\"signature\":\"sig-1\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool-1\",\"name\":\"bash\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\":\\\"pwd\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":5}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body,
        );
        stream
            .write_all(response.as_bytes())
            .expect("write collecting stream response");
        let _ = stream.flush();
    });

    (format!("http://{address}"), handle)
}

fn start_count_tokens_server() -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind count tokens server");
    let address = listener.local_addr().expect("count tokens server addr");

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept count tokens request");
        let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
        let mut request = [0_u8; 16 * 1024];
        let bytes = stream
            .read(&mut request)
            .expect("read count tokens request");
        let request_text = String::from_utf8_lossy(&request[..bytes]).to_string();
        let body = r#"{"input_tokens":321}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body,
        );
        stream
            .write_all(response.as_bytes())
            .expect("write count tokens response");
        let _ = stream.flush();
        request_text
    });

    (format!("http://{address}"), handle)
}

#[test]
fn render_blocks_for_display_omits_thinking_blocks() {
    let rendered = render_blocks_for_display(&[
        TranscriptBlock::Thinking {
            text: "internal plan".to_string(),
            signature: None,
        },
        TranscriptBlock::Text {
            text: "visible answer".to_string(),
        },
    ]);

    assert_eq!(rendered, "visible answer");
}

#[test]
fn render_blocks_for_display_omits_tool_marker_content() {
    let rendered = render_blocks_for_display(&[
        TranscriptBlock::Text {
            text: "Let me inspect the workspace.".to_string(),
        },
        TranscriptBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "glob".to_string(),
            input: r#"{"pattern":"src/**/*"}"#.to_string(),
        },
    ]);

    assert_eq!(rendered, "Let me inspect the workspace.");
}

#[test]
fn usage_from_value_parses_rich_anthropic_fields() {
    let usage = usage_from_value(Some(&json!({
        "input_tokens": 10,
        "cache_creation_input_tokens": 20,
        "cache_read_input_tokens": 30,
        "output_tokens": 4,
        "server_tool_use": {
            "web_search_requests": 2,
            "web_fetch_requests": 1
        },
        "service_tier": "priority",
        "cache_creation": {
            "ephemeral_1h_input_tokens": 12,
            "ephemeral_5m_input_tokens": 8
        },
        "iterations": [
            { "input_tokens": 5, "output_tokens": 1 },
            { "input_tokens": 8, "output_tokens": 2 }
        ],
        "speed": "fast"
    })));

    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.cache_creation_input_tokens, 20);
    assert_eq!(usage.cache_read_input_tokens, 30);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.server_tool_use.web_search_requests, 2);
    assert_eq!(usage.server_tool_use.web_fetch_requests, 1);
    assert_eq!(usage.service_tier.as_deref(), Some("priority"));
    assert_eq!(usage.cache_creation.ephemeral_1h_input_tokens, 12);
    assert_eq!(usage.cache_creation.ephemeral_5m_input_tokens, 8);
    assert_eq!(usage.iterations.len(), 2);
    assert_eq!(usage.iterations[1].input_tokens, 8);
    assert_eq!(usage.iterations[1].output_tokens, 2);
    assert_eq!(usage.speed.as_deref(), Some("fast"));
    assert_eq!(usage.total_tokens, 64);
}

#[test]
fn merge_usage_preserves_input_and_cache_when_delta_zeros_them() {
    let mut total = usage_from_value(Some(&json!({
        "input_tokens": 10,
        "cache_creation_input_tokens": 20,
        "cache_read_input_tokens": 30,
        "output_tokens": 0
    })));
    let delta = usage_from_value(Some(&json!({
        "input_tokens": 0,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "output_tokens": 5
    })));

    merge_usage(&mut total, &delta);

    assert_eq!(total.input_tokens, 10);
    assert_eq!(total.cache_creation_input_tokens, 20);
    assert_eq!(total.cache_read_input_tokens, 30);
    assert_eq!(total.output_tokens, 5);
    assert_eq!(total.total_tokens, 65);
}

#[test]
fn anthropic_request_body_includes_system_and_tools() {
    let request = ProviderRequest {
        session_id: "session-1".to_string(),
        prompt: "inspect repo".to_string(),
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            repo_root: Some("/tmp/project".to_string()),
            current_date: "2026-04-15".to_string(),
            git_branch: Some("main".to_string()),
            ..Default::default()
        },
        messages: vec![TranscriptMessage::new(MessageRole::User, "inspect repo")],
        system_prompt: "use tools".to_string(),
        tools: vec![ProviderToolDefinition {
            name: "file-read".to_string(),
            description: "Read file content.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                }
            }),
        }],
        model: "stub-model".to_string(),
        base_url: "stub://anthropic".to_string(),
        api_key: None,
        auth_token: Some("token".to_string()),
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let body = build_anthropic_request_body(&request);
    assert_eq!(body["system"], "use tools");
    assert_eq!(body["tools"][0]["name"], "file-read");
    assert_eq!(body["tool_choice"]["type"], "auto");
    assert!(
        body["messages"][0]["content"][0]["text"]
            .as_str()
            .expect("system reminder text")
            .contains("<system-reminder>")
    );
    assert!(
        body["messages"][0]["content"][0]["text"]
            .as_str()
            .expect("system reminder text")
            .contains("# currentDate")
    );
    assert!(
        body["messages"][0]["content"][0]["text"]
            .as_str()
            .expect("system reminder text")
            .contains("# currentWorkingDirectory")
    );
    assert!(
        body["messages"][0]["content"][0]["text"]
            .as_str()
            .expect("system reminder text")
            .contains("# repositoryRoot")
    );
    assert!(body.get("thinking").is_none());
}

#[test]
fn anthropic_request_body_can_disable_thinking() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.disable_thinking = true;

    let body = build_anthropic_request_body(&request);

    assert_eq!(body["thinking"]["type"], "disabled");
}

#[test]
fn anthropic_request_body_maps_effort_to_thinking_budget() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.effort = Some(EffortLevel::High);

    let body = build_anthropic_request_body(&request);

    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 8192);
    assert_eq!(body["max_tokens"], 12288);
}

#[test]
fn anthropic_request_body_disable_thinking_overrides_effort() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.disable_thinking = true;
    request.effort = Some(EffortLevel::Max);

    let body = build_anthropic_request_body(&request);

    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(body["thinking"].get("budget_tokens").is_none());
    assert_eq!(body["max_tokens"], 4096);
}

#[test]
fn anthropic_request_body_prepends_claude_md_user_context() {
    let request = ProviderRequest {
        session_id: "session-1".to_string(),
        prompt: "inspect repo".to_string(),
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            repo_root: Some("/tmp/project".to_string()),
            cwd_relative_to_repo: Some("orbcode".to_string()),
            current_date: "2026-04-15".to_string(),
            git_branch: Some("main".to_string()),
            claude_md: Some("Follow AGENTS.md carefully.".to_string()),
            ..Default::default()
        },
        messages: vec![TranscriptMessage::new(MessageRole::User, "inspect repo")],
        system_prompt: "use tools".to_string(),
        tools: vec![],
        model: "stub-model".to_string(),
        base_url: "stub://anthropic".to_string(),
        api_key: None,
        auth_token: Some("token".to_string()),
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let body = build_anthropic_request_body(&request);
    let reminder = body["messages"][0]["content"][0]["text"]
        .as_str()
        .expect("system reminder text");

    assert!(reminder.contains("# currentDate"));
    assert!(reminder.contains("# currentWorkingDirectory"));
    assert!(reminder.contains("# repositoryRoot"));
    assert!(reminder.contains("# repositorySubdirectory"));
    assert!(reminder.contains("# claudeMd"));
    assert!(reminder.contains("Follow AGENTS.md carefully."));
    assert_eq!(body["messages"][1]["role"], "user");
}

#[test]
fn anthropic_request_body_preserves_thinking_blocks() {
    let request = ProviderRequest {
        session_id: "session-123".to_string(),
        prompt: "Inspect the repo".to_string(),
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            repo_root: Some("/tmp/project".to_string()),
            current_date: "2026-04-15".to_string(),
            git_branch: Some("main".to_string()),
            ..Default::default()
        },
        messages: vec![TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Thinking {
                    text: "need to inspect files".to_string(),
                    signature: Some("sig-123".to_string()),
                },
                TranscriptBlock::Text {
                    text: "I'll inspect the repository.".to_string(),
                },
            ],
        )],
        system_prompt: "system".to_string(),
        tools: Vec::new(),
        model: "claude-sonnet-4-5".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        api_key: Some("test-key".to_string()),
        auth_token: None,
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let body = build_anthropic_request_body(&request);
    let content = body["messages"][1]["content"]
        .as_array()
        .expect("assistant content array");

    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "need to inspect files");
    assert_eq!(content[0]["signature"], "sig-123");
    assert_eq!(content[1]["type"], "text");
}

#[test]
fn anthropic_request_body_truncates_oversized_tool_results() {
    let request = ProviderRequest {
        session_id: "session-1".to_string(),
        prompt: "inspect repo".to_string(),
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            repo_root: Some("/tmp/project".to_string()),
            current_date: "2026-04-15".to_string(),
            git_branch: Some("main".to_string()),
            ..Default::default()
        },
        messages: vec![TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "a".repeat(MAX_PROVIDER_TOOL_RESULT_CHARS + 50_000).into(),
                is_error: false,
                metadata: None,
            }],
        )],
        system_prompt: "use tools".to_string(),
        tools: vec![],
        model: "stub-model".to_string(),
        base_url: "stub://anthropic".to_string(),
        api_key: None,
        auth_token: Some("token".to_string()),
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let body = build_anthropic_request_body(&request);
    let content = body["messages"][1]["content"][0]["content"]
        .as_str()
        .expect("tool result content should serialize as text");

    assert!(content.contains("Orb Code truncated an oversized earlier tool result"));
    assert!(content.len() < MAX_PROVIDER_TOOL_RESULT_CHARS + 1024);
}

#[test]
fn anthropic_http_request_uses_expected_headers_and_body() {
    let request = ProviderRequest {
        session_id: "session-1".to_string(),
        prompt: "inspect repo".to_string(),
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            repo_root: Some("/tmp/project".to_string()),
            cwd_relative_to_repo: Some("orbcode".to_string()),
            current_date: "2026-04-15".to_string(),
            git_branch: Some("main".to_string()),
            ..Default::default()
        },
        messages: vec![TranscriptMessage::new(MessageRole::User, "inspect repo")],
        system_prompt: "use tools".to_string(),
        tools: vec![],
        model: "stub-model".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        api_key: None,
        auth_token: Some("token".to_string()),
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let client = build_anthropic_http_client().expect("client");
    let http_request =
        build_anthropic_http_request(&client, &request, "https://api.anthropic.com/v1/messages")
            .expect("request");

    assert_eq!(http_request.method(), reqwest::Method::POST);
    assert_eq!(
        http_request.url().as_str(),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(http_request.headers()["content-type"], "application/json");
    assert_eq!(http_request.headers()["accept"], "text/event-stream");
    assert_eq!(
        http_request.headers()["x-claude-code-session-id"],
        "session-1"
    );
    assert_eq!(http_request.headers()["authorization"], "Bearer token");

    let body = http_request
        .body()
        .and_then(|body| body.as_bytes())
        .expect("json body bytes");
    let body = String::from_utf8(body.to_vec()).expect("utf8 body");
    assert!(body.contains("\"model\":\"stub-model\""));
    assert!(body.contains("\"stream\":true"));
}

/// The two providers deliberately send DIFFERENT session-id headers: Anthropic
/// gets `x-claude-code-session-id` because the API recognises it from the
/// TypeScript CLI, OpenAI gets our own `x-orbcode-session-id`. Pin both so the
/// asymmetry stays intentional rather than drifting unnoticed.
#[test]
fn openai_http_request_sends_orbcode_session_header() {
    let request = ProviderRequest {
        session_id: "session-1".to_string(),
        prompt: "inspect repo".to_string(),
        context: TurnContext::default(),
        messages: vec![TranscriptMessage::new(MessageRole::User, "inspect repo")],
        system_prompt: "use tools".to_string(),
        tools: vec![],
        model: "stub-model".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: Some("key".to_string()),
        auth_token: None,
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    // Same generic reqwest client the other OpenAI header tests use.
    let client = build_anthropic_http_client().expect("client");
    let http_request = build_openai_http_request(
        &client,
        &request,
        "https://api.openai.com/v1/chat/completions",
    )
    .expect("openai request");

    assert_eq!(http_request.headers()["x-orbcode-session-id"], "session-1");
    assert!(
        !http_request
            .headers()
            .contains_key("x-claude-code-session-id"),
        "the Claude-compat session header belongs to the Anthropic path only"
    );
}

#[test]
fn anthropic_body_respects_request_option_overrides() {
    let mut request = provider_test_request("https://api.anthropic.com".to_string());
    request.options.max_output_tokens = Some(2048);
    request.options.temperature = Some(0.25);
    request.options.anthropic_betas = vec![
        "context-1m-2025-01-14".to_string(),
        "prompt-caching-2024-07-31".to_string(),
    ];
    request.options.metadata = Some(json!({"user_id": "device-xyz"}));
    request
        .options
        .extra_body
        .insert("custom_flag".to_string(), json!(true));
    request.options.extra_body.insert(
        "anthropic_beta".to_string(),
        json!(["context-1m-2025-01-14", "task-budgets-2024-12-01"]),
    );

    let body = build_anthropic_request_body(&request);
    assert_eq!(body["max_tokens"], json!(2048));
    assert_eq!(body["temperature"], json!(0.25));
    assert_eq!(body["metadata"], json!({"user_id": "device-xyz"}));
    assert_eq!(body["custom_flag"], json!(true));
    let merged_betas = body["anthropic_beta"]
        .as_array()
        .expect("anthropic_beta array")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        merged_betas,
        vec![
            "context-1m-2025-01-14".to_string(),
            "task-budgets-2024-12-01".to_string(),
            "prompt-caching-2024-07-31".to_string(),
        ]
    );
}

#[test]
fn openai_body_respects_request_option_overrides() {
    let mut request = provider_test_request("https://api.openai.com/v1".to_string());
    request.options.max_output_tokens = Some(512);
    request.options.temperature = Some(0.5);
    request
        .options
        .extra_body
        .insert("reasoning_effort".to_string(), json!("low"));

    let body = build_openai_request_body(&request);
    assert_eq!(body["max_tokens"], json!(512));
    assert_eq!(body["temperature"], json!(0.5));
    assert_eq!(body["reasoning_effort"], json!("low"));
}

#[test]
fn anthropic_http_request_attaches_request_option_headers() {
    let mut request = provider_test_request("https://api.anthropic.com".to_string());
    request.auth_token = Some("token".to_string());
    request.api_key = None;
    request.options.anthropic_betas = vec!["prompt-caching-2024-07-31".to_string()];
    request.options.request_id = Some("req-1234".to_string());
    request.options.user_agent = Some("orbcode/test".to_string());
    request
        .options
        .custom_headers
        .push(("X-Test-Header".to_string(), "yes".to_string()));

    let client = build_anthropic_http_client().expect("client");
    let http_request =
        build_anthropic_http_request(&client, &request, "https://api.anthropic.com/v1/messages")
            .expect("request");

    let headers = http_request.headers();
    assert_eq!(headers["anthropic-beta"], "prompt-caching-2024-07-31");
    assert_eq!(headers["x-client-request-id"], "req-1234");
    assert_eq!(headers["user-agent"], "orbcode/test");
    assert_eq!(headers["x-test-header"], "yes");
}

#[test]
fn openai_http_request_attaches_request_option_headers_without_anthropic_beta() {
    let mut request = provider_test_request("https://api.openai.com/v1".to_string());
    request.api_key = Some("sk-test".to_string());
    request.options.anthropic_betas = vec!["should-not-leak".to_string()];
    request.options.request_id = Some("openai-req-1".to_string());
    request.options.user_agent = Some("orbcode/openai".to_string());

    let client = build_anthropic_http_client().expect("client");
    let http_request = build_openai_http_request(
        &client,
        &request,
        "https://api.openai.com/v1/chat/completions",
    )
    .expect("openai request");

    let headers = http_request.headers();
    assert!(headers.get("anthropic-beta").is_none());
    assert_eq!(headers["x-client-request-id"], "openai-req-1");
    assert_eq!(headers["user-agent"], "orbcode/openai");
    assert_eq!(headers["authorization"], "Bearer sk-test");
}

#[test]
fn provider_http_client_applies_proxy_and_timeout_options() {
    use crate::build_provider_http_client;
    let options = ProviderRequestOptions {
        proxy: Some("http://proxy.invalid:8080".to_string()),
        proxy_no_proxy: Some("localhost,127.0.0.1".to_string()),
        timeout: Some(std::time::Duration::from_secs(7)),
        user_agent: Some("orbcode/proxy-test".to_string()),
        ..ProviderRequestOptions::default()
    };
    build_provider_http_client(&options)
        .expect("client builder accepts proxy/timeout/user-agent options");
}

#[test]
fn provider_http_client_does_not_echo_invalid_proxy_credentials() {
    use crate::build_provider_http_client;
    let options = ProviderRequestOptions {
        proxy: Some("http://proxy-user:proxy-secret@[invalid".to_string()),
        ..ProviderRequestOptions::default()
    };
    let error = build_provider_http_client(&options).expect_err("proxy URL is invalid");
    assert_eq!(error.message, "invalid configured proxy URL");
    assert!(!error.message.contains("proxy-secret"));
}

#[tokio::test]
async fn generate_wrapper_matches_stub_provider_response() {
    let request = provider_test_request("stub://anthropic".to_string());
    let expected_content = format!(
        "OpenAI-compatible phase 2 response\n\nsession: {}\nmodel: {}\ncontext: {}\ngit_status: {}\n\nprompt:\n{}\n\nThis response came from the local compatibility stub.",
        request.session_id,
        request.model,
        request.context.compact_summary(),
        request
            .context
            .git_status
            .clone()
            .unwrap_or_else(|| "clean-or-unavailable".to_string()),
        request.prompt
    );
    let actual = provider_for(ProviderId::OpenAi)
        .generate(&request)
        .await
        .expect("stub generate response");

    assert_eq!(actual.provider, ProviderId::OpenAi);
    assert_eq!(actual.fallback_from, None);
    assert_eq!(actual.content, expected_content);
    assert_eq!(
        actual.blocks,
        vec![TranscriptBlock::Text {
            text: expected_content.clone()
        }]
    );
    assert_eq!(actual.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(
        actual.usage,
        TokenUsage::from_text(&request.prompt, &expected_content)
    );
    assert!(!actual.deltas.is_empty());
}

#[tokio::test]
async fn generate_wrapper_collects_anthropic_http_stream() {
    let (base_url, server_handle) = start_collecting_anthropic_server();
    let request = provider_test_request(base_url);
    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("anthropic stream generate response");

    server_handle
        .join()
        .expect("collecting stream server joins");
    assert_eq!(response.provider, ProviderId::Anthropic);
    assert_eq!(response.fallback_from, None);
    assert_eq!(response.content, "Hello");
    assert_eq!(response.deltas, vec!["Hel".to_string(), "lo".to_string()]);
    assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(response.usage.input_tokens, 3);
    assert_eq!(response.usage.output_tokens, 5);
    assert_eq!(response.usage.total_tokens, 8);
    assert!(matches!(
        response.blocks.as_slice(),
        [
            TranscriptBlock::Thinking { text, signature },
            TranscriptBlock::Text { text: visible },
            TranscriptBlock::ToolUse { id, name, input },
        ] if text == "plan"
            && signature.as_deref() == Some("sig-1")
            && visible == "Hello"
            && id == "tool-1"
            && name == "bash"
            && input == r#"{"command":"pwd"}"#
    ));
}

#[test]
fn anthropic_request_body_merges_transcript_system_messages() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.system_prompt = "base system".to_string();
    request.messages.insert(
        0,
        TranscriptMessage::new(MessageRole::System, "compacted history summary"),
    );

    let body = build_anthropic_request_body(&request);

    assert_eq!(body["system"], "base system\n\ncompacted history summary");
}

#[tokio::test]
async fn count_tokens_uses_anthropic_endpoint_with_request_context() {
    let (base_url, server_handle) = start_count_tokens_server();
    let mut request = provider_test_request(base_url);
    request.tools = vec![ProviderToolDefinition {
        name: "Bash".to_string(),
        description: "Run shell".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            },
            "required": ["command"]
        }),
    }];

    let tokens = provider_for(ProviderId::Anthropic)
        .count_tokens(&request)
        .await
        .expect("count tokens request");
    let request_text = server_handle.join().expect("count tokens server joins");

    assert_eq!(tokens, Some(321));
    assert!(request_text.contains("POST /v1/messages/count_tokens"));
    assert!(request_text.contains(r#""system":"use tools""#));
    assert!(request_text.contains("currentWorkingDirectory"));
    assert!(request_text.contains(r#""tools":[{"name":"Bash","description":"Run shell""#));
}

#[test]
fn openai_request_body_converts_messages_tools_and_schema() {
    let mut request = provider_test_request("https://api.openai.com/v1".to_string());
    request.model = "gpt-4o".to_string();
    request.system_prompt = "You are helpful.".to_string();
    request.messages = vec![
        TranscriptMessage::new(MessageRole::User, "list files"),
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Thinking {
                    text: "internal".to_string(),
                    signature: Some("sig".to_string()),
                },
                TranscriptBlock::Text {
                    text: "I'll inspect.".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "bash".to_string(),
                    input: r#"{"command":"ls"}"#.to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: "file.txt".into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];
    request.tools = vec![ProviderToolDefinition {
        name: "bash".to_string(),
        description: "Run a shell command.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "mode": { "const": "read" },
                "nested": {
                    "type": "object",
                    "properties": {
                        "kind": { "const": "file" }
                    }
                }
            }
        }),
    }];

    let body = build_openai_request_body(&request);
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "You are helpful.");
    let assistant = body["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .expect("assistant message");
    assert_eq!(assistant["content"], "I'll inspect.");
    assert_eq!(assistant["tool_calls"][0]["id"], "toolu_1");
    assert_eq!(assistant["tool_calls"][0]["function"]["name"], "bash");
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        r#"{"command":"ls"}"#
    );
    assert!(body["messages"].as_array().unwrap().iter().any(|message| {
        message.get("role").and_then(Value::as_str) == Some("tool")
            && message.get("tool_call_id").and_then(Value::as_str) == Some("toolu_1")
    }));
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["mode"]["enum"][0],
        "read"
    );
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["nested"]["properties"]["kind"]["enum"]
            [0],
        "file"
    );
}

#[tokio::test]
async fn mock_provider_streams_success_without_prompt_markers() {
    let request = provider_test_request("mock://anthropic?scenario=success".to_string());
    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("mock success");
    assert_eq!(response.provider, ProviderId::Anthropic);
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert!(response.content.contains("mock provider response"));
}

#[tokio::test]
async fn mock_provider_success_is_default_scenario() {
    let request = provider_test_request("mock://anthropic".to_string());
    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("mock default success");
    assert!(response.content.contains("mock provider response"));
}

#[tokio::test]
async fn mock_provider_retryable_scenario_returns_retryable_server_error() {
    let request = provider_test_request("mock://anthropic?scenario=retryable".to_string());
    let error = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect_err("mock retryable failure");
    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::ServerError);
    assert_eq!(error.provider, Some(ProviderId::Anthropic));
    assert!(error.suggestion.is_some());
}

#[tokio::test]
async fn mock_provider_fatal_scenario_returns_fatal_error() {
    let request = provider_test_request("mock://openai?scenario=fatal".to_string());
    let error = provider_for(ProviderId::OpenAi)
        .generate(&request)
        .await
        .expect_err("mock fatal failure");
    assert_eq!(error.kind, ProviderErrorKind::Fatal);
    assert_eq!(error.provider, Some(ProviderId::OpenAi));
}

#[tokio::test]
async fn mock_provider_interrupted_scenario_returns_interrupted_error() {
    let request = provider_test_request("mock://anthropic?scenario=interrupted".to_string());
    let error = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect_err("mock interrupted failure");
    assert_eq!(error.kind, ProviderErrorKind::Interrupted);
    assert_eq!(error.category, StreamErrorCategory::Interrupted);
    assert!(error.suggestion.is_none());
}

#[tokio::test]
async fn mock_provider_ratelimit_scenario_carries_429_and_retry_after() {
    let request = provider_test_request("mock://anthropic?scenario=ratelimit".to_string());
    let error = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect_err("mock rate limit");
    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::RateLimit);
    assert_eq!(error.status, Some(429));
    assert_eq!(error.retry_after_secs(), Some(1));
    assert!(error.suggestion.is_some());
}

#[tokio::test]
async fn mock_provider_retry_then_success_fails_n_times_then_succeeds() {
    let key = format!("retry-then-success-{}", uuid::Uuid::new_v4());
    let base_url = format!("mock://anthropic?scenario=retry_then_success&attempts=2&key={key}");
    let request = provider_test_request(base_url);

    for attempt in 1..=2 {
        let error = provider_for(ProviderId::Anthropic)
            .generate(&request)
            .await
            .err()
            .unwrap_or_else(|| panic!("attempt {attempt} should fail retryably"));
        assert_eq!(error.kind, ProviderErrorKind::Retryable);
    }
    // Third call clears the configured failure budget and succeeds.
    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("third attempt succeeds");
    assert!(response.content.contains("mock provider response"));
}

#[tokio::test]
async fn mock_provider_count_tokens_is_a_noop() {
    let request = provider_test_request("mock://anthropic?scenario=retryable".to_string());
    let count = provider_for(ProviderId::Anthropic)
        .count_tokens(&request)
        .await
        .expect("count tokens never fails for the mock");
    assert_eq!(count, None);
}

#[tokio::test]
async fn probe_provider_reports_success_for_healthy_provider() {
    let request = provider_test_request("mock://anthropic?scenario=success".to_string());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;
    assert!(report.is_ok());
    assert!(report.error.is_none());
}

#[tokio::test]
async fn probe_provider_surfaces_normalized_auth_error_with_suggestion() {
    let request = provider_test_request("mock://anthropic?scenario=auth".to_string());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;
    assert!(!report.is_ok());
    let error = report.error.expect("probe surfaces error");
    assert_eq!(error.category, StreamErrorCategory::Auth);
    assert_eq!(error.provider, Some(ProviderId::Anthropic));
    let suggestion = error.suggestion.expect("auth error carries a suggestion");
    assert!(
        suggestion.contains("ANTHROPIC_API_KEY") && suggestion.contains("orbcode auth status"),
        "auth suggestion should point at the documented credential remedies, got: {suggestion}"
    );
}

#[test]
fn extract_http_error_message_prefers_json_error_payloads() {
    assert_eq!(
        extract_http_error_message(
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#
        ),
        "invalid x-api-key"
    );
    assert_eq!(
        extract_http_error_message(503, ""),
        "HTTP_STATUS/503 provider request failed"
    );
}

#[tokio::test]
async fn stub_then_directive_chains_second_tool_call() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.prompt = r#"#tool:bash {"command":"cd /tmp"} #then:bash {"command":"pwd"}"#.to_string();
    request.messages = vec![
        TranscriptMessage::new(MessageRole::User, &request.prompt),
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "toolu-session-1".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"cd /tmp"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "toolu-session-1".to_string(),
                content: "".into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("chained tool response");

    assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(response.blocks.len(), 1);
    if let TranscriptBlock::ToolUse { name, input, .. } = &response.blocks[0] {
        assert_eq!(name, "bash");
        assert!(input.contains("pwd"));
    } else {
        panic!("expected ToolUse block, got: {:?}", response.blocks[0]);
    }
}

#[tokio::test]
async fn stub_then_directive_ends_after_all_rounds() {
    let mut request = provider_test_request("stub://anthropic".to_string());
    request.prompt = r#"#tool:bash {"command":"cd /tmp"} #then:bash {"command":"pwd"}"#.to_string();
    request.messages = vec![
        TranscriptMessage::new(MessageRole::User, &request.prompt),
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "toolu-1".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"cd /tmp"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "toolu-1".to_string(),
                content: "".into(),
                is_error: false,
                metadata: None,
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "toolu-2".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"pwd"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "toolu-2".to_string(),
                content: "/tmp".into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let response = provider_for(ProviderId::Anthropic)
        .generate(&request)
        .await
        .expect("final text response");

    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert!(response.content.contains("Tool `bash` completed."));
    assert!(response.content.contains("/tmp"));
}

#[tokio::test]
async fn probe_classify_returns_ok_for_healthy_provider() {
    let request = provider_test_request("mock://anthropic?scenario=success".to_string());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;
    assert!(matches!(report.classify(), ProbeResult::Ok));
}

#[tokio::test]
async fn probe_classify_returns_rate_limited_with_retry_after() {
    let request = provider_test_request("mock://anthropic?scenario=ratelimit".to_string());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;
    match report.classify() {
        ProbeResult::RateLimited {
            retry_after_seconds,
        } => {
            assert_eq!(
                retry_after_seconds,
                Some(1),
                "rate-limit probe should carry the retry-after value from the mock"
            );
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn probe_classify_returns_account_suspended_for_402() {
    let request = provider_test_request("mock://anthropic?scenario=account_suspended".to_string());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;
    assert!(
        matches!(report.classify(), ProbeResult::AccountSuspended),
        "402 probe should classify as AccountSuspended, got {:?}",
        report.classify()
    );
    let error = report.error.expect("probe surfaces error");
    assert_eq!(error.status, Some(402));
    assert_eq!(error.category, StreamErrorCategory::AccountSuspended);
    let suggestion = error
        .suggestion
        .expect("account-suspended carries a suggestion");
    assert!(
        suggestion.contains("suspended") || suggestion.contains("billing"),
        "suggestion should mention account/billing, got: {suggestion}"
    );
}

// ── Provider request compat transformation tests ──────────────────────

#[test]
fn openai_body_does_not_include_thinking_field() {
    let request = provider_test_request("http://localhost".to_string());
    let body = build_openai_request_body(&request);
    assert!(
        body.get("thinking").is_none(),
        "OpenAI body must not include the Anthropic thinking field"
    );
}

#[test]
fn anthropic_body_does_not_include_stream_options() {
    let request = provider_test_request("http://localhost".to_string());
    let body = build_anthropic_request_body(&request);
    assert!(
        body.get("stream_options").is_none(),
        "Anthropic body must not include stream_options (OpenAI-only)"
    );
}

#[test]
fn openai_body_includes_stream_options() {
    let request = provider_test_request("http://localhost".to_string());
    let body = build_openai_request_body(&request);
    let stream_options = body
        .get("stream_options")
        .expect("OpenAI body should include stream_options");
    assert_eq!(stream_options["include_usage"], true);
}

#[test]
fn openai_body_drops_thinking_blocks_from_messages() {
    let mut request = provider_test_request("http://localhost".to_string());
    request.messages = vec![
        TranscriptMessage::new(MessageRole::User, "hello"),
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Thinking {
                    text: "internal reasoning".to_string(),
                    signature: None,
                },
                TranscriptBlock::Text {
                    text: "visible reply".to_string(),
                },
            ],
        ),
    ];
    let body = build_openai_request_body(&request);
    let body_str = serde_json::to_string(&body).unwrap();
    assert!(
        !body_str.contains("internal reasoning"),
        "OpenAI body must strip Thinking blocks from messages"
    );
    assert!(
        body_str.contains("visible reply"),
        "OpenAI body must preserve visible text"
    );
}

#[test]
fn openai_body_maps_effort_to_reasoning_effort() {
    let mut request = provider_test_request("http://localhost".to_string());
    request.effort = Some(EffortLevel::Low);
    let body = build_openai_request_body(&request);
    assert_eq!(
        body.get("reasoning_effort").and_then(Value::as_str),
        Some("low"),
        "EffortLevel::Low should map to reasoning_effort: low"
    );

    request.effort = Some(EffortLevel::Medium);
    let body = build_openai_request_body(&request);
    assert_eq!(body["reasoning_effort"], "medium");

    request.effort = Some(EffortLevel::High);
    let body = build_openai_request_body(&request);
    assert_eq!(body["reasoning_effort"], "high");

    request.effort = Some(EffortLevel::Max);
    let body = build_openai_request_body(&request);
    assert_eq!(
        body["reasoning_effort"], "high",
        "EffortLevel::Max should clamp to OpenAI's highest: high"
    );
}

#[test]
fn openai_body_omits_reasoning_effort_when_thinking_disabled() {
    let mut request = provider_test_request("http://localhost".to_string());
    request.effort = Some(EffortLevel::High);
    request.disable_thinking = true;
    let body = build_openai_request_body(&request);
    assert!(
        body.get("reasoning_effort").is_none(),
        "reasoning_effort must be omitted when disable_thinking is true"
    );
}

#[test]
fn openai_body_omits_reasoning_effort_when_no_effort() {
    let request = provider_test_request("http://localhost".to_string());
    let body = build_openai_request_body(&request);
    assert!(
        body.get("reasoning_effort").is_none(),
        "reasoning_effort must be omitted when effort is None"
    );
}

#[test]
fn anthropic_body_maps_effort_to_thinking_budget() {
    let mut request = provider_test_request("http://localhost".to_string());
    request.effort = Some(EffortLevel::Low);
    let body = build_anthropic_request_body(&request);
    let thinking = body.get("thinking").expect("should have thinking");
    assert_eq!(thinking["type"], "enabled");
    assert_eq!(thinking["budget_tokens"], 1024);

    request.effort = Some(EffortLevel::High);
    let body = build_anthropic_request_body(&request);
    assert_eq!(body["thinking"]["budget_tokens"], 8192);
}

#[test]
fn anthropic_body_thinking_disabled_overrides_effort() {
    let mut request = provider_test_request("http://localhost".to_string());
    request.effort = Some(EffortLevel::High);
    request.disable_thinking = true;
    let body = build_anthropic_request_body(&request);
    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(body["thinking"].get("budget_tokens").is_none());
}
