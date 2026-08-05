use std::collections::HashMap;

use chrono::Utc;
use orbcode_app_server::{
    McpServerConfig, MessageRole, PermissionMode, ProviderId, StreamEvent, TokenUsage,
    ToolUseCompletionKind, TranscriptBlock, TranscriptMessage,
};
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

pub const ORBCODE_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Typed wire-format payloads
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(tag = "type")]
enum DeltaKind {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum EventPayload {
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: DeltaKind },
    #[serde(rename = "tool_use_started")]
    ToolUseStarted {
        tool_use_id: String,
        tool_name: String,
    },
    #[serde(rename = "tool_use_completed")]
    ToolUseCompleted {
        tool_use_id: String,
        tool_name: String,
        kind: String,
    },
    #[serde(rename = "tool_progress")]
    ToolProgress {
        tool_use_id: String,
        tool_name: String,
        progress: Value,
    },
    #[serde(rename = "permission_request")]
    PermissionRequest {
        tool_use_id: String,
        tool_name: String,
        tool_input: Value,
        summary: String,
    },
    #[serde(rename = "permission_resolved")]
    PermissionResolved { request_id: String, kind: String },
    #[serde(rename = "error")]
    Error {
        message: String,
        provider: Option<String>,
        category: Option<String>,
        suggestion: Option<String>,
    },
    #[serde(rename = "turn_cancelled")]
    TurnCancelled { kind: String },
    #[serde(rename = "budget")]
    Budget {
        outcome: String,
        blocked: bool,
        total_cost_usd: f64,
        max_budget_usd: f64,
        pricing_known: bool,
    },
    #[serde(rename = "assistant_message_discarded")]
    AssistantMessageDiscarded {
        provider: String,
        fallback_provider: String,
        reason: String,
    },
}

#[derive(Serialize)]
struct StreamEventRecord {
    #[serde(rename = "type")]
    record_type: &'static str,
    sequence: u64,
    uuid: String,
    session_id: String,
    parent_tool_use_id: Option<String>,
    timestamp: String,
    event: Value,
}

#[derive(Serialize)]
struct MessageRecord {
    #[serde(rename = "type")]
    record_type: &'static str,
    sequence: u64,
    uuid: String,
    session_id: String,
    parent_tool_use_id: Option<String>,
    timestamp: String,
    message: Value,
}

#[derive(Serialize)]
struct McpServerPayload {
    name: String,
    status: String,
}

#[derive(Serialize)]
struct SystemInitRecord {
    #[serde(rename = "type")]
    record_type: &'static str,
    subtype: &'static str,
    sequence: u64,
    uuid: String,
    session_id: String,
    cwd: String,
    tools: Vec<String>,
    mcp_servers: Vec<McpServerPayload>,
    model: String,
    #[serde(rename = "permissionMode")]
    permission_mode: String,
    #[serde(rename = "apiKeySource")]
    api_key_source: &'static str,
    claude_code_version: &'static str,
    slash_commands: Vec<String>,
    output_style: &'static str,
    skills: Vec<String>,
    plugins: Vec<Value>,
    timestamp: String,
}

#[derive(Serialize)]
struct CompactBoundaryRecord {
    #[serde(rename = "type")]
    record_type: &'static str,
    subtype: &'static str,
    sequence: u64,
    uuid: String,
    session_id: String,
    timestamp: String,
    compact_metadata: CompactMetadataPayload,
}

#[derive(Serialize)]
struct CompactMetadataPayload {
    trigger: &'static str,
    duration_ms: u64,
    pre_messages: usize,
    post_messages: usize,
    provider_generated: bool,
    fallback_reason: Option<String>,
}

#[derive(Serialize)]
struct ResultRecord {
    #[serde(rename = "type")]
    record_type: &'static str,
    subtype: String,
    sequence: u64,
    uuid: String,
    session_id: String,
    timestamp: String,
    duration_ms: u64,
    duration_api_ms: u64,
    is_error: bool,
    num_turns: u32,
    stop_reason: Option<String>,
    total_cost_usd: f64,
    pricing_known: bool,
    usage: Value,
    #[serde(rename = "modelUsage")]
    model_usage: Value,
    permission_denials: Vec<PermissionDenialPayload>,
}

#[derive(Serialize)]
struct UsagePayload {
    input_tokens: u32,
    cache_creation_input_tokens: u32,
    cache_read_input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
    service_tier: Option<String>,
    server_tool_use: ServerToolUsePayload,
}

#[derive(Serialize)]
struct ServerToolUsePayload {
    web_search_requests: u32,
    web_fetch_requests: u32,
}

#[derive(Serialize)]
struct ModelUsageEntry {
    #[serde(rename = "inputTokens")]
    input_tokens: u64,
    #[serde(rename = "outputTokens")]
    output_tokens: u64,
    #[serde(rename = "cacheReadInputTokens")]
    cache_read_input_tokens: u64,
    #[serde(rename = "cacheCreationInputTokens")]
    cache_creation_input_tokens: u64,
    #[serde(rename = "webSearchRequests")]
    web_search_requests: u64,
    #[serde(rename = "costUSD")]
    cost_usd: f64,
    #[serde(rename = "contextWindow")]
    context_window: u64,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u64,
}

#[derive(Serialize)]
struct PermissionDenialPayload {
    tool_name: String,
    tool_use_id: String,
    tool_input: Value,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct InitMetadata {
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    pub tool_names: Vec<String>,
    pub mcp_servers: Vec<McpServerInfo>,
    pub permission_mode: PermissionMode,
}

#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub status: String,
}

impl From<&McpServerConfig> for McpServerInfo {
    fn from(server: &McpServerConfig) -> Self {
        Self {
            name: server.id.clone(),
            status: server.status.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PermissionDenial {
    pub tool_name: String,
    pub tool_use_id: String,
    pub tool_input: Value,
}

#[derive(Debug, Default)]
pub struct ModelUsageAggregate {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_search_requests: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default)]
pub struct CostFields {
    pub total_cost_usd: f64,
    pub pricing_known: bool,
    pub model_costs: Option<HashMap<String, f64>>,
}

// ---------------------------------------------------------------------------
// Emitter
// ---------------------------------------------------------------------------

pub struct StreamJsonEmitter {
    session_id: String,
    model_name: String,
    pending_tool_inputs: HashMap<String, Value>,
    pub permission_denials: Vec<PermissionDenial>,
    pub usage_by_model: HashMap<String, ModelUsageAggregate>,
    sequence: u64,
}

impl StreamJsonEmitter {
    pub fn new(session_id: impl Into<String>, model_name: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            model_name: model_name.into(),
            pending_tool_inputs: HashMap::new(),
            permission_denials: Vec::new(),
            usage_by_model: HashMap::new(),
            sequence: 0,
        }
    }

    fn next_sequence(&mut self) -> u64 {
        let seq = self.sequence;
        self.sequence += 1;
        seq
    }

    #[cfg(test)]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn build_system_init(&mut self, meta: &InitMetadata) -> Value {
        let seq = self.next_sequence();
        let mcp_servers: Vec<McpServerPayload> = meta
            .mcp_servers
            .iter()
            .map(|server| McpServerPayload {
                name: server.name.clone(),
                status: server.status.clone(),
            })
            .collect();
        serde_json::to_value(SystemInitRecord {
            record_type: "system",
            subtype: "init",
            sequence: seq,
            uuid: Uuid::new_v4().to_string(),
            session_id: meta.session_id.clone(),
            cwd: meta.cwd.clone(),
            tools: meta.tool_names.clone(),
            mcp_servers,
            model: meta.model.clone(),
            permission_mode: meta.permission_mode.as_str().to_string(),
            api_key_source: "user",
            claude_code_version: ORBCODE_VERSION,
            slash_commands: Vec::new(),
            output_style: "default",
            skills: Vec::new(),
            plugins: Vec::new(),
            timestamp: Utc::now().to_rfc3339(),
        })
        .expect("SystemInitRecord serialization is infallible")
    }

    pub fn process(&mut self, event: &StreamEvent) -> Vec<Value> {
        match event {
            StreamEvent::AssistantDelta { delta, .. } => {
                vec![self.build_stream_event(EventPayload::ContentBlockDelta {
                    delta: DeltaKind::Text {
                        text: delta.clone(),
                    },
                })]
            }
            StreamEvent::ThinkingDelta { delta, .. } => {
                vec![self.build_stream_event(EventPayload::ContentBlockDelta {
                    delta: DeltaKind::Thinking {
                        thinking: delta.clone(),
                    },
                })]
            }
            StreamEvent::ToolUseStarted {
                tool_use_id,
                tool_name,
                ..
            } => vec![self.build_stream_event(EventPayload::ToolUseStarted {
                tool_use_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
            })],
            StreamEvent::ToolUseCompleted {
                tool_use_id,
                tool_name,
                kind,
                ..
            } => {
                if matches!(kind, ToolUseCompletionKind::PermissionDenied) {
                    let tool_input = self
                        .pending_tool_inputs
                        .get(tool_use_id)
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.permission_denials.push(PermissionDenial {
                        tool_name: tool_name.clone(),
                        tool_use_id: tool_use_id.clone(),
                        tool_input,
                    });
                }
                vec![self.build_stream_event(EventPayload::ToolUseCompleted {
                    tool_use_id: tool_use_id.clone(),
                    tool_name: tool_name.clone(),
                    kind: kind.as_str().to_string(),
                })]
            }
            StreamEvent::ToolProgress {
                tool_use_id,
                tool_name,
                progress,
                ..
            } => vec![self.build_stream_event(EventPayload::ToolProgress {
                tool_use_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                progress: progress.clone(),
            })],
            StreamEvent::PermissionRequested { request } => {
                let input = parse_tool_input(&request.tool_input);
                self.pending_tool_inputs
                    .insert(request.tool_use_id.clone(), input.clone());
                vec![self.build_stream_event(EventPayload::PermissionRequest {
                    tool_use_id: request.tool_use_id.clone(),
                    tool_name: request.tool_name.clone(),
                    tool_input: input,
                    summary: request.summary(),
                })]
            }
            StreamEvent::PermissionResolved {
                request_id, kind, ..
            } => {
                // Denials are recorded uniformly when the tool use completes with
                // `ToolUseCompletionKind::PermissionDenied`, which fires for rule,
                // hook, and interactive denials alike. This event only carries a
                // fresh `request_id` (not the tool-use id), so it is emitted for
                // visibility but not used for denial accounting.
                vec![self.build_stream_event(EventPayload::PermissionResolved {
                    request_id: request_id.clone(),
                    kind: kind.as_str().to_string(),
                })]
            }
            StreamEvent::AssistantMessageCompleted {
                message,
                provider,
                usage,
                ..
            } => {
                self.record_usage(provider, usage);
                for block in &message.blocks {
                    if let TranscriptBlock::ToolUse { id, input, .. } = block {
                        self.pending_tool_inputs
                            .insert(id.clone(), parse_tool_input(input));
                    }
                }
                let seq = self.next_sequence();
                let api_message = build_api_assistant_message(message, provider, usage);
                vec![
                    serde_json::to_value(MessageRecord {
                        record_type: "assistant",
                        sequence: seq,
                        uuid: message.id.clone(),
                        session_id: self.session_id.clone(),
                        parent_tool_use_id: None,
                        timestamp: message.created_at.to_rfc3339(),
                        message: api_message,
                    })
                    .expect("MessageRecord serialization is infallible"),
                ]
            }
            StreamEvent::UserMessage { message } => {
                if !message_has_visible_content(message) {
                    return Vec::new();
                }
                let seq = self.next_sequence();
                vec![
                    serde_json::to_value(MessageRecord {
                        record_type: "user",
                        sequence: seq,
                        uuid: message.id.clone(),
                        session_id: self.session_id.clone(),
                        parent_tool_use_id: None,
                        timestamp: message.created_at.to_rfc3339(),
                        message: build_api_user_message(message),
                    })
                    .expect("MessageRecord serialization is infallible"),
                ]
            }
            StreamEvent::Error {
                message,
                provider,
                category,
                suggestion,
                ..
            } => vec![self.build_stream_event(EventPayload::Error {
                message: message.clone(),
                provider: provider.as_ref().map(std::string::ToString::to_string),
                category: category.map(|c| c.as_str().to_string()),
                suggestion: suggestion.clone(),
            })],
            StreamEvent::TurnCancelled { kind, .. } => {
                vec![self.build_stream_event(EventPayload::TurnCancelled {
                    kind: kind.as_str().to_string(),
                })]
            }
            StreamEvent::Budget {
                outcome,
                blocked,
                total_usd,
                max_budget_usd,
                pricing_known,
                ..
            } => vec![self.build_stream_event(EventPayload::Budget {
                outcome: outcome.as_str().to_string(),
                blocked: *blocked,
                total_cost_usd: *total_usd,
                max_budget_usd: *max_budget_usd,
                pricing_known: *pricing_known,
            })],
            StreamEvent::AssistantMessageDiscarded {
                provider,
                fallback_provider,
                reason,
                ..
            } => vec![
                self.build_stream_event(EventPayload::AssistantMessageDiscarded {
                    provider: provider.to_string(),
                    fallback_provider: fallback_provider.to_string(),
                    reason: reason.clone(),
                }),
            ],
            StreamEvent::ContextCompacted {
                duration_ms,
                original_message_count,
                compacted_message_count,
                provider_generated,
                fallback_reason,
                ..
            } => {
                let seq = self.next_sequence();
                vec![
                    serde_json::to_value(CompactBoundaryRecord {
                        record_type: "system",
                        subtype: "compact_boundary",
                        sequence: seq,
                        uuid: Uuid::new_v4().to_string(),
                        session_id: self.session_id.clone(),
                        timestamp: Utc::now().to_rfc3339(),
                        compact_metadata: CompactMetadataPayload {
                            trigger: "auto",
                            duration_ms: *duration_ms,
                            pre_messages: *original_message_count,
                            post_messages: *compacted_message_count,
                            provider_generated: *provider_generated,
                            fallback_reason: fallback_reason.clone(),
                        },
                    })
                    .expect("CompactBoundaryRecord serialization is infallible"),
                ]
            }
            _ => Vec::new(),
        }
    }

    pub fn build_result(
        &mut self,
        subtype: &str,
        is_error: bool,
        duration_ms: u64,
        duration_api_ms: u64,
        num_turns: u32,
        usage: &TokenUsage,
        cost: &CostFields,
        stop_reason: Option<&str>,
        result_text: Option<&str>,
        errors: &[String],
    ) -> Value {
        let seq = self.next_sequence();
        let mut model_usage = Map::new();
        let single_model = self.usage_by_model.len() == 1;
        for (model, agg) in &self.usage_by_model {
            // No per-model cost breakdown was supplied (the production caller
            // passes `model_costs: None`). For a single-model session the entire
            // session cost belongs to that model, so report the authoritative
            // `total_cost_usd` instead of the never-populated `agg.cost_usd`
            // (which left `costUSD` at 0). Cheap field reads, so evaluated eagerly.
            let fallback_cost = if single_model {
                cost.total_cost_usd
            } else {
                agg.cost_usd
            };
            let per_model_cost = cost
                .model_costs
                .as_ref()
                .and_then(|m| {
                    m.get(model).copied().or_else(|| {
                        // usage_by_model keys by provider string ("anthropic") while
                        // CostSummary keys by canonical model name ("claude-sonnet-4-6").
                        // For single-model sessions (the common case), use the sole entry.
                        (m.len() == 1 && single_model)
                            .then(|| m.values().next().copied())
                            .flatten()
                    })
                })
                .unwrap_or(fallback_cost);
            model_usage.insert(
                model.clone(),
                serde_json::to_value(ModelUsageEntry {
                    input_tokens: agg.input_tokens,
                    output_tokens: agg.output_tokens,
                    cache_read_input_tokens: agg.cache_read_input_tokens,
                    cache_creation_input_tokens: agg.cache_creation_input_tokens,
                    web_search_requests: agg.web_search_requests,
                    cost_usd: per_model_cost,
                    context_window: 0,
                    max_output_tokens: 0,
                })
                .expect("ModelUsageEntry serialization is infallible"),
            );
        }
        let denials: Vec<PermissionDenialPayload> = self
            .permission_denials
            .iter()
            .map(|denial| PermissionDenialPayload {
                tool_name: denial.tool_name.clone(),
                tool_use_id: denial.tool_use_id.clone(),
                tool_input: denial.tool_input.clone(),
            })
            .collect();

        let mut result = serde_json::to_value(ResultRecord {
            record_type: "result",
            subtype: subtype.to_string(),
            sequence: seq,
            uuid: Uuid::new_v4().to_string(),
            session_id: self.session_id.clone(),
            timestamp: Utc::now().to_rfc3339(),
            duration_ms,
            duration_api_ms,
            is_error,
            num_turns,
            stop_reason: stop_reason.map(std::string::ToString::to_string),
            total_cost_usd: cost.total_cost_usd,
            pricing_known: cost.pricing_known,
            usage: serialize_usage(usage),
            model_usage: Value::Object(model_usage),
            permission_denials: denials,
        })
        .expect("ResultRecord serialization is infallible");
        let object = result.as_object_mut().expect("result is object");
        if is_error {
            object.insert(
                "errors".to_string(),
                Value::Array(errors.iter().map(|e| Value::String(e.clone())).collect()),
            );
        } else if let Some(text) = result_text {
            object.insert("result".to_string(), Value::String(text.to_string()));
        } else {
            object.insert("result".to_string(), Value::String(String::new()));
        }
        result
    }

    fn build_stream_event(&mut self, payload: EventPayload) -> Value {
        let seq = self.next_sequence();
        let event =
            serde_json::to_value(payload).expect("EventPayload serialization is infallible");
        serde_json::to_value(StreamEventRecord {
            record_type: "stream_event",
            sequence: seq,
            uuid: Uuid::new_v4().to_string(),
            session_id: self.session_id.clone(),
            parent_tool_use_id: None,
            timestamp: Utc::now().to_rfc3339(),
            event,
        })
        .expect("StreamEventRecord serialization is infallible")
    }

    fn record_usage(&mut self, _provider: &ProviderId, usage: &TokenUsage) {
        let entry = self
            .usage_by_model
            .entry(self.model_name.clone())
            .or_default();
        entry.input_tokens += usage.input_tokens as u64;
        entry.output_tokens += usage.output_tokens as u64;
        entry.cache_read_input_tokens += usage.cache_read_input_tokens as u64;
        entry.cache_creation_input_tokens += usage.cache_creation_input_tokens as u64;
        entry.web_search_requests += usage.server_tool_use.web_search_requests as u64;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_tool_input(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn serialize_usage(usage: &TokenUsage) -> Value {
    serde_json::to_value(UsagePayload {
        input_tokens: usage.input_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        service_tier: usage.service_tier.clone(),
        server_tool_use: ServerToolUsePayload {
            web_search_requests: usage.server_tool_use.web_search_requests,
            web_fetch_requests: usage.server_tool_use.web_fetch_requests,
        },
    })
    .expect("UsagePayload serialization is infallible")
}

fn build_api_assistant_message(
    message: &TranscriptMessage,
    provider: &ProviderId,
    usage: &TokenUsage,
) -> Value {
    let content = blocks_to_content(&message.blocks, MessageRole::Assistant);
    let mut object = Map::new();
    object.insert("id".to_string(), Value::String(message.id.clone()));
    object.insert("type".to_string(), Value::String("message".to_string()));
    object.insert("role".to_string(), Value::String("assistant".to_string()));
    object.insert("content".to_string(), Value::Array(content));
    object.insert("model".to_string(), Value::String(provider.to_string()));
    object.insert(
        "stop_reason".to_string(),
        message
            .stop_reason
            .as_ref()
            .map_or(Value::Null, |s| Value::String(s.clone())),
    );
    object.insert("stop_sequence".to_string(), Value::Null);
    object.insert("usage".to_string(), serialize_usage(usage));
    Value::Object(object)
}

fn build_api_user_message(message: &TranscriptMessage) -> Value {
    let content = blocks_to_content(&message.blocks, MessageRole::User);
    let role = match message.role {
        MessageRole::Assistant => "assistant",
        _ => "user",
    };
    let mut object = Map::new();
    object.insert("role".to_string(), Value::String(role.to_string()));
    object.insert("content".to_string(), Value::Array(content));
    Value::Object(object)
}

fn blocks_to_content(blocks: &[TranscriptBlock], role: MessageRole) -> Vec<Value> {
    if blocks.is_empty() {
        return Vec::new();
    }
    blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::Text { text } => {
                let mut map = Map::new();
                map.insert("type".to_string(), Value::String("text".to_string()));
                map.insert("text".to_string(), Value::String(text.clone()));
                Some(Value::Object(map))
            }
            TranscriptBlock::Thinking { text, signature } => {
                let mut map = Map::new();
                map.insert("type".to_string(), Value::String("thinking".to_string()));
                map.insert("thinking".to_string(), Value::String(text.clone()));
                map.insert(
                    "signature".to_string(),
                    signature
                        .as_ref()
                        .map_or(Value::Null, |s| Value::String(s.clone())),
                );
                Some(Value::Object(map))
            }
            TranscriptBlock::ToolUse { id, name, input } => {
                let parsed_input = parse_tool_input(input);
                let mut map = Map::new();
                map.insert("type".to_string(), Value::String("tool_use".to_string()));
                map.insert("id".to_string(), Value::String(id.clone()));
                map.insert("name".to_string(), Value::String(name.clone()));
                map.insert("input".to_string(), parsed_input);
                Some(Value::Object(map))
            }
            TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                if matches!(role, MessageRole::Assistant) {
                    None
                } else {
                    let mut map = Map::new();
                    map.insert("type".to_string(), Value::String("tool_result".to_string()));
                    map.insert(
                        "tool_use_id".to_string(),
                        Value::String(tool_use_id.clone()),
                    );
                    map.insert("content".to_string(), Value::String(content.to_string()));
                    map.insert("is_error".to_string(), Value::Bool(*is_error));
                    Some(Value::Object(map))
                }
            }
            _ => None,
        })
        .collect()
}

fn message_has_visible_content(message: &TranscriptMessage) -> bool {
    if !message.content.is_empty() {
        return true;
    }
    message.blocks.iter().any(|block| match block {
        TranscriptBlock::Text { text } | TranscriptBlock::Thinking { text, .. } => !text.is_empty(),
        TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. } => true,
        _ => false,
    })
}

/// Build a successful `control_response` envelope for an SDK control request.
/// Delegates to [`orbcode_protocol::ControlResponseEnvelope::success`].
pub fn control_response_success(request_id: &str) -> Value {
    serde_json::to_value(orbcode_protocol::ControlResponseEnvelope::success(
        request_id,
    ))
    .expect("ControlResponseEnvelope serialization is infallible")
}

/// Build an error `control_response` envelope for an SDK control request.
/// Delegates to [`orbcode_protocol::ControlResponseEnvelope::error`].
pub fn control_response_error(request_id: &str, error: &str) -> Value {
    serde_json::to_value(orbcode_protocol::ControlResponseEnvelope::error(
        request_id, error,
    ))
    .expect("ControlResponseEnvelope serialization is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_app_server::{
        PermissionRequest, PermissionResolutionKind, ToolUseCompletionKind, TurnCancellationKind,
    };
    use orbcode_protocol::SessionId;
    use serde_json::json;

    fn session_id() -> SessionId {
        "session-123".to_string()
    }

    fn make_emitter() -> StreamJsonEmitter {
        StreamJsonEmitter::new(session_id(), "anthropic")
    }

    fn assistant_text(text: &str) -> TranscriptMessage {
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Text {
                text: text.to_string(),
            }],
        )
    }

    fn user_tool_result(tool_use_id: &str, content: &str, is_error: bool) -> TranscriptMessage {
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.into(),
                is_error,
                metadata: None,
            }],
        )
    }

    #[test]
    fn system_init_records_tools_and_mcp_servers_as_arrays() {
        let mut emitter = make_emitter();
        let meta = InitMetadata {
            session_id: emitter.session_id().to_string(),
            cwd: "/tmp/work".to_string(),
            model: "claude-opus".to_string(),
            tool_names: vec!["read".to_string(), "bash".to_string()],
            mcp_servers: vec![McpServerInfo {
                name: "context7".to_string(),
                status: "ready".to_string(),
            }],
            permission_mode: PermissionMode::AcceptEdits,
        };
        let value = emitter.build_system_init(&meta);
        assert_eq!(value["type"], "system");
        assert_eq!(value["subtype"], "init");
        assert_eq!(value["session_id"], "session-123");
        assert_eq!(value["model"], "claude-opus");
        assert_eq!(value["permissionMode"], "acceptEdits");
        assert_eq!(value["tools"], json!(["read", "bash"]));
        assert_eq!(
            value["mcp_servers"],
            json!([{"name": "context7", "status": "ready"}])
        );
        assert!(value["uuid"].is_string());
        assert!(value["timestamp"].is_string());
    }

    #[test]
    fn assistant_delta_emits_stream_event_text_delta() {
        let mut emitter = make_emitter();
        let event = StreamEvent::AssistantDelta {
            session_id: session_id(),
            delta: "hello".to_string(),
        };
        let records = emitter.process(&event);
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record["type"], "stream_event");
        assert_eq!(record["session_id"], "session-123");
        assert_eq!(record["event"]["type"], "content_block_delta");
        assert_eq!(record["event"]["delta"]["type"], "text_delta");
        assert_eq!(record["event"]["delta"]["text"], "hello");
        assert!(record["uuid"].is_string());
        assert_eq!(record["parent_tool_use_id"], Value::Null);
    }

    #[test]
    fn assistant_message_completed_emits_assistant_record_with_blocks() {
        let mut emitter = make_emitter();
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Text {
                    text: "Looking at this".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool_42".to_string(),
                    name: "Read".to_string(),
                    input: "{\"path\":\"/x.rs\"}".to_string(),
                },
            ],
        );
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            ..TokenUsage::default()
        };
        let records = emitter.process(&StreamEvent::AssistantMessageCompleted {
            message: message.clone(),
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: usage.clone(),
        });
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record["type"], "assistant");
        assert_eq!(record["session_id"], "session-123");
        assert_eq!(record["uuid"], message.id);
        assert_eq!(record["message"]["role"], "assistant");
        assert_eq!(record["message"]["model"], "anthropic");
        let content = record["message"]["content"].as_array().expect("content");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "tool_42");
        assert_eq!(content[1]["input"], json!({"path": "/x.rs"}));
        assert_eq!(record["message"]["usage"]["input_tokens"], 10);

        let agg = emitter.usage_by_model.get("anthropic").expect("agg");
        assert_eq!(agg.input_tokens, 10);
        assert_eq!(agg.output_tokens, 20);
    }

    #[test]
    fn user_tool_result_message_emits_tool_result_content() {
        let mut emitter = make_emitter();
        let message = user_tool_result("tool_42", "ok", false);
        let records = emitter.process(&StreamEvent::UserMessage {
            message: message.clone(),
        });
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record["type"], "user");
        assert_eq!(record["uuid"], message.id);
        let content = record["message"]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tool_42");
        assert_eq!(content[0]["is_error"], false);
    }

    fn assistant_tool_use(id: &str, name: &str, input: &str) -> TranscriptMessage {
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: input.to_string(),
            }],
        )
    }

    // A configured deny rule (or PreToolUse hook) short-circuits before the
    // interactive permission flow, so no PermissionRequested/Resolved events are
    // emitted. The denial must still be recorded from the terminal
    // ToolUseCompleted{PermissionDenied} event, with the tool input recovered
    // from the assistant tool_use block.
    #[test]
    fn rule_denial_records_tool_input_for_result_summary() {
        let mut emitter = make_emitter();
        let _ = emitter.process(&StreamEvent::AssistantMessageCompleted {
            message: assistant_tool_use("call_1", "Bash", "{\"command\":\"rm -rf /\"}"),
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: TokenUsage::default(),
        });

        let _ = emitter.process(&StreamEvent::ToolUseCompleted {
            session_id: session_id(),
            tool_use_id: "call_1".to_string(),
            tool_name: "Bash".to_string(),
            kind: ToolUseCompletionKind::PermissionDenied,
        });

        assert_eq!(emitter.permission_denials.len(), 1);
        let denial = &emitter.permission_denials[0];
        assert_eq!(denial.tool_name, "Bash");
        assert_eq!(denial.tool_use_id, "call_1");
        assert_eq!(denial.tool_input["command"], "rm -rf /");
    }

    // Interactive denials carry a fresh `request_id` that differs from the
    // tool-use id; recording keyed off ToolUseCompleted (tool-use id) must still
    // match the input registered by PermissionRequested.
    #[test]
    fn interactive_denial_records_tool_input_despite_distinct_request_id() {
        let mut emitter = make_emitter();
        let request = PermissionRequest {
            request_id: "req_uuid".to_string(),
            session_id: session_id(),
            tool_use_id: "call_2".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: "{\"command\":\"rm -rf /\"}".to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        };
        let _ = emitter.process(&StreamEvent::PermissionRequested { request });
        let _ = emitter.process(&StreamEvent::PermissionResolved {
            session_id: session_id(),
            request_id: "req_uuid".to_string(),
            kind: PermissionResolutionKind::Denied,
        });
        let _ = emitter.process(&StreamEvent::ToolUseCompleted {
            session_id: session_id(),
            tool_use_id: "call_2".to_string(),
            tool_name: "Bash".to_string(),
            kind: ToolUseCompletionKind::PermissionDenied,
        });

        assert_eq!(emitter.permission_denials.len(), 1);
        let denial = &emitter.permission_denials[0];
        assert_eq!(denial.tool_use_id, "call_2");
        assert_eq!(denial.tool_input["command"], "rm -rf /");
    }

    #[test]
    fn successful_tool_completion_does_not_record_denial() {
        let mut emitter = make_emitter();
        let _ = emitter.process(&StreamEvent::AssistantMessageCompleted {
            message: assistant_tool_use("call_ok", "Read", "{}"),
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: TokenUsage::default(),
        });
        let _ = emitter.process(&StreamEvent::ToolUseCompleted {
            session_id: session_id(),
            tool_use_id: "call_ok".to_string(),
            tool_name: "Read".to_string(),
            kind: ToolUseCompletionKind::Success,
        });
        assert!(emitter.permission_denials.is_empty());
    }

    #[test]
    fn tool_use_lifecycle_emits_paired_stream_events() {
        let mut emitter = make_emitter();
        let start = emitter.process(&StreamEvent::ToolUseStarted {
            session_id: session_id(),
            tool_use_id: "t1".to_string(),
            tool_name: "Read".to_string(),
            tool_input: String::new(),
        });
        let end = emitter.process(&StreamEvent::ToolUseCompleted {
            session_id: session_id(),
            tool_use_id: "t1".to_string(),
            tool_name: "Read".to_string(),
            kind: ToolUseCompletionKind::Success,
        });
        assert_eq!(start.len(), 1);
        assert_eq!(end.len(), 1);
        assert_eq!(start[0]["event"]["type"], "tool_use_started");
        assert_eq!(end[0]["event"]["type"], "tool_use_completed");
        assert_eq!(end[0]["event"]["kind"], "success");
    }

    #[test]
    fn turn_cancelled_emits_stream_event_but_no_assistant_record() {
        let mut emitter = make_emitter();
        emitter.process(&StreamEvent::AssistantDelta {
            session_id: session_id(),
            delta: "partial".to_string(),
        });
        let cancelled = emitter.process(&StreamEvent::TurnCancelled {
            session_id: session_id(),
            kind: TurnCancellationKind::AssistantStreaming,
            partial: Some(assistant_text("partial")),
            usage: None,
        });
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0]["type"], "stream_event");
        assert_eq!(cancelled[0]["event"]["type"], "turn_cancelled");
        assert_eq!(cancelled[0]["event"]["kind"], "assistant_streaming");
    }

    #[test]
    fn empty_user_message_is_suppressed() {
        let mut emitter = make_emitter();
        let mut empty = TranscriptMessage::new(MessageRole::User, "");
        empty.blocks.clear();
        let records = emitter.process(&StreamEvent::UserMessage { message: empty });
        assert!(records.is_empty());
    }

    #[test]
    fn build_result_success_carries_usage_and_denials() {
        let mut emitter = make_emitter();
        emitter.permission_denials.push(PermissionDenial {
            tool_name: "Bash".to_string(),
            tool_use_id: "tu1".to_string(),
            tool_input: json!({"command": "ls"}),
        });
        emitter
            .usage_by_model
            .entry("anthropic".to_string())
            .or_default()
            .input_tokens = 5;
        let usage = TokenUsage {
            input_tokens: 5,
            output_tokens: 7,
            total_tokens: 12,
            ..TokenUsage::default()
        };
        let cost = CostFields {
            total_cost_usd: 0.0042,
            pricing_known: true,
            model_costs: None,
        };
        let result = emitter.build_result(
            "success",
            false,
            100,
            80,
            1,
            &usage,
            &cost,
            Some("end_turn"),
            Some("done"),
            &[],
        );
        assert_eq!(result["subtype"], "success");
        assert_eq!(result["is_error"], false);
        assert_eq!(result["num_turns"], 1);
        assert_eq!(result["stop_reason"], "end_turn");
        assert_eq!(result["result"], "done");
        assert_eq!(result["total_cost_usd"], 0.0042);
        assert_eq!(result["pricing_known"], true);
        assert_eq!(result["usage"]["input_tokens"], 5);
        assert_eq!(result["usage"]["output_tokens"], 7);
        assert_eq!(result["modelUsage"]["anthropic"]["inputTokens"], 5);
        // Single-model session: per-model costUSD reflects the authoritative
        // total instead of the previously-always-0 `agg.cost_usd`.
        assert_eq!(result["modelUsage"]["anthropic"]["costUSD"], 0.0042);
        let denials = result["permission_denials"].as_array().expect("denials");
        assert_eq!(denials.len(), 1);
        assert_eq!(denials[0]["tool_name"], "Bash");
        assert!(result["errors"].is_null());
        assert!(result["uuid"].is_string());
    }

    #[test]
    fn error_event_emits_stream_event_only_no_assistant_record() {
        let mut emitter = make_emitter();
        emitter.process(&StreamEvent::AssistantDelta {
            session_id: session_id(),
            delta: "Reading...".to_string(),
        });
        let records = emitter.process(&StreamEvent::Error {
            session_id: Some(session_id()),
            provider: Some(ProviderId::Anthropic),
            category: None,
            message: "rate limited".to_string(),
            suggestion: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "stream_event");
        assert_eq!(records[0]["event"]["type"], "error");
        assert_eq!(records[0]["event"]["message"], "rate limited");
    }

    #[test]
    fn assistant_message_discarded_does_not_emit_assistant_record() {
        let mut emitter = make_emitter();
        let records = emitter.process(&StreamEvent::AssistantMessageDiscarded {
            session_id: session_id(),
            provider: ProviderId::Anthropic,
            fallback_provider: ProviderId::OpenAi,
            reason: "stream closed".to_string(),
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "stream_event");
        assert_eq!(records[0]["event"]["type"], "assistant_message_discarded");
        assert_eq!(records[0]["event"]["fallback_provider"], "openai");
    }

    fn normalize(records: &[Value]) -> Vec<Value> {
        records
            .iter()
            .map(|record| {
                let mut value = record.clone();
                strip_volatile(&mut value);
                value
            })
            .collect()
    }

    fn strip_volatile(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if map.contains_key("uuid") {
                    map.insert("uuid".to_string(), Value::String("<uuid>".to_string()));
                }
                if map.contains_key("timestamp") {
                    map.insert(
                        "timestamp".to_string(),
                        Value::String("<timestamp>".to_string()),
                    );
                }
                map.remove("sequence");
                for child in map.values_mut() {
                    strip_volatile(child);
                }
            }
            Value::Array(items) => {
                for child in items.iter_mut() {
                    strip_volatile(child);
                }
            }
            _ => {}
        }
    }

    fn run_sequence(emitter: &mut StreamJsonEmitter, events: Vec<StreamEvent>) -> Vec<Value> {
        let mut records = Vec::new();
        for event in events {
            records.extend(emitter.process(&event));
        }
        normalize(&records)
    }

    fn fixture_assistant_text() -> TranscriptMessage {
        let mut message = assistant_text("hello world");
        message.id = "msg-fixed".to_string();
        message.created_at = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 5, 28, 0, 0, 0)
            .single()
            .unwrap();
        message
    }

    #[test]
    fn golden_fixture_simple_text_turn() {
        let mut emitter = make_emitter();
        let records = run_sequence(
            &mut emitter,
            vec![
                StreamEvent::AssistantDelta {
                    session_id: session_id(),
                    delta: "hello".to_string(),
                },
                StreamEvent::AssistantDelta {
                    session_id: session_id(),
                    delta: " world".to_string(),
                },
                StreamEvent::AssistantMessageCompleted {
                    message: fixture_assistant_text(),
                    provider: ProviderId::Anthropic,
                    fallback_from: None,
                    usage: TokenUsage {
                        input_tokens: 4,
                        output_tokens: 2,
                        total_tokens: 6,
                        ..TokenUsage::default()
                    },
                },
                StreamEvent::TurnFinished {
                    session_id: session_id(),
                    provider: ProviderId::Anthropic,
                    fallback_from: None,
                    usage: TokenUsage::default(),
                },
            ],
        );
        let expected = json!([
            {
                "type": "stream_event",
                "uuid": "<uuid>",
                "session_id": "session-123",
                "parent_tool_use_id": null,
                "timestamp": "<timestamp>",
                "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "hello"}},
            },
            {
                "type": "stream_event",
                "uuid": "<uuid>",
                "session_id": "session-123",
                "parent_tool_use_id": null,
                "timestamp": "<timestamp>",
                "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": " world"}},
            },
            {
                "type": "assistant",
                "uuid": "<uuid>",
                "session_id": "session-123",
                "parent_tool_use_id": null,
                "timestamp": "<timestamp>",
                "message": {
                    "id": "msg-fixed",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "hello world"}],
                    "model": "anthropic",
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {
                        "input_tokens": 4,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "output_tokens": 2,
                        "total_tokens": 6,
                        "service_tier": null,
                        "server_tool_use": {"web_search_requests": 0, "web_fetch_requests": 0},
                    },
                },
            }
        ]);
        assert_eq!(Value::Array(records), expected);
    }

    #[test]
    fn golden_fixture_tool_use_round_trip() {
        let mut emitter = make_emitter();
        let mut tool_msg = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tu-1".to_string(),
                name: "Read".to_string(),
                input: "{\"path\":\"/x\"}".to_string(),
            }],
        );
        tool_msg.id = "asst-1".to_string();
        let mut result_msg = user_tool_result("tu-1", "file body", false);
        result_msg.id = "user-1".to_string();
        let records = run_sequence(
            &mut emitter,
            vec![
                StreamEvent::ToolUseStarted {
                    session_id: session_id(),
                    tool_use_id: "tu-1".to_string(),
                    tool_name: "Read".to_string(),
                    tool_input: String::new(),
                },
                StreamEvent::AssistantMessageCompleted {
                    message: tool_msg,
                    provider: ProviderId::Anthropic,
                    fallback_from: None,
                    usage: TokenUsage::default(),
                },
                StreamEvent::ToolUseCompleted {
                    session_id: session_id(),
                    tool_use_id: "tu-1".to_string(),
                    tool_name: "Read".to_string(),
                    kind: ToolUseCompletionKind::Success,
                },
                StreamEvent::UserMessage {
                    message: result_msg,
                },
            ],
        );
        assert_eq!(records[0]["event"]["type"], "tool_use_started");
        assert_eq!(records[1]["type"], "assistant");
        assert_eq!(records[1]["message"]["content"][0]["type"], "tool_use");
        assert_eq!(
            records[1]["message"]["content"][0]["input"],
            json!({"path": "/x"})
        );
        assert_eq!(records[2]["event"]["type"], "tool_use_completed");
        assert_eq!(records[2]["event"]["kind"], "success");
        assert_eq!(records[3]["type"], "user");
        assert_eq!(records[3]["message"]["content"][0]["type"], "tool_result");
        assert_eq!(records[3]["message"]["content"][0]["tool_use_id"], "tu-1");
    }

    #[test]
    fn golden_fixture_cancellation_emits_no_assistant_record() {
        let mut emitter = make_emitter();
        let records = run_sequence(
            &mut emitter,
            vec![
                StreamEvent::AssistantDelta {
                    session_id: session_id(),
                    delta: "thinking".to_string(),
                },
                StreamEvent::TurnCancelled {
                    session_id: session_id(),
                    kind: TurnCancellationKind::AssistantStreaming,
                    partial: Some(assistant_text("thinking")),
                    usage: None,
                },
            ],
        );
        assert!(records.iter().all(|r| r["type"] != "assistant"));
        assert_eq!(records[0]["event"]["type"], "content_block_delta");
        assert_eq!(records[1]["event"]["type"], "turn_cancelled");
        assert_eq!(records[1]["event"]["kind"], "assistant_streaming");
    }

    #[test]
    fn golden_fixture_model_error_does_not_emit_assistant_record() {
        let mut emitter = make_emitter();
        let records = run_sequence(
            &mut emitter,
            vec![
                StreamEvent::AssistantDelta {
                    session_id: session_id(),
                    delta: "trying".to_string(),
                },
                StreamEvent::Error {
                    session_id: Some(session_id()),
                    provider: Some(ProviderId::Anthropic),
                    category: None,
                    message: "stream interrupted".to_string(),
                    suggestion: None,
                },
            ],
        );
        assert!(records.iter().all(|r| r["type"] != "assistant"));
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["event"]["type"], "error");
        assert_eq!(records[1]["event"]["provider"], "anthropic");
    }

    #[test]
    fn golden_fixture_permission_deny_records_denial_for_result() {
        let mut emitter = make_emitter();
        let _ = run_sequence(
            &mut emitter,
            vec![
                StreamEvent::AssistantMessageCompleted {
                    message: assistant_tool_use("call-1", "Bash", "{\"command\":\"rm -rf /\"}"),
                    provider: ProviderId::Anthropic,
                    fallback_from: None,
                    usage: TokenUsage::default(),
                },
                StreamEvent::ToolUseCompleted {
                    session_id: session_id(),
                    tool_use_id: "call-1".to_string(),
                    tool_name: "Bash".to_string(),
                    kind: ToolUseCompletionKind::PermissionDenied,
                },
            ],
        );
        let usage = TokenUsage::default();
        let result = emitter.build_result(
            "error_during_execution",
            true,
            10,
            5,
            1,
            &usage,
            &CostFields::default(),
            Some("permission_denied"),
            None,
            &["tool denied".to_string()],
        );
        let denials = result["permission_denials"].as_array().expect("denials");
        assert_eq!(denials.len(), 1);
        assert_eq!(denials[0]["tool_name"], "Bash");
        assert_eq!(denials[0]["tool_input"]["command"], "rm -rf /");
        assert_eq!(result["errors"], json!(["tool denied"]));
    }

    #[test]
    fn golden_fixture_resume_carries_session_id_into_events() {
        let resume_id = "resume-abc".to_string();
        let mut emitter = StreamJsonEmitter::new(resume_id.clone(), "anthropic");
        let records = run_sequence(
            &mut emitter,
            vec![StreamEvent::AssistantDelta {
                session_id: resume_id.clone(),
                delta: "continuing".to_string(),
            }],
        );
        assert_eq!(records[0]["session_id"], resume_id);
        let usage = TokenUsage::default();
        let result = emitter.build_result(
            "success",
            false,
            0,
            0,
            0,
            &usage,
            &CostFields::default(),
            Some("end_turn"),
            Some(""),
            &[],
        );
        assert_eq!(result["session_id"], resume_id);
    }

    #[test]
    fn build_result_error_carries_errors_not_text() {
        let mut emitter = make_emitter();
        let usage = TokenUsage::default();
        let errors = vec!["model exploded".to_string()];
        let result = emitter.build_result(
            "error_during_execution",
            true,
            42,
            42,
            1,
            &usage,
            &CostFields::default(),
            Some("error"),
            None,
            &errors,
        );
        assert_eq!(result["subtype"], "error_during_execution");
        assert_eq!(result["is_error"], true);
        assert_eq!(result["errors"], json!(["model exploded"]));
        assert!(result["result"].is_null());
    }

    #[test]
    fn build_result_unknown_model_has_pricing_known_false() {
        let mut emitter = make_emitter();
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            ..TokenUsage::default()
        };
        let cost = CostFields {
            total_cost_usd: 0.0,
            pricing_known: false,
            model_costs: None,
        };
        let result = emitter.build_result(
            "success",
            false,
            10,
            5,
            1,
            &usage,
            &cost,
            Some("end_turn"),
            Some("hi"),
            &[],
        );
        assert_eq!(result["pricing_known"], false);
        assert_eq!(result["total_cost_usd"], 0.0);
    }

    #[test]
    fn build_result_propagates_per_model_cost_from_cost_fields() {
        let mut emitter = make_emitter();
        emitter
            .usage_by_model
            .entry("claude-sonnet-4-6".to_string())
            .or_default()
            .input_tokens = 1000;
        emitter
            .usage_by_model
            .entry("claude-sonnet-4-6".to_string())
            .or_default()
            .output_tokens = 500;
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            total_tokens: 1500,
            ..TokenUsage::default()
        };
        let mut model_costs = HashMap::new();
        model_costs.insert("claude-sonnet-4-6".to_string(), 0.0123);
        let cost = CostFields {
            total_cost_usd: 0.0123,
            pricing_known: true,
            model_costs: Some(model_costs),
        };
        let result = emitter.build_result(
            "success",
            false,
            50,
            40,
            1,
            &usage,
            &cost,
            Some("end_turn"),
            Some("done"),
            &[],
        );
        assert_eq!(result["pricing_known"], true);
        assert_eq!(result["total_cost_usd"], 0.0123);
        assert_eq!(result["modelUsage"]["claude-sonnet-4-6"]["costUSD"], 0.0123);
        assert_eq!(
            result["modelUsage"]["claude-sonnet-4-6"]["inputTokens"],
            1000
        );
    }

    #[test]
    fn build_result_model_cost_sum_equals_total() {
        let mut emitter = make_emitter();
        emitter
            .usage_by_model
            .entry("model-a".to_string())
            .or_default()
            .input_tokens = 100;
        emitter
            .usage_by_model
            .entry("model-b".to_string())
            .or_default()
            .input_tokens = 200;
        let mut model_costs = HashMap::new();
        model_costs.insert("model-a".to_string(), 0.005);
        model_costs.insert("model-b".to_string(), 0.010);
        let cost = CostFields {
            total_cost_usd: 0.015,
            pricing_known: true,
            model_costs: Some(model_costs),
        };
        let result = emitter.build_result(
            "success",
            false,
            10,
            5,
            1,
            &TokenUsage::default(),
            &cost,
            Some("end_turn"),
            Some(""),
            &[],
        );
        let model_usage = result["modelUsage"].as_object().unwrap();
        let cost_sum: f64 = model_usage
            .values()
            .map(|v| v["costUSD"].as_f64().unwrap())
            .sum();
        assert!(
            (cost_sum - 0.015).abs() < 1e-10,
            "sum of per-model costs ({cost_sum}) should equal total (0.015)"
        );
    }

    #[test]
    fn build_result_resolves_cost_when_keys_mismatch() {
        // usage_by_model keyed by provider string, model_costs keyed by model name
        let mut emitter = make_emitter();
        emitter
            .usage_by_model
            .entry("anthropic".to_string())
            .or_default()
            .input_tokens = 1000;
        let mut model_costs = HashMap::new();
        model_costs.insert("claude-sonnet-4-6".to_string(), 0.05);
        let cost = CostFields {
            total_cost_usd: 0.05,
            pricing_known: true,
            model_costs: Some(model_costs),
        };
        let result = emitter.build_result(
            "success",
            false,
            10,
            5,
            1,
            &TokenUsage::default(),
            &cost,
            Some("end_turn"),
            Some("hi"),
            &[],
        );
        assert_eq!(
            result["modelUsage"]["anthropic"]["costUSD"], 0.05,
            "single-entry fallback should resolve mismatched keys"
        );
    }

    #[test]
    fn control_response_success_matches_sdk_shape() {
        let response = control_response_success("req-1");
        assert_eq!(response["type"], "control_response");
        assert_eq!(response["response"]["subtype"], "success");
        assert_eq!(response["response"]["request_id"], "req-1");
        assert!(
            response["response"].get("error").is_none(),
            "success response must not carry an error field"
        );
    }

    #[test]
    fn control_response_error_carries_request_id_and_message() {
        let response = control_response_error("req-2", "unsupported control request");
        assert_eq!(response["type"], "control_response");
        assert_eq!(response["response"]["subtype"], "error");
        assert_eq!(response["response"]["request_id"], "req-2");
        assert_eq!(response["response"]["error"], "unsupported control request");
    }

    #[test]
    fn sequence_numbers_are_monotonically_increasing() {
        let mut emitter = make_emitter();
        let meta = InitMetadata {
            session_id: session_id(),
            cwd: "/tmp".to_string(),
            model: "claude-test".to_string(),
            tool_names: vec![],
            mcp_servers: vec![],
            permission_mode: PermissionMode::Default,
        };
        let init = emitter.build_system_init(&meta);
        assert_eq!(init["sequence"], 0);

        let events = vec![
            StreamEvent::AssistantDelta {
                session_id: session_id(),
                delta: "hello".to_string(),
            },
            StreamEvent::AssistantMessageCompleted {
                message: fixture_assistant_text(),
                provider: ProviderId::Anthropic,
                fallback_from: None,
                usage: TokenUsage::default(),
            },
            StreamEvent::TurnFinished {
                session_id: session_id(),
                provider: ProviderId::Anthropic,
                fallback_from: None,
                usage: TokenUsage::default(),
            },
        ];
        let mut records = Vec::new();
        for event in events {
            records.extend(emitter.process(&event));
        }
        let result = emitter.build_result(
            "success",
            false,
            0,
            0,
            1,
            &TokenUsage::default(),
            &CostFields::default(),
            Some("end_turn"),
            Some("hello"),
            &[],
        );
        records.push(result);

        let mut prev_seq: Option<u64> = Some(0);
        for record in &records {
            if let Some(seq) = record.get("sequence").and_then(Value::as_u64) {
                if let Some(prev) = prev_seq {
                    assert!(seq > prev, "sequence must increase: {prev} -> {seq}");
                }
                prev_seq = Some(seq);
            }
        }
        assert!(
            prev_seq.unwrap() > 0,
            "at least one record after init should have a sequence"
        );
    }
}
