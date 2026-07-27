mod anthropic;
mod disabled;
mod openai;

use orbcode_protocol::{MessageRole, ProviderId, TokenUsage, TranscriptBlock, TranscriptMessage};
use serde_json::{Value, json};
use tokio::task::yield_now;

use crate::{
    ModelProvider, ProviderCancellationToken, ProviderCompletion, ProviderContentBlockDelta,
    ProviderContentBlockStart, ProviderDescriptor, ProviderError, ProviderRequest,
    ProviderResponse, ProviderStreamEvent, ProviderStreamSink,
};

const MAX_STUB_TOOL_RESULT_PREVIEW_CHARS: usize = 2_000;

pub fn supported_providers() -> Vec<ProviderDescriptor> {
    vec![
        anthropic::AnthropicProvider.descriptor(),
        openai::OpenAiProvider.descriptor(),
    ]
}

pub fn is_provider_active(id: ProviderId) -> bool {
    id.is_active()
}

pub fn provider_for(id: ProviderId) -> Box<dyn ModelProvider> {
    match id {
        ProviderId::Anthropic => Box::new(anthropic::AnthropicProvider),
        ProviderId::OpenAi => Box::new(openai::OpenAiProvider),
        _ => Box::new(disabled::DisabledProvider(id)),
    }
}

// ---------------------------------------------------------------------------
// Shared stub helpers — used by real providers for `stub://` paths and tests.
// ---------------------------------------------------------------------------

fn stub_response(provider: ProviderId, title: &str, request: &ProviderRequest) -> ProviderResponse {
    if let Some(response) = stub_tool_response(provider, request) {
        return response;
    }

    let content = format!(
        "{title}\n\nsession: {}\nmodel: {}\ncontext: {}\ngit_status: {}\n\nprompt:\n{}\n\nThis response came from the local compatibility stub.",
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

    ProviderResponse {
        provider,
        fallback_from: None,
        blocks: vec![TranscriptBlock::Text {
            text: content.clone(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: TokenUsage::from_text(&request.prompt, &content),
        deltas: chunk_text(&content),
        content,
    }
}

async fn stream_stub_response(
    provider: ProviderId,
    title: &str,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    let response = stub_response(provider, title, request);
    stream_response_events(&response, sink, cancellation).await
}

async fn stream_response_events(
    response: &ProviderResponse,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    if cancellation.is_cancelled() {
        return Err(provider_interrupted_error());
    }
    let mut start_usage = response.usage.clone();
    start_usage.output_tokens = 0;
    start_usage.refresh_total_from_components();
    sink.emit(ProviderStreamEvent::MessageStart {
        provider: response.provider,
        fallback_from: response.fallback_from,
        usage: start_usage,
    })
    .await?;
    yield_now().await;

    for (index, block) in response.blocks.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(provider_interrupted_error());
        }
        match block {
            TranscriptBlock::Text { text } => {
                sink.emit(ProviderStreamEvent::ContentBlockStart {
                    index,
                    block: ProviderContentBlockStart::Text {
                        text: String::new(),
                    },
                })
                .await?;
                yield_now().await;
                for delta in if response.deltas.is_empty() {
                    chunk_text(text)
                } else {
                    response.deltas.clone()
                } {
                    if cancellation.is_cancelled() {
                        return Err(provider_interrupted_error());
                    }
                    sink.emit(ProviderStreamEvent::ContentBlockDelta {
                        index,
                        delta: ProviderContentBlockDelta::Text(delta),
                    })
                    .await?;
                    yield_now().await;
                }
            }
            TranscriptBlock::Thinking { text, signature } => {
                sink.emit(ProviderStreamEvent::ContentBlockStart {
                    index,
                    block: ProviderContentBlockStart::Thinking {
                        text: String::new(),
                        signature: signature.clone(),
                    },
                })
                .await?;
                yield_now().await;
                for delta in chunk_text(text) {
                    if cancellation.is_cancelled() {
                        return Err(provider_interrupted_error());
                    }
                    sink.emit(ProviderStreamEvent::ContentBlockDelta {
                        index,
                        delta: ProviderContentBlockDelta::Thinking(delta),
                    })
                    .await?;
                    yield_now().await;
                }
            }
            TranscriptBlock::ToolUse { id, name, input } => {
                sink.emit(ProviderStreamEvent::ContentBlockStart {
                    index,
                    block: ProviderContentBlockStart::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: String::new(),
                    },
                })
                .await?;
                yield_now().await;
                if !input.trim().is_empty() {
                    if cancellation.is_cancelled() {
                        return Err(provider_interrupted_error());
                    }
                    sink.emit(ProviderStreamEvent::ContentBlockDelta {
                        index,
                        delta: ProviderContentBlockDelta::InputJson(input.clone()),
                    })
                    .await?;
                    yield_now().await;
                }
            }
            _ => {}
        }
        if cancellation.is_cancelled() {
            return Err(provider_interrupted_error());
        }
        sink.emit(ProviderStreamEvent::ContentBlockStop { index })
            .await?;
        yield_now().await;
    }

    if cancellation.is_cancelled() {
        return Err(provider_interrupted_error());
    }
    sink.emit(ProviderStreamEvent::MessageDelta {
        stop_reason: response.stop_reason.clone(),
        usage: response.usage.clone(),
    })
    .await?;
    yield_now().await;
    sink.emit(ProviderStreamEvent::MessageStop).await?;

    Ok(ProviderCompletion {
        provider: response.provider,
        fallback_from: response.fallback_from,
        usage: response.usage.clone(),
        stop_reason: response.stop_reason.clone(),
    })
}

fn provider_interrupted_error() -> ProviderError {
    ProviderError::interrupted("provider stream interrupted")
}

fn stub_tool_response(provider: ProviderId, request: &ProviderRequest) -> Option<ProviderResponse> {
    let (completed_rounds, last_round) = completed_tool_rounds(&request.messages);

    if completed_rounds == 0 {
        let (tool_name, tool_input) = parse_stub_tool_directive(&request.prompt)?;
        let tool_use_id = format!("toolu-{}", request.session_id);
        return Some(ProviderResponse {
            provider,
            fallback_from: None,
            content: String::new(),
            blocks: vec![TranscriptBlock::ToolUse {
                id: tool_use_id,
                name: tool_name,
                input: tool_input,
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: TokenUsage::from_text(&request.prompt, ""),
            deltas: Vec::new(),
        });
    }

    let then_directives = parse_all_then_directives(&request.prompt);
    let then_index = completed_rounds - 1;
    if then_index < then_directives.len() {
        let (tool_name, tool_input) = &then_directives[then_index];
        let tool_use_id = format!("toolu-{}-{}", request.session_id, completed_rounds);
        return Some(ProviderResponse {
            provider,
            fallback_from: None,
            content: String::new(),
            blocks: vec![TranscriptBlock::ToolUse {
                id: tool_use_id,
                name: tool_name.clone(),
                input: tool_input.clone(),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: TokenUsage::from_text(&request.prompt, ""),
            deltas: Vec::new(),
        });
    }

    let (tool_use, tool_result) = last_round?;
    let content = stub_tool_result_response_content(&tool_use, &tool_result);
    Some(ProviderResponse {
        provider,
        fallback_from: None,
        blocks: vec![TranscriptBlock::Text {
            text: content.clone(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: TokenUsage::from_text(&request.prompt, &content),
        deltas: chunk_text(&content),
        content,
    })
}

fn stub_tool_result_response_content(
    tool_use: &ToolUseSnapshot,
    tool_result: &ToolResultSnapshot,
) -> String {
    let status = if tool_result.is_error {
        "failed"
    } else {
        "completed"
    };
    format!(
        "Tool `{}` {status}.\n\n{}",
        tool_use.name,
        stub_tool_result_preview(&tool_result.content)
    )
}

fn stub_tool_result_preview(content: &str) -> String {
    let total_chars = content.chars().count();
    if total_chars <= MAX_STUB_TOOL_RESULT_PREVIEW_CHARS {
        return content.to_string();
    }

    let head_chars = MAX_STUB_TOOL_RESULT_PREVIEW_CHARS / 2;
    let tail_chars = MAX_STUB_TOOL_RESULT_PREVIEW_CHARS - head_chars;
    let (head, tail, omitted) = split_preview_on_line_boundaries(content, head_chars, tail_chars);
    format!(
        "{head}\n\n[Stub tool result preview truncated for interactive responsiveness. Transcript retains the original tool result. Omitted {omitted} middle characters.]\n\n{tail}"
    )
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

fn completed_tool_rounds(
    messages: &[TranscriptMessage],
) -> (usize, Option<(ToolUseSnapshot, ToolResultSnapshot)>) {
    let mut count = 0;
    let mut last: Option<(ToolUseSnapshot, ToolResultSnapshot)> = None;
    let mut pending: Option<ToolUseSnapshot> = None;

    for message in messages {
        match message.role {
            MessageRole::Assistant => {
                for block in &message.blocks {
                    if let TranscriptBlock::ToolUse { id, name, input: _ } = block {
                        pending = Some(ToolUseSnapshot {
                            id: id.clone(),
                            name: name.clone(),
                        });
                    }
                }
            }
            MessageRole::User => {
                for block in &message.blocks {
                    if let TranscriptBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } = block
                        && let Some(ref tool_use) = pending
                        && tool_use.id == *tool_use_id
                    {
                        count += 1;
                        last = Some((
                            tool_use.clone(),
                            ToolResultSnapshot {
                                content: content.clone(),
                                is_error: *is_error,
                            },
                        ));
                        pending = None;
                    }
                }
            }
            _ => {}
        }
    }

    (count, last)
}

fn parse_all_then_directives(prompt: &str) -> Vec<(String, String)> {
    let marker = "#then:";
    let mut directives = Vec::new();
    let mut search_from = 0;

    while let Some(pos) = find_next_stub_directive_boundary(&prompt[search_from..], false) {
        let start = search_from + pos + marker.len();
        let rest = &prompt[start..];
        let trimmed = rest.trim_start();
        let skip = rest.len() - trimmed.len();
        let end = skip + directive_boundary(trimmed);
        let directive = rest[..end].trim();
        if !directive.is_empty()
            && let Some(parsed) = parse_tool_name_and_input(directive)
        {
            directives.push(parsed);
        }
        search_from = start + end;
    }

    directives
}

fn parse_tool_name_and_input(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let name_end = text.find(char::is_whitespace).unwrap_or(text.len());
    let name = text[..name_end].trim();
    if name.is_empty() {
        return None;
    }
    let input = text[name_end..].trim();
    let normalized = if input.is_empty() {
        "{}".to_string()
    } else if serde_json::from_str::<Value>(input).is_ok() {
        input.to_string()
    } else {
        json!({ "input": input }).to_string()
    };
    Some((name.to_string(), normalized))
}

#[derive(Clone)]
struct ToolUseSnapshot {
    id: String,
    name: String,
}

#[derive(Clone)]
struct ToolResultSnapshot {
    content: String,
    is_error: bool,
}

fn find_json_object_end(s: &str) -> Option<usize> {
    if !s.starts_with('{') {
        return None;
    }
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;
    for (i, ch) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            match ch {
                '\\' => escape_next = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn directive_boundary(text: &str) -> usize {
    let name_end = text.find(char::is_whitespace).unwrap_or(text.len());
    let after_name = text[name_end..].trim_start();
    let input_offset = text.len() - after_name.len();
    if let Some(json_len) = find_json_object_end(after_name) {
        return input_offset + json_len;
    }
    find_next_stub_directive_boundary(text, true).unwrap_or(text.len())
}

fn parse_stub_tool_directive(prompt: &str) -> Option<(String, String)> {
    let marker = "#tool:";
    let start = prompt.find(marker)? + marker.len();
    let rest = prompt[start..].trim_start();
    if rest.is_empty() {
        return None;
    }
    let boundary = directive_boundary(rest);
    let directive = rest[..boundary].trim();
    parse_tool_name_and_input(directive)
}

fn find_next_stub_directive_boundary(text: &str, include_tool_marker: bool) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => {
                escaped = true;
            }
            '"' => {
                in_string = !in_string;
            }
            '#' if !in_string => {
                let rest = &text[index..];
                if rest.starts_with("#then:")
                    || (include_tool_marker && index > 0 && rest.starts_with("#tool:"))
                {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}

fn chunk_text(text: &str) -> Vec<String> {
    const MAX_CHUNK: usize = 18;

    let mut chunks = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= MAX_CHUNK || ch == '\n' {
            chunks.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_then_extracts_single_directive() {
        let prompt = r#"#tool:bash {"command":"cd /tmp"} #then:bash {"command":"pwd"}"#;
        let directives = parse_all_then_directives(prompt);
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].0, "bash");
        assert!(directives[0].1.contains("pwd"));
    }

    #[test]
    fn parse_then_extracts_multiple_directives() {
        let prompt =
            r#"#tool:bash {"command":"a"} #then:bash {"command":"b"} #then:bash {"command":"c"}"#;
        let directives = parse_all_then_directives(prompt);
        assert_eq!(directives.len(), 2);
        assert!(directives[0].1.contains("\"b\""));
        assert!(directives[1].1.contains("\"c\""));
    }

    #[test]
    fn parse_then_returns_empty_when_absent() {
        let directives = parse_all_then_directives(r#"#tool:bash {"command":"cd /tmp"}"#);
        assert!(directives.is_empty());
    }

    #[test]
    fn parse_tool_directive_ignores_nested_markers_inside_json_strings() {
        let prompt = r##"#tool:Agent {"description":"Explore repo","prompt":"#tool:bash {\"command\":\"pwd\"}","subagent_type":"Explore"}"##;
        let (tool_name, tool_input) = parse_stub_tool_directive(prompt).expect("tool directive");

        assert_eq!(tool_name, "Agent");
        let parsed = serde_json::from_str::<Value>(&tool_input).expect("agent json");
        assert_eq!(parsed["description"], "Explore repo");
        assert_eq!(parsed["prompt"], r#"#tool:bash {"command":"pwd"}"#);
    }

    #[test]
    fn supported_providers_returns_only_active_providers() {
        let providers = supported_providers();
        let ids: Vec<ProviderId> = providers.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![ProviderId::Anthropic, ProviderId::OpenAi]);
    }

    #[test]
    fn is_provider_active_matches_supported_list() {
        assert!(is_provider_active(ProviderId::Anthropic));
        assert!(is_provider_active(ProviderId::OpenAi));
        assert!(!is_provider_active(ProviderId::Gemini));
        assert!(!is_provider_active(ProviderId::Grok));
    }

    #[test]
    fn provider_for_returns_correct_id() {
        assert_eq!(
            provider_for(ProviderId::Anthropic).id(),
            ProviderId::Anthropic
        );
        assert_eq!(provider_for(ProviderId::OpenAi).id(), ProviderId::OpenAi);
        assert_eq!(provider_for(ProviderId::Gemini).id(), ProviderId::Gemini);
        assert_eq!(provider_for(ProviderId::Grok).id(), ProviderId::Grok);
    }

    /// Invariant: `supported_providers()` descriptor IDs must be exactly
    /// `ProviderId::ACTIVE`. If someone adds a new active provider variant
    /// without wiring a real adapter in `supported_providers()`, or wires
    /// an adapter without adding it to `ACTIVE`, this test fails.
    #[test]
    fn provider_surface_matches_active_set() {
        let descriptor_ids: std::collections::HashSet<ProviderId> =
            supported_providers().into_iter().map(|d| d.id).collect();
        let active_ids: std::collections::HashSet<ProviderId> =
            ProviderId::ACTIVE.iter().copied().collect();

        assert_eq!(
            descriptor_ids, active_ids,
            "supported_providers() must return exactly the ProviderId::ACTIVE set.\n\
             Descriptors: {descriptor_ids:?}\n\
             ACTIVE: {active_ids:?}\n\
             If adding a new provider, update ProviderId::ACTIVE and add a \
             real adapter — do not add it as a disabled placeholder."
        );
    }

    /// Invariant: `provider_for()` must return a working adapter for every
    /// ACTIVE provider and a DisabledProvider for every DISABLED provider.
    /// The returned `id()` must match what was requested. This catches a
    /// new variant that falls through without a match arm.
    #[test]
    fn provider_for_covers_all_variants() {
        for id in ProviderId::ALL {
            let provider = provider_for(id);
            assert_eq!(
                provider.id(),
                id,
                "provider_for({id:?}).id() returned wrong id"
            );
        }
    }

    /// Invariant: DISABLED providers must use DisabledProvider (returning
    /// unsupported_provider errors), not a real adapter that silently
    /// accepts requests. This ensures Gemini/Grok stay inert.
    #[test]
    fn disabled_providers_not_in_supported_list() {
        let descriptor_ids: std::collections::HashSet<ProviderId> =
            supported_providers().into_iter().map(|d| d.id).collect();

        for id in ProviderId::DISABLED {
            assert!(
                !descriptor_ids.contains(&id),
                "disabled provider {id} must not appear in supported_providers()"
            );
        }
    }

    #[tokio::test]
    async fn disabled_provider_returns_unsupported_error_outside_mock() {
        use crate::{
            ProviderErrorKind, ProviderRequest, ProviderRequestOptions, ProviderStreamSink,
            StreamErrorCategory,
        };
        use orbcode_protocol::TurnContext;

        struct NullSink;
        #[async_trait::async_trait]
        impl ProviderStreamSink for NullSink {
            async fn emit(&mut self, _event: ProviderStreamEvent) -> Result<(), ProviderError> {
                Ok(())
            }
        }

        let request = ProviderRequest {
            session_id: "test".to_string(),
            prompt: "hello".to_string(),
            context: TurnContext::default(),
            messages: Vec::new(),
            system_prompt: String::new(),
            tools: Vec::new(),
            model: String::new(),
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: None,
            options: ProviderRequestOptions::default(),
        };

        for disabled_id in [ProviderId::Gemini, ProviderId::Grok] {
            let provider = provider_for(disabled_id);
            let mut sink = NullSink;
            let cancel = ProviderCancellationToken::from_flag(std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ));
            let result = provider.stream(&request, &mut sink, cancel).await;
            let err = result.expect_err("disabled provider should error");
            assert_eq!(err.kind, ProviderErrorKind::Fatal);
            assert_eq!(err.category, StreamErrorCategory::UnsupportedProvider);
            assert_eq!(err.provider, Some(disabled_id));
            assert!(
                err.message.contains("not supported"),
                "error should mention unsupported: {}",
                err.message
            );
            assert!(
                err.message.contains(disabled_id.as_str()),
                "error should name the provider: {}",
                err.message
            );
            assert!(
                err.suggestion.is_some(),
                "error should include a suggestion for disabled provider"
            );
        }
    }
}
