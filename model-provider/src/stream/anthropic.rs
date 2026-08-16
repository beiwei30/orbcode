use orbcode_protocol::{ProviderId, TokenUsage};
use serde::Deserialize;
use serde_json::Value;

use orbcode_protocol::StreamErrorCategory;

use crate::{
    ProviderContentBlockDelta, ProviderContentBlockStart, ProviderError, ProviderErrorKind,
    ProviderStreamEvent, classify_provider_error, sanitize_provider_error_message,
    suggestion_for_message,
};

#[cfg(test)]
use crate::ProviderStreamAccumulator;

use super::decode_stream_line;

// --- Anthropic SSE typed structs ---

#[derive(Deserialize)]
struct AnthropicSseFrame {
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    index: Option<u64>,
    #[serde(default)]
    message: Option<AnthropicMessagePayload>,
    #[serde(default)]
    delta: Option<AnthropicDelta>,
    #[serde(default)]
    content_block: Option<AnthropicContentBlock>,
    #[serde(default)]
    error: Option<AnthropicStreamErrorDetail>,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct AnthropicMessagePayload {
    #[serde(default)]
    usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(default, rename = "type")]
    block_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
}

#[derive(Deserialize)]
struct AnthropicDelta {
    #[serde(default, rename = "type")]
    delta_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicStreamErrorDetail {
    #[serde(default, rename = "type")]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn usage_from_optional(usage: Option<TokenUsage>) -> TokenUsage {
    match usage {
        Some(mut u) => {
            u.refresh_total_from_components();
            u
        }
        None => TokenUsage::default(),
    }
}

fn saturating_content_index(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

pub fn provider_stream_event_from_sse_frame(
    event_name: &str,
    data: &str,
) -> Result<Option<ProviderStreamEvent>, ProviderError> {
    if data == "[DONE]" {
        return Ok(None);
    }

    let frame = match serde_json::from_str::<AnthropicSseFrame>(data) {
        Ok(frame) => frame,
        Err(_) if event_name == "error" => {
            let message = sanitize_provider_error_message(data);
            return Err(stream_error_with_classification(None, &message));
        }
        Err(error) => {
            return Err(ProviderError {
                kind: ProviderErrorKind::Fatal,
                category: StreamErrorCategory::Other,
                provider: None,
                status: None,
                message: format!("invalid streaming frame: {error}"),
                suggestion: None,
                rate_limit: None,
            });
        }
    };

    let event = match frame.event_type.as_deref() {
        Some("message_start") => Some(ProviderStreamEvent::MessageStart {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: usage_from_optional(frame.message.and_then(|m| m.usage)),
        }),
        Some("message_delta") => Some(ProviderStreamEvent::MessageDelta {
            stop_reason: frame.delta.as_ref().and_then(|d| d.stop_reason.clone()),
            usage: usage_from_optional(frame.usage),
        }),
        Some("content_block_start") => {
            let Some(block) = frame.content_block else {
                return Ok(None);
            };
            let index = frame.index.map_or(usize::MAX, saturating_content_index);
            let content = match block.block_type.as_deref() {
                Some("text") => ProviderContentBlockStart::Text {
                    text: block.text.unwrap_or_default(),
                },
                Some("thinking") => ProviderContentBlockStart::Thinking {
                    text: block.thinking.unwrap_or_default(),
                    signature: block.signature.filter(|v| !v.is_empty()),
                },
                Some("tool_use") => ProviderContentBlockStart::ToolUse {
                    id: block.id.unwrap_or_else(|| "tool-use".to_string()),
                    name: block.name.unwrap_or_else(|| "tool".to_string()),
                    input: serialize_initial_tool_input(block.input.as_ref()),
                },
                _ => ProviderContentBlockStart::Thinking {
                    text: String::new(),
                    signature: None,
                },
            };
            Some(ProviderStreamEvent::ContentBlockStart {
                index,
                block: content,
            })
        }
        Some("content_block_delta") => {
            let index = frame.index.map_or(usize::MAX, saturating_content_index);
            let Some(delta) = frame.delta else {
                return Ok(None);
            };
            let delta = match delta.delta_type.as_deref() {
                Some("text_delta") => delta.text.map(ProviderContentBlockDelta::Text),
                Some("thinking_delta") => delta.thinking.map(ProviderContentBlockDelta::Thinking),
                Some("signature_delta") => {
                    delta.signature.map(ProviderContentBlockDelta::Signature)
                }
                Some("input_json_delta") => {
                    delta.partial_json.map(ProviderContentBlockDelta::InputJson)
                }
                _ => None,
            };
            delta.map(|delta| ProviderStreamEvent::ContentBlockDelta { index, delta })
        }
        Some("content_block_stop") => Some(ProviderStreamEvent::ContentBlockStop {
            index: frame.index.map_or(usize::MAX, saturating_content_index),
        }),
        Some("message_stop") => Some(ProviderStreamEvent::MessageStop),
        Some("error") => {
            let message = frame
                .error
                .as_ref()
                .and_then(|e| e.message.as_deref())
                .unwrap_or("unknown Anthropic stream error");
            let message = sanitize_provider_error_message(message);
            return Err(stream_error_with_classification(
                frame.error.as_ref(),
                &message,
            ));
        }
        _ => None,
    };

    if event_name == "error" {
        let message = sanitize_provider_error_message(data);
        return Err(stream_error_with_classification(None, &message));
    }

    Ok(event)
}

fn stream_error_with_classification(
    error: Option<&AnthropicStreamErrorDetail>,
    message: &str,
) -> ProviderError {
    let provider = Some(ProviderId::Anthropic);
    let mut classified = classify_provider_error(provider, None, message);
    if classified.category == StreamErrorCategory::Other
        && let Some(error_type) = error.and_then(|e| e.error_type.as_deref())
    {
        match error_type {
            "rate_limit_error" => {
                classified.category = StreamErrorCategory::RateLimit;
                classified.kind = ProviderErrorKind::Retryable;
            }
            "overloaded_error" => {
                classified.category = StreamErrorCategory::Overload;
                classified.kind = ProviderErrorKind::Retryable;
            }
            "authentication_error" | "permission_error" => {
                classified.category = StreamErrorCategory::Auth;
                classified.kind = ProviderErrorKind::Fatal;
            }
            "invalid_request_error" | "not_found_error" => {
                classified.category = StreamErrorCategory::InvalidRequest;
                classified.kind = ProviderErrorKind::Fatal;
            }
            _ => {}
        }
        if classified.category != StreamErrorCategory::Other {
            classified.suggestion = provider.map(|provider| {
                suggestion_for_message(provider, classified.category, None, message)
            });
        }
    }
    ProviderError {
        kind: classified.kind,
        category: classified.category,
        provider,
        status: None,
        message: message.to_string(),
        suggestion: classified.suggestion,
        rate_limit: None,
    }
}

#[derive(Default)]
pub struct AnthropicStreamReader {
    pending_bytes: Vec<u8>,
    frame_event: String,
    frame_data: Vec<String>,
    plain_output: String,
    next_block_index: usize,
    pending_error: Option<ProviderError>,
}

impl AnthropicStreamReader {
    pub fn push_chunk_events(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        if let Some(error) = self.pending_error.take() {
            return Err(error);
        }
        self.pending_bytes.extend_from_slice(chunk);
        self.consume_complete_line_events()
    }

    pub fn finish_events(&mut self) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        if let Some(error) = self.pending_error.take() {
            return Err(error);
        }
        let mut events = Vec::new();
        if !self.pending_bytes.is_empty() {
            let line_bytes = std::mem::take(&mut self.pending_bytes);
            let line = decode_stream_line(&line_bytes)?;
            match self.consume_line_event(&line) {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {}
                Err(error) => {
                    if events.is_empty() {
                        return Err(error);
                    }
                    self.pending_error = Some(error);
                    return Ok(events);
                }
            }
        }

        match self.flush_frame_event() {
            Ok(Some(event)) => events.push(event),
            Ok(None) => {}
            Err(error) => {
                if events.is_empty() {
                    return Err(error);
                }
                self.pending_error = Some(error);
            }
        }
        Ok(events)
    }

    pub fn plain_output(&self) -> &str {
        &self.plain_output
    }

    #[cfg(test)]
    pub(crate) fn push_chunk(
        &mut self,
        chunk: &[u8],
        accumulator: &mut ProviderStreamAccumulator,
    ) -> Result<(), ProviderError> {
        for event in self.push_chunk_events(chunk)? {
            accumulator.apply(&event);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn finish(
        &mut self,
        accumulator: &mut ProviderStreamAccumulator,
    ) -> Result<(), ProviderError> {
        for event in self.finish_events()? {
            accumulator.apply(&event);
        }
        Ok(())
    }

    fn consume_complete_line_events(&mut self) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        let mut events = Vec::new();
        while let Some(newline_index) = self.pending_bytes.iter().position(|byte| *byte == b'\n') {
            let line_bytes = self
                .pending_bytes
                .drain(..=newline_index)
                .collect::<Vec<_>>();
            let line = decode_stream_line(&line_bytes)?;
            match self.consume_line_event(&line) {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {}
                Err(error) => {
                    if events.is_empty() {
                        return Err(error);
                    }
                    self.pending_error = Some(error);
                    break;
                }
            }
        }
        Ok(events)
    }

    fn consume_line_event(
        &mut self,
        line: &str,
    ) -> Result<Option<ProviderStreamEvent>, ProviderError> {
        if line.is_empty() {
            return self.flush_frame_event();
        }

        if let Some(event) = line.strip_prefix("event:") {
            self.frame_event = event.trim().to_string();
            return Ok(None);
        }

        if let Some(data) = line.strip_prefix("data:") {
            self.frame_data.push(data.trim_start().to_string());
            return Ok(None);
        }

        if line.starts_with("id:") || line.starts_with("retry:") || line.starts_with(':') {
            return Ok(None);
        }

        if !self.plain_output.is_empty() {
            self.plain_output.push('\n');
        }
        self.plain_output.push_str(line);
        Ok(None)
    }

    fn flush_frame_event(&mut self) -> Result<Option<ProviderStreamEvent>, ProviderError> {
        if self.frame_data.is_empty() {
            self.frame_event.clear();
            return Ok(None);
        }

        let data = self.frame_data.join("\n");
        let event = provider_stream_event_from_sse_frame(&self.frame_event, &data)?
            .map(|event| self.normalize_event_index(event));
        self.frame_event.clear();
        self.frame_data.clear();
        Ok(event)
    }

    fn normalize_event_index(&mut self, event: ProviderStreamEvent) -> ProviderStreamEvent {
        match event {
            ProviderStreamEvent::ContentBlockStart { index, block } if index == usize::MAX => {
                let index = self.next_block_index;
                self.next_block_index += 1;
                ProviderStreamEvent::ContentBlockStart { index, block }
            }
            ProviderStreamEvent::ContentBlockStart { index, block } => {
                self.next_block_index = self.next_block_index.max(index.saturating_add(1));
                ProviderStreamEvent::ContentBlockStart { index, block }
            }
            ProviderStreamEvent::ContentBlockDelta { index, delta } if index == usize::MAX => {
                ProviderStreamEvent::ContentBlockDelta {
                    index: self.next_block_index.saturating_sub(1),
                    delta,
                }
            }
            ProviderStreamEvent::ContentBlockStop { index } if index == usize::MAX => {
                ProviderStreamEvent::ContentBlockStop {
                    index: self.next_block_index.saturating_sub(1),
                }
            }
            event => event,
        }
    }
}

fn serialize_block_payload(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if value.is_null() {
        return String::new();
    }
    serde_json::to_string_pretty(value)
        .or_else(|_| serde_json::to_string(value))
        .unwrap_or_default()
}

fn serialize_initial_tool_input(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if value.is_null() {
        return String::new();
    }
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        return String::new();
    }
    serialize_block_payload(Some(value))
}

#[cfg(test)]
mod numeric_tests {
    use super::saturating_content_index;

    #[test]
    fn content_index_saturates_to_platform_limit() {
        assert_eq!(saturating_content_index(u64::MAX), usize::MAX);
    }
}
