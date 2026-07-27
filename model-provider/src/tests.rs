use orbcode_protocol::{
    MessageRole, ProviderId, TokenUsage, TranscriptBlock, TranscriptMessage, TurnContext,
};
use serde_json::json;

use super::{
    AnthropicStreamReader, OpenAiStreamReader, ProviderContentBlockDelta, ProviderError,
    ProviderErrorKind, ProviderRequest, ProviderRequestOptions, ProviderStreamAccumulator,
    ProviderStreamEvent, RateLimitMetadata, StreamErrorCategory, classify_provider_error,
    count_tokens_anthropic, merge_usage, parse_provider_error_body,
    provider_stream_event_from_sse_frame, render_blocks_for_display, retry_delay_ms,
    sanitize_provider_error_message, usage_from_value,
};

fn consume_anthropic_sse_frame(
    event_name: &str,
    data: &str,
    accumulator: &mut ProviderStreamAccumulator,
) -> Result<(), ProviderError> {
    if let Some(event) = provider_stream_event_from_sse_frame(event_name, data)? {
        let event = normalize_legacy_event_index(event, accumulator);
        accumulator.apply(&event);
    }
    Ok(())
}

fn normalize_legacy_event_index(
    event: ProviderStreamEvent,
    accumulator: &ProviderStreamAccumulator,
) -> ProviderStreamEvent {
    match event {
        ProviderStreamEvent::ContentBlockStart { index, block } if index == usize::MAX => {
            ProviderStreamEvent::ContentBlockStart {
                index: accumulator.block_count(),
                block,
            }
        }
        ProviderStreamEvent::ContentBlockDelta { index, delta } if index == usize::MAX => {
            ProviderStreamEvent::ContentBlockDelta {
                index: accumulator.block_count().saturating_sub(1),
                delta,
            }
        }
        ProviderStreamEvent::ContentBlockStop { index } if index == usize::MAX => {
            ProviderStreamEvent::ContentBlockStop {
                index: accumulator.block_count().saturating_sub(1),
            }
        }
        event => event,
    }
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
fn merge_usage_preserves_input_and_cache_when_delta_zeros_them() {
    let mut total = TokenUsage {
        input_tokens: 10,
        cache_creation_input_tokens: 20,
        cache_read_input_tokens: 30,
        total_tokens: 60,
        ..TokenUsage::default()
    };
    let delta = TokenUsage {
        output_tokens: 5,
        total_tokens: 5,
        ..TokenUsage::default()
    };

    merge_usage(&mut total, &delta);

    assert_eq!(total.input_tokens, 10);
    assert_eq!(total.cache_creation_input_tokens, 20);
    assert_eq!(total.cache_read_input_tokens, 30);
    assert_eq!(total.output_tokens, 5);
    assert_eq!(total.total_tokens, 65);
}

#[test]
fn usage_from_value_parses_rich_provider_fields() {
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
fn anthropic_sse_frame_keeps_thinking_out_of_visible_deltas() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    consume_anthropic_sse_frame(
        "content_block_start",
        r#"{"type":"content_block_start","content_block":{"type":"thinking","thinking":"plan"}}"#,
        &mut accumulator,
    )
    .expect("thinking start frame");
    consume_anthropic_sse_frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" more"}}"#,
            &mut accumulator,
        )
        .expect("thinking delta frame");
    consume_anthropic_sse_frame(
        "content_block_start",
        r#"{"type":"content_block_start","content_block":{"type":"text","text":"Hello"}}"#,
        &mut accumulator,
    )
    .expect("text start frame");

    let (content, blocks, _stop_reason, _usage, deltas) = accumulator.into_parts();
    assert_eq!(content, "Hello");
    assert_eq!(deltas, vec!["Hello".to_string()]);
    assert!(matches!(
        blocks.as_slice(),
        [
            TranscriptBlock::Thinking { text, .. },
            TranscriptBlock::Text { text: visible }
        ] if text == "plan more" && visible == "Hello"
    ));
}

#[test]
fn anthropic_sse_frame_preserves_thinking_signatures() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    consume_anthropic_sse_frame(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"plan"}}"#,
            &mut accumulator,
        )
        .expect("thinking start frame");
    consume_anthropic_sse_frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-xyz"}}"#,
            &mut accumulator,
        )
        .expect("signature delta frame");

    let (_content, blocks, _stop_reason, _usage, _deltas) = accumulator.into_parts();
    assert!(matches!(
        blocks.as_slice(),
        [TranscriptBlock::Thinking { text, signature }]
            if text == "plan" && signature.as_deref() == Some("sig-xyz")
    ));
}

#[test]
fn anthropic_sse_frame_replaces_empty_tool_input_before_json_deltas() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    consume_anthropic_sse_frame(
            "content_block_start",
            r#"{"type":"content_block_start","content_block":{"type":"tool_use","id":"tool_1","name":"bash","input":{}}}"#,
            &mut accumulator,
        )
        .expect("tool start frame");
    consume_anthropic_sse_frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}"#,
            &mut accumulator,
        )
        .expect("tool input delta");

    let (_content, blocks, _stop_reason, _usage, _deltas) = accumulator.into_parts();
    assert!(matches!(
        blocks.as_slice(),
        [TranscriptBlock::ToolUse { id, name, input }]
            if id == "tool_1" && name == "bash" && input == "{\"command\":\"pwd\"}"
    ));
}

#[test]
fn anthropic_sse_frame_respects_explicit_block_indices_for_tool_input_deltas() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    consume_anthropic_sse_frame(
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking"}}"#,
        &mut accumulator,
    )
    .expect("redacted thinking start should preserve slot");
    consume_anthropic_sse_frame(
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_1","name":"bash","input":{}}}"#,
            &mut accumulator,
        )
        .expect("tool use start should parse");
    consume_anthropic_sse_frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}"#,
            &mut accumulator,
        )
        .expect("tool delta should append to indexed tool block");

    let (_content, blocks, _stop_reason, _usage, _deltas) = accumulator.into_parts();
    assert!(matches!(
        blocks.as_slice(),
        [
            TranscriptBlock::Thinking { text, .. },
            TranscriptBlock::ToolUse { id, name, input }
        ] if text.is_empty() && id == "tool_1" && name == "bash" && input == "{\"command\":\"pwd\"}"
    ));
}

#[test]
fn anthropic_sse_frame_marks_rate_limit_stream_errors_retryable() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    let error = consume_anthropic_sse_frame(
            "error",
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"HTTP_STATUS/429 rate limit exceeded"}}"#,
            &mut accumulator,
        )
        .expect_err("rate limit frame should error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert!(error.message.contains("429"));
}

#[test]
fn anthropic_sse_frame_marks_overloaded_stream_errors_retryable() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    let error = consume_anthropic_sse_frame(
        "error",
        r#"{"type":"error","error":{"type":"overloaded_error","message":"server overloaded"}}"#,
        &mut accumulator,
    )
    .expect_err("overloaded frame should error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
}

#[test]
fn anthropic_sse_error_event_accepts_plaintext_rate_limit_payload() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    let error = consume_anthropic_sse_frame(
        "error",
        "HTTP_STATUS/429 rate limit exceeded",
        &mut accumulator,
    )
    .expect_err("plaintext error frame should error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.message, "HTTP_STATUS/429 rate limit exceeded");
}

#[test]
fn anthropic_sse_error_event_sanitizes_compacted_sse_control_text() {
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    let error = consume_anthropic_sse_frame(
        "error",
        "id:1event:error:data:HTTP_STATUS/429 rate limit exceeded",
        &mut accumulator,
    )
    .expect_err("compacted plaintext error frame should error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.message, "HTTP_STATUS/429 rate limit exceeded");
}

#[test]
fn sanitize_provider_error_message_strips_sse_prefix_noise() {
    assert_eq!(
        sanitize_provider_error_message("id:1event:error:HTTP_STATUS/529 overloaded"),
        "HTTP_STATUS/529 overloaded"
    );
    assert_eq!(
        sanitize_provider_error_message("event:error\ndata:HTTP_STATUS/429 retry later"),
        "HTTP_STATUS/429 retry later"
    );
    // A legitimate error containing the substring "data:" mid-message must not
    // be truncated: only a line-anchored SSE `data:` frame is stripped.
    assert_eq!(
        sanitize_provider_error_message("failed to load data: connection reset"),
        "failed to load data: connection reset"
    );
}

#[test]
fn chunked_stream_reader_reassembles_split_sse_frames() {
    let mut stream = AnthropicStreamReader::default();
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    stream
            .push_chunk(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"Hel",
                &mut accumulator,
            )
            .expect("first chunk");
    stream
            .push_chunk(
                b"lo\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n",
                &mut accumulator,
            )
            .expect("second chunk");
    stream.finish(&mut accumulator).expect("finish stream");

    let (content, blocks, _stop_reason, _usage, deltas) = accumulator.into_parts();
    assert_eq!(content, "Hello world");
    assert_eq!(deltas, vec!["Hello".to_string(), " world".to_string()]);
    assert!(matches!(
        blocks.as_slice(),
        [TranscriptBlock::Text { text }] if text == "Hello world"
    ));
}

#[test]
fn chunked_stream_reader_emits_raw_style_events_in_order() {
    let mut stream = AnthropicStreamReader::default();
    let events = stream
            .push_chunk_events(
                concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
                    "event: content_block_start\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
                    "event: message_delta\n",
                    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                )
                .as_bytes(),
            )
            .expect("stream events");

    assert!(matches!(
        events.as_slice(),
        [
            ProviderStreamEvent::MessageStart { usage, .. },
            ProviderStreamEvent::ContentBlockStart { index: 0, .. },
            ProviderStreamEvent::ContentBlockDelta {
                index: 0,
                delta: super::ProviderContentBlockDelta::Text(text),
            },
            ProviderStreamEvent::MessageDelta { stop_reason, usage: output_usage },
            ProviderStreamEvent::MessageStop,
        ] if usage.input_tokens == 2
            && text == "hi"
            && stop_reason.as_deref() == Some("end_turn")
            && output_usage.output_tokens == 1
    ));
}

#[test]
fn openai_trailing_usage_only_chunk_emits_token_totals() {
    // OpenAI with `stream_options.include_usage` sends the token counts in a
    // trailing chunk with `choices: []`, after the finish_reason chunk. The
    // adapter must emit a MessageDelta carrying those counts; otherwise totals
    // stay 0 for the whole turn.
    let mut stream = OpenAiStreamReader::new("gpt-4o".to_string());
    let events = stream
        .push_chunk_events(
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":4,\"total_tokens\":15}}\n\n",
                "data: [DONE]\n\n",
            )
            .as_bytes(),
        )
        .expect("openai stream events");

    let usage = events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::MessageDelta { usage, .. } => Some(usage),
            _ => None,
        })
        .find(|usage| usage.output_tokens > 0)
        .expect("a MessageDelta must carry the trailing usage");
    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.output_tokens, 4);
}

#[test]
fn openai_stream_adapter_converts_text_tool_reasoning_and_finish() {
    let mut stream = OpenAiStreamReader::new("gpt-4o".to_string());
    let events = stream
            .push_chunk_events(
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"comm\"}}]},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"and\\\":\\\"ls\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":5,\"total_tokens\":12,\"prompt_tokens_details\":{\"cached_tokens\":3}}}\n\n",
                    "data: [DONE]\n\n",
                )
                .as_bytes(),
            )
            .expect("openai stream events");

    assert!(events.iter().any(|event| {
        matches!(
            event,
            ProviderStreamEvent::ContentBlockDelta {
                delta: ProviderContentBlockDelta::Thinking(text),
                ..
            } if text == "think"
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ProviderStreamEvent::ContentBlockDelta {
                delta: ProviderContentBlockDelta::Text(text),
                ..
            } if text == "Hello"
        )
    }));
    let input_json = events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::ContentBlockDelta {
                delta: ProviderContentBlockDelta::InputJson(partial),
                ..
            } => Some(partial.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(input_json, r#"{"command":"ls"}"#);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ProviderStreamEvent::MessageDelta {
                stop_reason: Some(reason),
                usage,
            } if reason == "tool_use"
                && usage.input_tokens == 4
                && usage.cache_read_input_tokens == 3
                && usage.output_tokens == 5
                && usage.total_tokens == 12
        )
    }));
    assert!(matches!(
        events.last(),
        Some(ProviderStreamEvent::MessageStop)
    ));
}

#[test]
fn openai_stream_adapter_maps_finish_reasons_and_closes_unfinished_blocks() {
    let mut length_stream = OpenAiStreamReader::new("gpt-4o".to_string());
    let events = length_stream
            .push_chunk_events(
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"truncated\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
                )
                .as_bytes(),
            )
            .expect("length events");
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ProviderStreamEvent::MessageDelta {
                stop_reason: Some(reason),
                ..
            } if reason == "max_tokens"
        )
    }));

    let mut unfinished = OpenAiStreamReader::new("gpt-4o".to_string());
    unfinished
            .push_chunk_events(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
            )
            .expect("partial event");
    let finish_events = unfinished.finish_events().expect("finish closes blocks");
    assert!(matches!(
        finish_events.as_slice(),
        [ProviderStreamEvent::ContentBlockStop { index: 0 }]
    ));
}

#[test]
fn openai_stream_fixture_preserves_text_usage_and_cache_mapping() {
    let response = collect_openai_fixture(concat!(
        "data: {\"id\":\"chatcmpl-openai\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-openai\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-openai\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-openai\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":2,\"total_tokens\":14,\"prompt_tokens_details\":{\"cached_tokens\":4}}}\n\n",
        "data: [DONE]\n\n",
    ));

    assert_eq!(response.provider, ProviderId::OpenAi);
    assert_eq!(response.content, "Hello");
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(response.deltas, vec!["Hel".to_string(), "lo".to_string()]);
    assert_eq!(response.usage.input_tokens, 8);
    assert_eq!(response.usage.cache_read_input_tokens, 4);
    assert_eq!(response.usage.output_tokens, 2);
    assert_eq!(response.usage.total_tokens, 14);
}

#[test]
fn ollama_like_stream_fixture_accepts_openai_compatible_text_chunks() {
    let response = collect_openai_fixture(concat!(
        "data: {\"model\":\"llama3.1\",\"created_at\":\"2026-05-26T00:00:00Z\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"local \"},\"finish_reason\":null}]}\n\n",
        "data: {\"model\":\"llama3.1\",\"created_at\":\"2026-05-26T00:00:01Z\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"answer\"},\"finish_reason\":null}]}\n\n",
        "data: {\"model\":\"llama3.1\",\"created_at\":\"2026-05-26T00:00:02Z\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n\n",
        "data: [DONE]\n\n",
    ));

    assert_eq!(response.content, "local answer");
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(response.usage.input_tokens, 5);
    assert_eq!(response.usage.output_tokens, 2);
    assert_eq!(response.usage.total_tokens, 7);
}

#[test]
fn deepseek_like_stream_fixture_preserves_reasoning_as_thinking() {
    let response = collect_openai_fixture(concat!(
        "data: {\"id\":\"deepseek-chat\",\"choices\":[{\"delta\":{\"reasoning_content\":\"inspect\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"deepseek-chat\",\"choices\":[{\"delta\":{\"reasoning_content\":\" files\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"deepseek-chat\",\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"deepseek-chat\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":3,\"total_tokens\":12}}\n\n",
        "data: [DONE]\n\n",
    ));

    assert_eq!(response.content, "done");
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert!(matches!(
        response.blocks.as_slice(),
        [
            TranscriptBlock::Thinking { text, .. },
            TranscriptBlock::Text { text: answer },
        ] if text == "inspect files" && answer == "done"
    ));
}

#[test]
fn vllm_like_stream_fixture_preserves_streamed_tool_calls() {
    let response = collect_openai_fixture(concat!(
        "data: {\"id\":\"cmpl-vllm\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_vllm_1\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"cmpl-vllm\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"pwd\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"cmpl-vllm\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":15,\"completion_tokens\":6,\"total_tokens\":21}}\n\n",
        "data: [DONE]\n\n",
    ));

    assert_eq!(response.content, "");
    assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
    assert!(matches!(
        response.blocks.as_slice(),
        [TranscriptBlock::ToolUse { id, name, input }]
            if id == "call_vllm_1" && name == "bash" && input == r#"{"command":"pwd"}"#
    ));
}

#[test]
fn openai_streaming_error_uses_normalized_diagnostics() {
    let mut stream = OpenAiStreamReader::new("gpt-4o".to_string());
    let error = stream
        .push_chunk_events(
            b"event: error\ndata: {\"error\":{\"type\":\"invalid_request_error\",\"message\":\"bad tool schema\"}}\n\n",
        )
        .expect_err("OpenAI stream error should use diagnostics");

    assert_eq!(error.provider, Some(ProviderId::OpenAi));
    assert_eq!(error.category, StreamErrorCategory::InvalidRequest);
    assert_eq!(error.kind, ProviderErrorKind::Fatal);
    assert!(
        error
            .suggestion
            .as_deref()
            .unwrap_or_default()
            .contains("OPENAI_MODEL")
    );
}

fn collect_openai_fixture(fixture: &str) -> super::ProviderResponse {
    let mut stream = OpenAiStreamReader::new("fixture-model".to_string());
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::OpenAi, None);
    for event in stream
        .push_chunk_events(fixture.as_bytes())
        .expect("fixture stream events")
    {
        accumulator.apply(&event);
    }
    for event in stream.finish_events().expect("finish fixture stream") {
        accumulator.apply(&event);
    }
    accumulator.into_response()
}

fn anthropic_error_body(error_type: &str, message: &str) -> String {
    json!({
        "type": "error",
        "error": {"type": error_type, "message": message},
    })
    .to_string()
}

fn openai_error_body(error_type: &str, message: &str) -> String {
    json!({
        "error": {"type": error_type, "message": message, "code": error_type},
    })
    .to_string()
}

fn assert_error(
    error: &ProviderError,
    provider: ProviderId,
    status: u16,
    category: StreamErrorCategory,
    kind: ProviderErrorKind,
) {
    assert_eq!(error.provider, Some(provider));
    assert_eq!(error.status, Some(status));
    assert_eq!(error.category, category);
    assert_eq!(error.kind, kind);
    assert!(
        !error.suggestion.as_deref().unwrap_or_default().is_empty(),
        "expected suggestion for {provider:?}/{category:?}, got: {error:?}"
    );
}

#[test]
fn billing_quota_exhaustion_classifies_as_account_suspended_not_rate_limit() {
    // A permanent billing/quota-exhaustion failure must be AccountSuspended
    // (fatal), not RateLimit (retryable) — it was previously caught by the
    // broad `"quota"` → RateLimit branch and retried through the whole backoff.
    let suspended = classify_provider_error(
        Some(ProviderId::Anthropic),
        None,
        "Your credit balance is too low: quota exceeded for this billing account.",
    );
    assert_eq!(suspended.category, StreamErrorCategory::AccountSuspended);

    // A genuine rate limit that mentions "quota" still classifies as RateLimit.
    let rate = classify_provider_error(
        Some(ProviderId::Anthropic),
        Some(429),
        "rate limit exceeded; your quota resets in 60 seconds",
    );
    assert_eq!(rate.category, StreamErrorCategory::RateLimit);
}

#[test]
fn anthropic_401_classifies_as_auth_fatal() {
    let body = anthropic_error_body("authentication_error", "invalid x-api-key");
    let error = parse_provider_error_body(ProviderId::Anthropic, 401, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        401,
        StreamErrorCategory::Auth,
        ProviderErrorKind::Fatal,
    );
    assert!(error.suggestion.as_deref().unwrap().contains("ANTHROPIC"));
}

#[test]
fn anthropic_403_classifies_as_auth_fatal() {
    let body = anthropic_error_body("permission_error", "credential lacks required scope");
    let error = parse_provider_error_body(ProviderId::Anthropic, 403, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        403,
        StreamErrorCategory::Auth,
        ProviderErrorKind::Fatal,
    );
}

#[test]
fn anthropic_expired_oauth_error_has_specific_suggestion() {
    let body = anthropic_error_body("authentication_error", "OAuth token expired");
    let error = parse_provider_error_body(ProviderId::Anthropic, 401, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        401,
        StreamErrorCategory::Auth,
        ProviderErrorKind::Fatal,
    );
    let suggestion = error.suggestion.as_deref().unwrap_or_default();
    assert!(suggestion.contains("expired"), "{suggestion}");
    assert!(suggestion.contains("orbcode auth logout"), "{suggestion}");
}

#[test]
fn anthropic_missing_scope_error_has_specific_suggestion() {
    let body = anthropic_error_body("permission_error", "credential lacks required scope");
    let error = parse_provider_error_body(ProviderId::Anthropic, 403, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        403,
        StreamErrorCategory::Auth,
        ProviderErrorKind::Fatal,
    );
    let suggestion = error.suggestion.as_deref().unwrap_or_default();
    assert!(suggestion.contains("required scope"), "{suggestion}");
    assert!(suggestion.contains("ANTHROPIC_API_KEY"), "{suggestion}");
}

#[test]
fn anthropic_profile_scope_error_has_specific_suggestion() {
    let body = anthropic_error_body("permission_error", "missing user:profile scope");
    let error = parse_provider_error_body(ProviderId::Anthropic, 403, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        403,
        StreamErrorCategory::Auth,
        ProviderErrorKind::Fatal,
    );
    let suggestion = error.suggestion.as_deref().unwrap_or_default();
    assert!(suggestion.contains("user:profile"), "{suggestion}");
    assert!(suggestion.contains("profile/subscription"), "{suggestion}");
}

#[test]
fn anthropic_subscription_access_error_has_specific_suggestion() {
    let body = anthropic_error_body(
        "permission_error",
        "account does not have required subscription access",
    );
    let error = parse_provider_error_body(ProviderId::Anthropic, 403, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        403,
        StreamErrorCategory::Auth,
        ProviderErrorKind::Fatal,
    );
    let suggestion = error.suggestion.as_deref().unwrap_or_default();
    assert!(
        suggestion.contains("subscription/profile access"),
        "{suggestion}"
    );
    assert!(suggestion.contains("organization"), "{suggestion}");
}

#[tokio::test]
async fn anthropic_missing_credentials_mentions_supported_sources() {
    let request = ProviderRequest {
        session_id: "session-1".to_string(),
        prompt: "hello".to_string(),
        context: TurnContext::default(),
        messages: vec![TranscriptMessage::new(MessageRole::User, "hello")],
        system_prompt: String::new(),
        tools: vec![],
        model: "claude-sonnet-4-5".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        api_key: None,
        auth_token: None,
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let error = count_tokens_anthropic(&request)
        .await
        .expect_err("missing credentials should fail before making a request");

    assert_eq!(error.provider, Some(ProviderId::Anthropic));
    assert_eq!(error.category, StreamErrorCategory::Auth);
    assert!(error.message.contains("ANTHROPIC_AUTH_TOKEN"));
    assert!(error.message.contains("ANTHROPIC_API_KEY"));
    assert!(error.message.contains("CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(error.message.contains("orbcode auth status"));

    let suggestion = error.suggestion.as_deref().unwrap_or_default();
    assert!(suggestion.contains("ANTHROPIC_AUTH_TOKEN"), "{suggestion}");
    assert!(suggestion.contains("ANTHROPIC_API_KEY"), "{suggestion}");
    assert!(
        suggestion.contains("CLAUDE_CODE_OAUTH_TOKEN"),
        "{suggestion}"
    );
    assert!(suggestion.contains("orbcode auth status"), "{suggestion}");
}

#[test]
fn anthropic_429_classifies_as_rate_limit_retryable() {
    let body = anthropic_error_body("rate_limit_error", "rate limit exceeded");
    let error = parse_provider_error_body(ProviderId::Anthropic, 429, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        429,
        StreamErrorCategory::RateLimit,
        ProviderErrorKind::Retryable,
    );
}

#[test]
fn anthropic_529_classifies_as_overload_retryable() {
    let body = anthropic_error_body("overloaded_error", "Anthropic is overloaded");
    let error = parse_provider_error_body(ProviderId::Anthropic, 529, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        529,
        StreamErrorCategory::Overload,
        ProviderErrorKind::Retryable,
    );
}

#[test]
fn anthropic_500_classifies_as_server_error_retryable() {
    let body = anthropic_error_body("api_error", "internal server error");
    let error = parse_provider_error_body(ProviderId::Anthropic, 500, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        500,
        StreamErrorCategory::ServerError,
        ProviderErrorKind::Retryable,
    );
}

#[test]
fn anthropic_invalid_request_classifies_as_invalid_request_fatal() {
    let body = anthropic_error_body("invalid_request_error", "messages: invalid role");
    let error = parse_provider_error_body(ProviderId::Anthropic, 400, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        400,
        StreamErrorCategory::InvalidRequest,
        ProviderErrorKind::Fatal,
    );
}

#[test]
fn anthropic_prompt_too_long_classifies_as_prompt_too_long_fatal() {
    let body = anthropic_error_body("invalid_request_error", "prompt is too long: 250000 tokens");
    let error = parse_provider_error_body(ProviderId::Anthropic, 400, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        400,
        StreamErrorCategory::PromptTooLong,
        ProviderErrorKind::Fatal,
    );
    assert!(error.suggestion.as_deref().unwrap().contains("compact"));
}

#[test]
fn anthropic_max_output_classifies_as_max_output_fatal() {
    let body = anthropic_error_body(
        "invalid_request_error",
        "max_tokens must be less than or equal to 4096",
    );
    let error = parse_provider_error_body(ProviderId::Anthropic, 400, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        400,
        StreamErrorCategory::MaxOutput,
        ProviderErrorKind::Fatal,
    );
}

#[test]
fn openai_401_classifies_as_auth_fatal_with_openai_suggestion() {
    let body = openai_error_body("invalid_api_key", "Incorrect API key provided");
    let error = parse_provider_error_body(ProviderId::OpenAi, 401, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        401,
        StreamErrorCategory::Auth,
        ProviderErrorKind::Fatal,
    );
    assert!(
        error
            .suggestion
            .as_deref()
            .unwrap()
            .contains("OPENAI_API_KEY")
    );
}

#[test]
fn openai_429_classifies_as_rate_limit_retryable() {
    let body = openai_error_body("rate_limit_exceeded", "Rate limit reached for requests");
    let error = parse_provider_error_body(ProviderId::OpenAi, 429, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        429,
        StreamErrorCategory::RateLimit,
        ProviderErrorKind::Retryable,
    );
}

#[test]
fn openai_502_classifies_as_server_error_retryable() {
    let body = openai_error_body("server_error", "Bad gateway");
    let error = parse_provider_error_body(ProviderId::OpenAi, 502, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        502,
        StreamErrorCategory::ServerError,
        ProviderErrorKind::Retryable,
    );
}

#[test]
fn openai_invalid_request_classifies_as_invalid_request_fatal() {
    let body = openai_error_body(
        "invalid_request_error",
        "Unknown parameter: 'not_a_real_param'",
    );
    let error = parse_provider_error_body(ProviderId::OpenAi, 400, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        400,
        StreamErrorCategory::InvalidRequest,
        ProviderErrorKind::Fatal,
    );
}

#[test]
fn openai_context_length_classifies_as_prompt_too_long_fatal() {
    let body = openai_error_body(
        "context_length_exceeded",
        "This model's maximum context length is 128000 tokens",
    );
    let error = parse_provider_error_body(ProviderId::OpenAi, 400, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        400,
        StreamErrorCategory::PromptTooLong,
        ProviderErrorKind::Fatal,
    );
}

#[test]
fn anthropic_406_classifies_as_invalid_request_fatal() {
    let error = parse_provider_error_body(ProviderId::Anthropic, 406, "");
    assert_error(
        &error,
        ProviderId::Anthropic,
        406,
        StreamErrorCategory::InvalidRequest,
        ProviderErrorKind::Fatal,
    );
}

#[test]
fn empty_body_yields_synthetic_message_with_status() {
    let error = parse_provider_error_body(ProviderId::Anthropic, 503, "");
    assert_eq!(error.status, Some(503));
    assert_eq!(error.category, StreamErrorCategory::ServerError);
    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert!(error.message.contains("HTTP_STATUS/503"));
}

#[test]
fn anthropic_http_error_type_hint_classifies_generic_message() {
    let body = json!({
        "type": "error",
        "error": {
            "type": "overloaded_error",
            "message": "please try again"
        }
    })
    .to_string();
    let error = parse_provider_error_body(ProviderId::Anthropic, 400, &body);
    assert_error(
        &error,
        ProviderId::Anthropic,
        400,
        StreamErrorCategory::Overload,
        ProviderErrorKind::Retryable,
    );
    assert_eq!(error.message, "please try again");
}

#[test]
fn openai_http_error_code_hint_classifies_generic_message() {
    let body = json!({
        "error": {
            "code": "context_length_exceeded",
            "message": "request rejected"
        }
    })
    .to_string();
    let error = parse_provider_error_body(ProviderId::OpenAi, 400, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        400,
        StreamErrorCategory::PromptTooLong,
        ProviderErrorKind::Fatal,
    );
    assert_eq!(error.message, "request rejected");
}

#[test]
fn openai_top_level_error_hint_classifies_generic_message() {
    let body = json!({
        "code": "context_length_exceeded",
        "message": "request rejected",
        "type": "error"
    })
    .to_string();
    let error = parse_provider_error_body(ProviderId::OpenAi, 400, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        400,
        StreamErrorCategory::PromptTooLong,
        ProviderErrorKind::Fatal,
    );
    assert_eq!(error.message, "request rejected");
}

#[test]
fn string_error_body_is_preserved_and_classified() {
    let body = json!({
        "error": "quota exceeded for this billing account"
    })
    .to_string();
    let error = parse_provider_error_body(ProviderId::OpenAi, 402, &body);
    assert_error(
        &error,
        ProviderId::OpenAi,
        402,
        StreamErrorCategory::AccountSuspended,
        ProviderErrorKind::Fatal,
    );
    assert_eq!(error.message, "quota exceeded for this billing account");
}

#[test]
fn plaintext_http_error_body_is_sanitized_and_classified() {
    let error = parse_provider_error_body(ProviderId::OpenAi, 429, "  Too many requests\n");
    assert_error(
        &error,
        ProviderId::OpenAi,
        429,
        StreamErrorCategory::RateLimit,
        ProviderErrorKind::Retryable,
    );
    assert_eq!(error.message, "Too many requests");
}

#[test]
fn retry_after_wins_when_unified_reset_metadata_is_also_present() {
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_secs()
        + 60;
    let meta = RateLimitMetadata {
        retry_after_secs: Some(3),
        unified_reset_unix: Some(reset),
        unified_status: Some("rejected".to_string()),
        ..RateLimitMetadata::default()
    };

    assert!(
        meta.reset_delay_ms().is_some(),
        "fixture should also contain a future unified reset"
    );
    assert_eq!(
        retry_delay_ms(4, meta.retry_after_secs, 1_000, 1.0),
        3_000,
        "Retry-After bypasses exponential backoff and unified reset metadata"
    );
}

#[test]
fn anthropic_streaming_auth_error_carries_suggestion() {
    let mut stream = AnthropicStreamReader::default();
    let error = stream
        .push_chunk_events(
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"x-api-key invalid\"}}\n\n",
        )
        .expect_err("auth stream frame should produce an error");

    assert_eq!(error.category, StreamErrorCategory::Auth);
    assert_eq!(error.kind, ProviderErrorKind::Fatal);
    assert_eq!(error.provider, Some(ProviderId::Anthropic));
    assert!(
        error
            .suggestion
            .as_deref()
            .unwrap_or_default()
            .contains("ANTHROPIC")
    );
}

#[test]
fn anthropic_streaming_missing_scope_error_carries_specific_suggestion() {
    let mut stream = AnthropicStreamReader::default();
    let error = stream
        .push_chunk_events(
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"permission_error\",\"message\":\"missing user:profile scope\"}}\n\n",
        )
        .expect_err("scope stream frame should produce an error");

    assert_eq!(error.category, StreamErrorCategory::Auth);
    assert_eq!(error.kind, ProviderErrorKind::Fatal);
    assert_eq!(error.provider, Some(ProviderId::Anthropic));
    let suggestion = error.suggestion.as_deref().unwrap_or_default();
    assert!(suggestion.contains("user:profile"), "{suggestion}");
}

#[test]
fn anthropic_streaming_invalid_request_classifies_as_invalid_request() {
    let mut stream = AnthropicStreamReader::default();
    let error = stream
        .push_chunk_events(
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"bad request shape\"}}\n\n",
        )
        .expect_err("invalid request stream frame should produce an error");

    assert_eq!(error.category, StreamErrorCategory::InvalidRequest);
    assert_eq!(error.kind, ProviderErrorKind::Fatal);
}

#[test]
fn anthropic_streaming_prompt_too_long_text_normalizes_category() {
    let mut stream = AnthropicStreamReader::default();
    let error = stream
        .push_chunk_events(
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"prompt is too long: 230000 tokens\"}}\n\n",
        )
        .expect_err("prompt too long should error");

    assert_eq!(error.category, StreamErrorCategory::PromptTooLong);
    assert_eq!(error.kind, ProviderErrorKind::Fatal);
}

#[test]
fn rendered_message_includes_provider_category_and_suggestion() {
    let body = anthropic_error_body("rate_limit_error", "rate limit exceeded");
    let error = parse_provider_error_body(ProviderId::Anthropic, 429, &body);
    let rendered = error.rendered_message();
    assert!(rendered.contains("anthropic"));
    assert!(rendered.contains("rate_limit"));
    assert!(rendered.contains("rate limit exceeded"));
    assert!(rendered.contains("rate limited"));
}
