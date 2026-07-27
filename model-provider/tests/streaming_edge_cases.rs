//! Edge-case coverage for SSE stream parsing, accumulation, and block
//! lifecycle. Tests drive the public `AnthropicStreamReader`,
//! `OpenAiStreamReader`, and `ProviderStreamAccumulator` directly with
//! hand-crafted SSE payloads — no HTTP round-trips required.

use std::fmt::Write as _;

use orbcode_model_provider::{
    AnthropicStreamReader, OpenAiStreamReader, ProviderContentBlockStart, ProviderErrorKind,
    ProviderStreamAccumulator, ProviderStreamEvent, StreamErrorCategory,
    provider_stream_event_from_sse_frame,
};
use orbcode_protocol::{ProviderId, TranscriptBlock};

fn empty_accumulator() -> ProviderStreamAccumulator {
    ProviderStreamAccumulator::new(ProviderId::Anthropic, None)
}

fn push_sse(
    reader: &mut AnthropicStreamReader,
    acc: &mut ProviderStreamAccumulator,
    sse: &str,
) -> Result<(), orbcode_model_provider::ProviderError> {
    for event in reader.push_chunk_events(sse.as_bytes())? {
        acc.apply(&event);
    }
    Ok(())
}

fn finish(
    reader: &mut AnthropicStreamReader,
    acc: &mut ProviderStreamAccumulator,
) -> Result<(), orbcode_model_provider::ProviderError> {
    for event in reader.finish_events()? {
        acc.apply(&event);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 1. Empty thinking block (start → stop, no delta)
// ---------------------------------------------------------------------------

#[test]
fn empty_thinking_block_does_not_panic_or_produce_content() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    assert_eq!(
        acc.content(),
        "hello",
        "visible content must only come from text blocks"
    );
    let response = acc.into_response();
    assert_eq!(response.blocks.len(), 2);
    assert!(
        matches!(&response.blocks[0], TranscriptBlock::Thinking { text, .. } if text.is_empty()),
        "empty thinking block is preserved with empty text"
    );
    assert!(matches!(&response.blocks[1], TranscriptBlock::Text { text } if text == "hello"));
}

// ---------------------------------------------------------------------------
// 2. Thinking block with only a signature delta, no thinking text
// ---------------------------------------------------------------------------

#[test]
fn thinking_block_with_only_signature_no_text() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-only\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    let response = acc.into_response();
    match &response.blocks[0] {
        TranscriptBlock::Thinking { text, signature } => {
            assert!(text.is_empty(), "no thinking text was emitted");
            assert_eq!(signature.as_deref(), Some("sig-only"));
        }
        other => panic!("expected Thinking block, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Partial JSON tool-input delta across two chunks
// ---------------------------------------------------------------------------

#[test]
fn partial_json_tool_input_spliced_across_two_deltas() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"bash\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"com\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"mand\\\":\\\"ls\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    let response = acc.into_response();
    match &response.blocks[0] {
        TranscriptBlock::ToolUse { name, input, .. } => {
            assert_eq!(name, "bash");
            let parsed: serde_json::Value = serde_json::from_str(input)
                .unwrap_or_else(|e| panic!("tool input should be valid JSON: {e}, got: {input}"));
            assert_eq!(parsed["command"], "ls");
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. Many small tool-input deltas joined correctly
// ---------------------------------------------------------------------------

#[test]
fn many_small_tool_input_deltas_joined() {
    let mut sse = String::from(concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"write\",\"input\":{}}}\n\n",
    ));

    let fragments = [
        "{\"fi", "le_p", "ath\"", ":\"te", "st.t", "xt\",", "\"con", "tent", "\":\"h", "ello",
        "\"}",
    ];
    for frag in &fragments {
        let escaped = frag.replace('\\', "\\\\").replace('"', "\\\"");
        write!(
            sse,
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{escaped}\"}}}}\n\n"
        )
        .expect("writing to String cannot fail");
    }
    sse.push_str(concat!(
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    ));

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, &sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    let response = acc.into_response();
    match &response.blocks[0] {
        TranscriptBlock::ToolUse { input, .. } => {
            let parsed: serde_json::Value =
                serde_json::from_str(input).expect("reassembled JSON is valid");
            assert_eq!(parsed["file_path"], "test.txt");
            assert_eq!(parsed["content"], "hello");
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 5. Initial empty {} in tool_use start is replaced by streamed deltas
// ---------------------------------------------------------------------------

#[test]
fn initial_empty_object_cleared_before_json_deltas() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t3\",\"name\":\"bash\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    let response = acc.into_response();
    match &response.blocks[0] {
        TranscriptBlock::ToolUse { input, .. } => {
            assert!(
                !input.starts_with("{}"),
                "initial empty object must be cleared before appending deltas"
            );
            let parsed: serde_json::Value = serde_json::from_str(input).unwrap();
            assert_eq!(parsed["command"], "pwd");
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 6. Malformed SSE: lines without `data:` prefix → plain output, no crash
// ---------------------------------------------------------------------------

#[test]
fn malformed_sse_missing_data_prefix_treated_as_plain_output() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "This line has no SSE prefix at all\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"survived\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    assert_eq!(
        acc.content(),
        "survived",
        "stream continues past non-SSE lines"
    );
    assert!(
        reader
            .plain_output()
            .contains("This line has no SSE prefix"),
        "non-SSE line captured as plain output"
    );
}

// ---------------------------------------------------------------------------
// 7. Malformed SSE: truncated JSON in data field → error, no panic
// ---------------------------------------------------------------------------

#[test]
fn malformed_sse_truncated_json_returns_error() {
    let sse = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let events = reader.push_chunk_events(sse.as_bytes());
    assert!(
        events.is_err(),
        "truncated JSON in an SSE frame must produce an error, not panic"
    );
    let err = events.unwrap_err();
    assert!(
        err.message.contains("invalid streaming frame"),
        "error message indicates a parse failure: {}",
        err.message
    );
}

// ---------------------------------------------------------------------------
// 8. Duplicate [DONE] markers are gracefully ignored
// ---------------------------------------------------------------------------

#[test]
fn duplicate_done_marker_is_graceful() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
        "data: [DONE]\n\n",
        "data: [DONE]\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    assert_eq!(
        acc.content(),
        "hi",
        "duplicate [DONE] does not corrupt state"
    );
    assert_eq!(acc.stop_reason(), Some("end_turn"));
}

// ---------------------------------------------------------------------------
// 9. SSE comment lines (starting with `:`) are silently skipped
// ---------------------------------------------------------------------------

#[test]
fn sse_comment_lines_are_skipped() {
    let sse = concat!(
        ": this is an SSE comment\n",
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        ": another comment mid-stream\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"commented\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    assert_eq!(acc.content(), "commented");
    assert!(
        reader.plain_output().is_empty(),
        "comment lines must not leak into plain output"
    );
}

// ---------------------------------------------------------------------------
// 10. OpenAI: finish_reason "length" → stop_reason "max_tokens"
// ---------------------------------------------------------------------------

#[test]
fn openai_finish_reason_length_maps_to_max_tokens() {
    let sse = concat!(
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"truncated\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    let mut reader = OpenAiStreamReader::new("gpt-test".to_string());
    let mut acc = ProviderStreamAccumulator::new(ProviderId::OpenAi, None);
    for event in reader.push_chunk_events(sse.as_bytes()).unwrap() {
        acc.apply(&event);
    }
    for event in reader.finish_events().unwrap() {
        acc.apply(&event);
    }

    assert_eq!(acc.stop_reason(), Some("max_tokens"));
    assert_eq!(acc.content(), "truncated");
}

// ---------------------------------------------------------------------------
// 11. OpenAI: tool_calls finish_reason → stop_reason "tool_use"
// ---------------------------------------------------------------------------

#[test]
fn openai_finish_reason_tool_calls_maps_to_tool_use() {
    let sse = concat!(
        "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"cmd\\\":\\\"ls\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    let mut reader = OpenAiStreamReader::new("gpt-test".to_string());
    let mut acc = ProviderStreamAccumulator::new(ProviderId::OpenAi, None);
    for event in reader.push_chunk_events(sse.as_bytes()).unwrap() {
        acc.apply(&event);
    }
    for event in reader.finish_events().unwrap() {
        acc.apply(&event);
    }

    assert_eq!(acc.stop_reason(), Some("tool_use"));
    let response = acc.into_response();
    match &response.blocks[0] {
        TranscriptBlock::ToolUse { name, input, .. } => {
            assert_eq!(name, "bash");
            let parsed: serde_json::Value = serde_json::from_str(input).unwrap();
            assert_eq!(parsed["cmd"], "ls");
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 12. OpenAI: finish_reason "stop" → stop_reason "end_turn"
// ---------------------------------------------------------------------------

#[test]
fn openai_finish_reason_stop_maps_to_end_turn() {
    let sse = concat!(
        "data: {\"id\":\"c3\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c3\",\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c3\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    let mut reader = OpenAiStreamReader::new("gpt-test".to_string());
    let mut acc = ProviderStreamAccumulator::new(ProviderId::OpenAi, None);
    for event in reader.push_chunk_events(sse.as_bytes()).unwrap() {
        acc.apply(&event);
    }
    for event in reader.finish_events().unwrap() {
        acc.apply(&event);
    }

    assert_eq!(acc.stop_reason(), Some("end_turn"));
}

// ---------------------------------------------------------------------------
// 13. Thinking block followed by text, then a second thinking block
//     (simulates nested thinking after a tool round in multi-turn)
// ---------------------------------------------------------------------------

#[test]
fn multiple_thinking_blocks_interleaved_with_text() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        // First thinking block (index 0)
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"first thought\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-1\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        // Text block (index 1)
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"response\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        // Second thinking block (index 2) — simulates post-tool-round thinking
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"second thought\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-2\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
        // Final text block (index 3)
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":3,\"delta\":{\"type\":\"text_delta\",\"text\":\" final\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":3}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, sse).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    let response = acc.into_response();
    assert_eq!(response.blocks.len(), 4);

    match &response.blocks[0] {
        TranscriptBlock::Thinking { text, signature } => {
            assert_eq!(text, "first thought");
            assert_eq!(signature.as_deref(), Some("sig-1"));
        }
        other => panic!("expected Thinking[0], got {other:?}"),
    }
    assert!(matches!(&response.blocks[1], TranscriptBlock::Text { text } if text == "response"));
    match &response.blocks[2] {
        TranscriptBlock::Thinking { text, signature } => {
            assert_eq!(text, "second thought");
            assert_eq!(signature.as_deref(), Some("sig-2"));
        }
        other => panic!("expected Thinking[2], got {other:?}"),
    }
    assert!(matches!(&response.blocks[3], TranscriptBlock::Text { text } if text == " final"));

    assert_eq!(
        response.content, "response\n\nfinal",
        "visible content joins text blocks with paragraph separator (thinking excluded)"
    );
}

// ---------------------------------------------------------------------------
// 14. SSE data split across multiple TCP chunks
// ---------------------------------------------------------------------------

#[test]
fn sse_data_split_across_chunks_reassembled() {
    let part1 = "event: message_start\ndata: {\"type\":\"mess";
    let part2 = "age_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";
    let part3 = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"chunked\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let mut reader = AnthropicStreamReader::default();
    let mut acc = empty_accumulator();
    push_sse(&mut reader, &mut acc, part1).unwrap();
    push_sse(&mut reader, &mut acc, part2).unwrap();
    push_sse(&mut reader, &mut acc, part3).unwrap();
    finish(&mut reader, &mut acc).unwrap();

    assert_eq!(acc.content(), "chunked");
}

// ---------------------------------------------------------------------------
// 15. provider_stream_event_from_sse_frame: [DONE] returns Ok(None)
// ---------------------------------------------------------------------------

#[test]
fn sse_frame_done_marker_returns_none() {
    let result = provider_stream_event_from_sse_frame("message_stop", "[DONE]");
    assert!(result.is_ok());
    assert!(
        result.unwrap().is_none(),
        "[DONE] must return Ok(None) regardless of event name"
    );
}

// ---------------------------------------------------------------------------
// 16. Unknown content_block type falls back to empty Thinking
// ---------------------------------------------------------------------------

#[test]
fn unknown_content_block_type_defaults_to_thinking() {
    let result = provider_stream_event_from_sse_frame(
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking"}}"#,
    );
    let event = result.unwrap().unwrap();
    match event {
        ProviderStreamEvent::ContentBlockStart { block, .. } => {
            assert!(
                matches!(block, ProviderContentBlockStart::Thinking { text, signature } if text.is_empty() && signature.is_none()),
                "unknown block type should fall back to empty Thinking"
            );
        }
        other => panic!("expected ContentBlockStart, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 17. OpenAI: reasoning_content populates thinking block
// ---------------------------------------------------------------------------

#[test]
fn openai_reasoning_content_creates_thinking_block() {
    let sse = concat!(
        "data: {\"id\":\"c4\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"let me think\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c4\",\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c4\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    let mut reader = OpenAiStreamReader::new("deepseek-r1".to_string());
    let mut acc = ProviderStreamAccumulator::new(ProviderId::OpenAi, None);
    for event in reader.push_chunk_events(sse.as_bytes()).unwrap() {
        acc.apply(&event);
    }
    for event in reader.finish_events().unwrap() {
        acc.apply(&event);
    }

    let response = acc.into_response();
    assert!(
        response.blocks.len() >= 2,
        "should have thinking + text blocks"
    );
    assert!(
        matches!(&response.blocks[0], TranscriptBlock::Thinking { text, .. } if text == "let me think")
    );
    assert!(matches!(&response.blocks[1], TranscriptBlock::Text { text } if text == "answer"));
}

// ---------------------------------------------------------------------------
// 18. Anthropic: plaintext event:error frames are classified, not generic
// ---------------------------------------------------------------------------

#[test]
fn anthropic_plaintext_error_event_is_classified() {
    let mut reader = AnthropicStreamReader::default();
    let error = reader
        .push_chunk_events(b"event: error\ndata: rate limit reached for requests\n\n")
        .expect_err("plaintext error frame should surface as provider error");

    assert_eq!(error.provider, Some(ProviderId::Anthropic));
    assert_eq!(error.category, StreamErrorCategory::RateLimit);
    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.message, "rate limit reached for requests");
    assert!(
        error
            .suggestion
            .as_deref()
            .unwrap_or_default()
            .contains("rate limited")
    );
}

// ---------------------------------------------------------------------------
// 19. OpenAI-compatible: plaintext event:error frames are classified
// ---------------------------------------------------------------------------

#[test]
fn openai_plaintext_error_event_is_classified() {
    let mut reader = OpenAiStreamReader::new("gpt-test".to_string());
    let error = reader
        .push_chunk_events(b"event: error\ndata: rate limit reached for requests\n\n")
        .expect_err("plaintext error frame should surface as provider error");

    assert_eq!(error.provider, Some(ProviderId::OpenAi));
    assert_eq!(error.category, StreamErrorCategory::RateLimit);
    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.message, "rate limit reached for requests");
    assert!(
        error
            .suggestion
            .as_deref()
            .unwrap_or_default()
            .contains("rate limited")
    );
}

// ---------------------------------------------------------------------------
// 20. OpenAI-compatible: JSON stream error code hints feed classification
// ---------------------------------------------------------------------------

#[test]
fn openai_json_error_code_hint_classifies_prompt_too_long() {
    let mut reader = OpenAiStreamReader::new("gpt-test".to_string());
    let error = reader
        .push_chunk_events(
            b"event: error\ndata: {\"error\":{\"code\":\"context_length_exceeded\",\"message\":\"request rejected\"}}\n\n",
        )
        .expect_err("OpenAI-compatible error frame should surface as provider error");

    assert_eq!(error.provider, Some(ProviderId::OpenAi));
    assert_eq!(error.category, StreamErrorCategory::PromptTooLong);
    assert_eq!(error.kind, ProviderErrorKind::Fatal);
    assert_eq!(error.message, "context_length_exceeded: request rejected");
}
