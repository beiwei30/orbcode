use std::collections::{BTreeMap, BTreeSet};

use orbcode_protocol::{ProviderId, TokenUsage};
use serde::Deserialize;
use uuid::Uuid;

use orbcode_protocol::StreamErrorCategory;

use crate::{
    ProviderContentBlockDelta, ProviderContentBlockStart, ProviderError, ProviderErrorKind,
    ProviderStreamEvent, classify_provider_error, sanitize_provider_error_message,
};

use super::decode_provider_stream_line;

// --- OpenAI SSE typed structs ---

#[derive(Deserialize)]
struct OpenAiSseChunk {
    #[serde(default)]
    choices: Option<Vec<OpenAiChoice>>,
    #[serde(default)]
    usage: Option<OpenAiUsagePayload>,
    #[serde(default)]
    error: Option<OpenAiStreamErrorPayload>,
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    #[serde(default)]
    delta: Option<OpenAiDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Deserialize)]
struct OpenAiToolCallDelta {
    #[serde(default)]
    index: Option<u64>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Deserialize)]
struct OpenAiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsagePayload {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct OpenAiStreamErrorPayload {
    #[serde(default, rename = "type")]
    error_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Clone, Debug)]
struct OpenAiToolBlock {
    content_index: usize,
}

#[derive(Debug)]
pub struct OpenAiStreamReader {
    model: String,
    pending_bytes: Vec<u8>,
    frame_event: String,
    frame_data: Vec<String>,
    adapter: OpenAiStreamAdapter,
    pending_error: Option<ProviderError>,
}

impl OpenAiStreamReader {
    pub fn new(model: String) -> Self {
        Self {
            model,
            pending_bytes: Vec::new(),
            frame_event: String::new(),
            frame_data: Vec::new(),
            adapter: OpenAiStreamAdapter::default(),
            pending_error: None,
        }
    }

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
            let line = decode_provider_stream_line(&line_bytes, ProviderId::OpenAi, "OpenAI")?;
            match self.consume_line_events(&line) {
                Ok(line_events) => events.extend(line_events),
                Err(error) => {
                    if events.is_empty() {
                        return Err(error);
                    }
                    self.pending_error = Some(error);
                    return Ok(events);
                }
            }
        }
        match self.flush_frame_events() {
            Ok(frame_events) => events.extend(frame_events),
            Err(error) => {
                if events.is_empty() {
                    return Err(error);
                }
                self.pending_error = Some(error);
                return Ok(events);
            }
        }
        events.extend(self.adapter.finish_events());
        Ok(events)
    }

    fn consume_complete_line_events(&mut self) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        let mut events = Vec::new();
        while let Some(newline_index) = self.pending_bytes.iter().position(|byte| *byte == b'\n') {
            let line_bytes = self
                .pending_bytes
                .drain(..=newline_index)
                .collect::<Vec<_>>();
            let line = decode_provider_stream_line(&line_bytes, ProviderId::OpenAi, "OpenAI")?;
            match self.consume_line_events(&line) {
                Ok(line_events) => events.extend(line_events),
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

    fn consume_line_events(
        &mut self,
        line: &str,
    ) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        if line.is_empty() {
            return self.flush_frame_events();
        }
        if let Some(event) = line.strip_prefix("event:") {
            self.frame_event = event.trim_start().to_string();
            return Ok(Vec::new());
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.frame_data.push(data.trim_start().to_string());
        }
        Ok(Vec::new())
    }

    fn flush_frame_events(&mut self) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        if self.frame_data.is_empty() {
            return Ok(Vec::new());
        }
        let event_name = std::mem::take(&mut self.frame_event);
        let data = self.frame_data.join("\n");
        self.frame_data.clear();
        if data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        let chunk = match serde_json::from_str::<OpenAiSseChunk>(&data) {
            Ok(chunk) => chunk,
            Err(_) if event_name == "error" => return Err(openai_stream_error_from_message(&data)),
            Err(error) => {
                return Err(ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    category: StreamErrorCategory::Other,
                    provider: Some(ProviderId::OpenAi),
                    status: None,
                    message: format!("OpenAI stream contained invalid JSON: {error}: {data}"),
                    suggestion: None,
                    rate_limit: None,
                });
            }
        };
        if event_name == "error"
            || chunk.error.is_some()
            || chunk.event_type.as_deref() == Some("error")
        {
            return Err(openai_stream_error(&chunk, &data));
        }
        self.adapter.events_from_chunk(&chunk, &self.model)
    }
}

fn openai_stream_error(chunk: &OpenAiSseChunk, raw: &str) -> ProviderError {
    let (error_type, message) = if let Some(ref err) = chunk.error {
        (
            err.error_type.as_deref().or(err.code.as_deref()),
            err.message
                .as_deref()
                .or(chunk.message.as_deref())
                .unwrap_or(raw),
        )
    } else {
        (
            chunk.event_type.as_deref(),
            chunk.message.as_deref().unwrap_or(raw),
        )
    };
    let message = match error_type {
        Some(et)
            if !message
                .to_ascii_lowercase()
                .contains(&et.to_ascii_lowercase()) =>
        {
            format!("{et}: {message}")
        }
        _ => message.to_string(),
    };
    openai_stream_error_from_message(&message)
}

fn openai_stream_error_from_message(message: &str) -> ProviderError {
    let message = sanitize_provider_error_message(message);
    let classified = classify_provider_error(Some(ProviderId::OpenAi), None, &message);
    ProviderError {
        kind: classified.kind,
        category: classified.category,
        provider: Some(ProviderId::OpenAi),
        status: None,
        message,
        suggestion: classified.suggestion,
        rate_limit: None,
    }
}

#[derive(Debug, Default)]
struct OpenAiStreamAdapter {
    started: bool,
    next_content_index: usize,
    thinking_block: Option<usize>,
    text_block: Option<usize>,
    tool_blocks: BTreeMap<u64, OpenAiToolBlock>,
    open_block_indices: BTreeSet<usize>,
    input_tokens: u32,
    cache_read_input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
    stopped: bool,
}

impl OpenAiStreamAdapter {
    fn events_from_chunk(
        &mut self,
        chunk: &OpenAiSseChunk,
        model: &str,
    ) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        self.update_usage(chunk.usage.as_ref());
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(ProviderStreamEvent::MessageStart {
                provider: ProviderId::OpenAi,
                fallback_from: None,
                usage: self.current_usage(false),
            });
        }

        let choice = chunk.choices.as_ref().and_then(|choices| choices.first());
        let Some(choice) = choice else {
            // OpenAI with `stream_options.include_usage` sends a trailing chunk
            // with `choices: []` carrying the final token counts under `usage`.
            // `update_usage` above captured them; emit a MessageDelta so the
            // accumulator records them — otherwise input/output totals stay 0
            // for the whole turn (the finish_reason chunk fired before usage
            // arrived). `merge_usage` replaces fields, so this never double-counts.
            if chunk.usage.is_some() {
                events.push(ProviderStreamEvent::MessageDelta {
                    stop_reason: None,
                    usage: self.current_usage(true),
                });
            }
            return Ok(events);
        };
        let delta = choice.delta.as_ref();

        if let Some(reasoning) = delta.and_then(|d| d.reasoning_content.as_deref())
            && !reasoning.is_empty()
        {
            self.ensure_thinking_block(&mut events);
            if let Some(index) = self.thinking_block {
                events.push(ProviderStreamEvent::ContentBlockDelta {
                    index,
                    delta: ProviderContentBlockDelta::Thinking(reasoning.to_string()),
                });
            }
        }

        if let Some(content) = delta.and_then(|d| d.content.as_deref())
            && !content.is_empty()
        {
            self.close_thinking_block(&mut events);
            self.ensure_text_block(&mut events);
            if let Some(index) = self.text_block {
                events.push(ProviderStreamEvent::ContentBlockDelta {
                    index,
                    delta: ProviderContentBlockDelta::Text(content.to_string()),
                });
            }
        }

        if let Some(tool_calls) = delta.and_then(|d| d.tool_calls.as_ref()) {
            if !tool_calls.is_empty() {
                self.close_thinking_block(&mut events);
                self.close_text_block(&mut events);
            }
            for tool_call in tool_calls {
                self.handle_tool_call(tool_call, &mut events);
            }
        }

        if let Some(finish_reason) = choice.finish_reason.as_deref() {
            self.close_all_blocks(&mut events);
            let stop_reason = if self.tool_blocks.is_empty() {
                map_openai_finish_reason(finish_reason)
            } else {
                "tool_use".to_string()
            };
            events.push(ProviderStreamEvent::MessageDelta {
                stop_reason: Some(stop_reason.clone()),
                usage: self.current_usage(true),
            });
            events.push(ProviderStreamEvent::MessageStop);
            self.stopped = true;
        }

        let _ = model;
        Ok(events)
    }

    fn finish_events(&mut self) -> Vec<ProviderStreamEvent> {
        if self.stopped {
            return Vec::new();
        }
        let mut events = Vec::new();
        self.close_all_blocks(&mut events);
        events
    }

    fn update_usage(&mut self, usage: Option<&OpenAiUsagePayload>) {
        let Some(usage) = usage else {
            return;
        };
        if let Some(input_tokens) = usage.prompt_tokens {
            self.input_tokens = input_tokens as u32;
        }
        if let Some(cached) = usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
        {
            self.cache_read_input_tokens = cached as u32;
        }
        if let Some(output_tokens) = usage.completion_tokens {
            self.output_tokens = output_tokens as u32;
        }
        if let Some(total_tokens) = usage.total_tokens {
            self.total_tokens = total_tokens as u32;
        }
    }

    fn current_usage(&self, include_output: bool) -> TokenUsage {
        let mut usage = TokenUsage {
            input_tokens: self
                .input_tokens
                .saturating_sub(self.cache_read_input_tokens),
            cache_read_input_tokens: self.cache_read_input_tokens,
            output_tokens: if include_output {
                self.output_tokens
            } else {
                0
            },
            total_tokens: self.total_tokens,
            ..TokenUsage::default()
        };
        if !include_output {
            usage.total_tokens = 0;
            usage.refresh_total_from_components();
        } else if usage.total_tokens == 0 {
            usage.refresh_total_from_components();
        }
        usage
    }

    fn ensure_thinking_block(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        if self.thinking_block.is_some() {
            return;
        }
        let index = self.next_index();
        self.thinking_block = Some(index);
        self.open_block_indices.insert(index);
        events.push(ProviderStreamEvent::ContentBlockStart {
            index,
            block: ProviderContentBlockStart::Thinking {
                text: String::new(),
                signature: Some(String::new()),
            },
        });
    }

    fn ensure_text_block(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        if self.text_block.is_some() {
            return;
        }
        let index = self.next_index();
        self.text_block = Some(index);
        self.open_block_indices.insert(index);
        events.push(ProviderStreamEvent::ContentBlockStart {
            index,
            block: ProviderContentBlockStart::Text {
                text: String::new(),
            },
        });
    }

    fn handle_tool_call(
        &mut self,
        tool_call: &OpenAiToolCallDelta,
        events: &mut Vec<ProviderStreamEvent>,
    ) {
        let tool_index = tool_call.index.unwrap_or(self.tool_blocks.len() as u64);
        if !self.tool_blocks.contains_key(&tool_index) {
            let index = self.next_index();
            let id = tool_call
                .id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map_or_else(
                    || format!("toolu_{}", Uuid::new_v4().simple()),
                    ToString::to_string,
                );
            let name = tool_call
                .function
                .as_ref()
                .and_then(|f| f.name.as_deref())
                .unwrap_or_default()
                .to_string();
            self.tool_blocks.insert(
                tool_index,
                OpenAiToolBlock {
                    content_index: index,
                },
            );
            self.open_block_indices.insert(index);
            events.push(ProviderStreamEvent::ContentBlockStart {
                index,
                block: ProviderContentBlockStart::ToolUse {
                    id,
                    name,
                    input: String::new(),
                },
            });
        }
        let Some(arguments) = tool_call
            .function
            .as_ref()
            .and_then(|f| f.arguments.as_deref())
        else {
            return;
        };
        if arguments.is_empty() {
            return;
        }
        if let Some(block) = self.tool_blocks.get(&tool_index) {
            events.push(ProviderStreamEvent::ContentBlockDelta {
                index: block.content_index,
                delta: ProviderContentBlockDelta::InputJson(arguments.to_string()),
            });
        }
    }

    fn close_thinking_block(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        if let Some(index) = self.thinking_block.take() {
            self.open_block_indices.remove(&index);
            events.push(ProviderStreamEvent::ContentBlockStop { index });
        }
    }

    fn close_text_block(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        if let Some(index) = self.text_block.take() {
            self.open_block_indices.remove(&index);
            events.push(ProviderStreamEvent::ContentBlockStop { index });
        }
    }

    fn close_all_blocks(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        self.close_thinking_block(events);
        self.close_text_block(events);
        for index in std::mem::take(&mut self.open_block_indices) {
            events.push(ProviderStreamEvent::ContentBlockStop { index });
        }
    }

    fn next_index(&mut self) -> usize {
        let index = self.next_content_index;
        self.next_content_index += 1;
        index
    }
}

fn map_openai_finish_reason(reason: &str) -> String {
    match reason {
        "tool_calls" => "tool_use",
        "length" => "max_tokens",
        "stop" | "content_filter" => "end_turn",
        _ => "end_turn",
    }
    .to_string()
}
