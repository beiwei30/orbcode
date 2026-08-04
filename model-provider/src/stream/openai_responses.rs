use std::collections::{BTreeSet, HashMap};

use orbcode_protocol::{ProviderId, TokenUsage};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    ProviderContentBlockDelta, ProviderContentBlockStart, ProviderError, ProviderStreamEvent,
    classify_provider_error, sanitize_provider_error_message,
};

use super::decode_stream_line;

#[derive(Debug, Deserialize)]
struct ResponsesStreamEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    response: Option<Value>,
    #[serde(default)]
    item: Option<Value>,
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    delta: Option<String>,
}

#[derive(Debug)]
struct ToolBlock {
    index: usize,
    arguments_seen: bool,
}

#[derive(Debug)]
pub struct OpenAiResponsesStreamReader {
    pending_bytes: Vec<u8>,
    frame_event: String,
    frame_data: Vec<String>,
    started: bool,
    completed: bool,
    next_index: usize,
    thinking_block: Option<usize>,
    text_block: Option<usize>,
    tool_blocks: HashMap<String, ToolBlock>,
    pending_tool_arguments: HashMap<String, Vec<String>>,
    open_blocks: BTreeSet<usize>,
}

impl Default for OpenAiResponsesStreamReader {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiResponsesStreamReader {
    pub fn new() -> Self {
        Self {
            pending_bytes: Vec::new(),
            frame_event: String::new(),
            frame_data: Vec::new(),
            started: false,
            completed: false,
            next_index: 0,
            thinking_block: None,
            text_block: None,
            tool_blocks: HashMap::new(),
            pending_tool_arguments: HashMap::new(),
            open_blocks: BTreeSet::new(),
        }
    }

    pub fn push_chunk_events(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        self.pending_bytes.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(newline_index) = self.pending_bytes.iter().position(|byte| *byte == b'\n') {
            let line_bytes = self
                .pending_bytes
                .drain(..=newline_index)
                .collect::<Vec<_>>();
            let line = decode_stream_line(&line_bytes)?;
            events.extend(self.consume_line(&line)?);
        }
        Ok(events)
    }

    pub fn finish_events(&mut self) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        let mut events = Vec::new();
        if !self.pending_bytes.is_empty() {
            let line = decode_stream_line(&std::mem::take(&mut self.pending_bytes))?;
            events.extend(self.consume_line(&line)?);
        }
        events.extend(self.flush_frame()?);
        if !self.completed {
            return Err(responses_error(
                "OpenAI Responses stream closed before response.completed",
            ));
        }
        Ok(events)
    }

    fn consume_line(&mut self, line: &str) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        if line.is_empty() {
            return self.flush_frame();
        }
        if let Some(event) = line.strip_prefix("event:") {
            self.frame_event = event.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data:") {
            self.frame_data.push(data.trim_start().to_string());
        }
        Ok(Vec::new())
    }

    fn flush_frame(&mut self) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        if self.frame_data.is_empty() {
            return Ok(Vec::new());
        }
        let event_name = std::mem::take(&mut self.frame_event);
        let data = self.frame_data.join("\n");
        self.frame_data.clear();
        if data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        let event = serde_json::from_str::<ResponsesStreamEvent>(&data).map_err(|error| {
            responses_error(&format!(
                "OpenAI Responses stream contained invalid JSON: {error}"
            ))
        })?;
        if event_name == "error" || event.kind == "error" {
            return Err(responses_error(&event_error_message(&data)));
        }
        self.events_from_event(event)
    }

    fn events_from_event(
        &mut self,
        event: ResponsesStreamEvent,
    ) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(ProviderStreamEvent::MessageStart {
                provider: ProviderId::OpenAi,
                fallback_from: None,
                usage: TokenUsage::default(),
            });
        }

        match event.kind.as_str() {
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = event.delta.filter(|delta| !delta.is_empty()) {
                    let index = self.ensure_thinking_block(&mut events);
                    events.push(ProviderStreamEvent::ContentBlockDelta {
                        index,
                        delta: ProviderContentBlockDelta::Thinking(delta),
                    });
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = event.delta.filter(|delta| !delta.is_empty()) {
                    self.close_thinking(&mut events);
                    let index = self.ensure_text_block(&mut events);
                    events.push(ProviderStreamEvent::ContentBlockDelta {
                        index,
                        delta: ProviderContentBlockDelta::Text(delta),
                    });
                }
            }
            "response.output_item.added" => {
                if let Some(item) = event.item.as_ref() {
                    self.handle_output_item(item, false, &mut events);
                }
            }
            "response.function_call_arguments.delta" => {
                let item_id = event
                    .item_id
                    .or(event.call_id)
                    .unwrap_or_else(|| "function-call".to_string());
                if let Some(delta) = event.delta.filter(|delta| !delta.is_empty()) {
                    if let Some(tool) = self.tool_blocks.get_mut(&item_id) {
                        tool.arguments_seen = true;
                        let index = tool.index;
                        events.push(ProviderStreamEvent::ContentBlockDelta {
                            index,
                            delta: ProviderContentBlockDelta::InputJson(delta),
                        });
                    } else {
                        self.pending_tool_arguments
                            .entry(item_id)
                            .or_default()
                            .push(delta);
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.item.as_ref() {
                    self.handle_output_item(item, true, &mut events);
                }
            }
            "response.completed" => {
                self.close_all(&mut events);
                let usage = event
                    .response
                    .as_ref()
                    .and_then(|response| response.get("usage"))
                    .map(responses_usage)
                    .unwrap_or_default();
                let stop_reason = if self.tool_blocks.is_empty() {
                    "end_turn"
                } else {
                    "tool_use"
                };
                events.push(ProviderStreamEvent::MessageDelta {
                    stop_reason: Some(stop_reason.to_string()),
                    usage,
                });
                events.push(ProviderStreamEvent::MessageStop);
                self.completed = true;
            }
            "response.failed" => {
                let message = event
                    .response
                    .as_ref()
                    .and_then(|response| response.get("error"))
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("OpenAI Responses request failed");
                return Err(responses_error(message));
            }
            "response.incomplete" => {
                let reason = event
                    .response
                    .as_ref()
                    .and_then(|response| response.get("incomplete_details"))
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(responses_error(&format!(
                    "OpenAI Responses request was incomplete: {reason}"
                )));
            }
            _ => {}
        }
        Ok(events)
    }

    fn handle_output_item(
        &mut self,
        item: &Value,
        done: bool,
        events: &mut Vec<ProviderStreamEvent>,
    ) {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                self.close_thinking(events);
                self.close_text(events);
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("call_id").and_then(Value::as_str))
                    .unwrap_or("function-call");
                let call_id = item.get("call_id").and_then(Value::as_str);
                let name = item.get("name").and_then(Value::as_str);
                let index = self.ensure_tool_block(item_id, call_id, name, events);
                self.flush_pending_tool_arguments(item_id, index, events);
                let arguments_seen = self
                    .tool_blocks
                    .get(item_id)
                    .is_some_and(|tool| tool.arguments_seen);
                if done
                    && !arguments_seen
                    && let Some(arguments) = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                {
                    events.push(ProviderStreamEvent::ContentBlockDelta {
                        index,
                        delta: ProviderContentBlockDelta::InputJson(arguments.to_string()),
                    });
                }
                if done {
                    self.close_block(index, events);
                }
            }
            Some("reasoning") => {
                let index = self.ensure_thinking_block(events);
                if done
                    && let Some(encrypted) = item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                {
                    events.push(ProviderStreamEvent::ContentBlockDelta {
                        index,
                        delta: ProviderContentBlockDelta::Signature(encrypted.to_string()),
                    });
                }
            }
            _ => {}
        }
    }

    fn ensure_thinking_block(&mut self, events: &mut Vec<ProviderStreamEvent>) -> usize {
        if let Some(index) = self.thinking_block {
            return index;
        }
        let index = self.next_block_index();
        self.thinking_block = Some(index);
        self.open_blocks.insert(index);
        events.push(ProviderStreamEvent::ContentBlockStart {
            index,
            block: ProviderContentBlockStart::Thinking {
                text: String::new(),
                signature: None,
            },
        });
        index
    }

    fn flush_pending_tool_arguments(
        &mut self,
        item_id: &str,
        index: usize,
        events: &mut Vec<ProviderStreamEvent>,
    ) {
        let Some(deltas) = self.pending_tool_arguments.remove(item_id) else {
            return;
        };
        if let Some(tool) = self.tool_blocks.get_mut(item_id) {
            tool.arguments_seen = true;
        }
        events.extend(
            deltas
                .into_iter()
                .map(|delta| ProviderStreamEvent::ContentBlockDelta {
                    index,
                    delta: ProviderContentBlockDelta::InputJson(delta),
                }),
        );
    }

    fn ensure_text_block(&mut self, events: &mut Vec<ProviderStreamEvent>) -> usize {
        if let Some(index) = self.text_block {
            return index;
        }
        let index = self.next_block_index();
        self.text_block = Some(index);
        self.open_blocks.insert(index);
        events.push(ProviderStreamEvent::ContentBlockStart {
            index,
            block: ProviderContentBlockStart::Text {
                text: String::new(),
            },
        });
        index
    }

    fn ensure_tool_block(
        &mut self,
        item_id: &str,
        call_id: Option<&str>,
        name: Option<&str>,
        events: &mut Vec<ProviderStreamEvent>,
    ) -> usize {
        if let Some(tool) = self.tool_blocks.get(item_id) {
            return tool.index;
        }
        let index = self.next_block_index();
        self.open_blocks.insert(index);
        self.tool_blocks.insert(
            item_id.to_string(),
            ToolBlock {
                index,
                arguments_seen: false,
            },
        );
        events.push(ProviderStreamEvent::ContentBlockStart {
            index,
            block: ProviderContentBlockStart::ToolUse {
                id: call_id.unwrap_or(item_id).to_string(),
                name: name.unwrap_or("tool").to_string(),
                input: String::new(),
            },
        });
        index
    }

    fn next_block_index(&mut self) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        index
    }

    fn close_thinking(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        if let Some(index) = self.thinking_block.take() {
            self.close_block(index, events);
        }
    }

    fn close_text(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        if let Some(index) = self.text_block.take() {
            self.close_block(index, events);
        }
    }

    fn close_block(&mut self, index: usize, events: &mut Vec<ProviderStreamEvent>) {
        if self.open_blocks.remove(&index) {
            events.push(ProviderStreamEvent::ContentBlockStop { index });
        }
    }

    fn close_all(&mut self, events: &mut Vec<ProviderStreamEvent>) {
        for index in std::mem::take(&mut self.open_blocks) {
            events.push(ProviderStreamEvent::ContentBlockStop { index });
        }
        self.thinking_block = None;
        self.text_block = None;
    }
}

fn responses_usage(value: &Value) -> TokenUsage {
    let input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    let cached = value
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    let output_tokens = value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    let total_tokens = value
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| u64::from(input_tokens) + u64::from(output_tokens))
        as u32;
    TokenUsage {
        input_tokens: input_tokens.saturating_sub(cached),
        cache_read_input_tokens: cached,
        output_tokens,
        total_tokens,
        ..TokenUsage::default()
    }
}

fn event_error_message(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .or_else(|| value.pointer("/error/message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "OpenAI Responses stream returned an error".to_string())
}

fn responses_error(message: &str) -> ProviderError {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_reasoning_tool_and_usage() {
        let mut reader = OpenAiResponsesStreamReader::new();
        let stream = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
            "event: response.reasoning_summary_text.delta\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"think\"}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"opaque\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"item-1\",\"call_id\":\"call-1\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"item-1\",\"delta\":\"{\\\"path\\\":\\\"a\\\"}\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"usage\":{\"input_tokens\":10,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens\":4,\"total_tokens\":14}}}\n\n"
        );
        let events = reader.push_chunk_events(stream.as_bytes()).expect("events");
        reader.finish_events().expect("completed");
        assert!(events.iter().any(|event| matches!(event, ProviderStreamEvent::ContentBlockDelta { delta: ProviderContentBlockDelta::Signature(value), .. } if value == "opaque")));
        assert!(events.iter().any(|event| matches!(event, ProviderStreamEvent::ContentBlockStart { block: ProviderContentBlockStart::ToolUse { id, name, .. }, .. } if id == "call-1" && name == "Read")));
        assert!(events.iter().any(|event| matches!(event, ProviderStreamEvent::MessageDelta { stop_reason: Some(reason), usage } if reason == "tool_use" && usage.input_tokens == 8 && usage.cache_read_input_tokens == 2)));
    }

    #[test]
    fn buffers_tool_arguments_until_function_call_metadata_arrives() {
        let mut reader = OpenAiResponsesStreamReader::new();
        let before_item = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"item-1\",\"delta\":\"{\\\"path\\\":\"}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"item-1\",\"delta\":\"\\\"a\\\"}\"}\n\n"
        );
        let events = reader
            .push_chunk_events(before_item.as_bytes())
            .expect("argument deltas");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            ProviderStreamEvent::MessageStart { .. }
        ));

        let item = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"item-1\",\"call_id\":\"call-1\",\"name\":\"Read\"}}\n\n"
        );
        let events = reader
            .push_chunk_events(item.as_bytes())
            .expect("function call item");
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::ContentBlockStart {
                block: ProviderContentBlockStart::ToolUse { id, name, .. },
                ..
            } if id == "call-1" && name == "Read"
        ));
        assert!(matches!(
            &events[1],
            ProviderStreamEvent::ContentBlockDelta {
                delta: ProviderContentBlockDelta::InputJson(delta),
                ..
            } if delta == "{\"path\":"
        ));
        assert!(matches!(
            &events[2],
            ProviderStreamEvent::ContentBlockDelta {
                delta: ProviderContentBlockDelta::InputJson(delta),
                ..
            } if delta == "\"a\"}"
        ));

        reader
            .push_chunk_events(b"data: {\"type\":\"response.completed\",\"response\":{}}\n\n")
            .expect("completed");
        reader.finish_events().expect("finished");
    }

    #[test]
    fn rejects_early_eof() {
        let mut reader = OpenAiResponsesStreamReader::new();
        reader
            .push_chunk_events(b"data: {\"type\":\"response.created\",\"response\":{}}\n\n")
            .expect("created");
        let error = reader.finish_events().expect_err("early EOF");
        assert!(error.message.contains("response.completed"));
    }
}
