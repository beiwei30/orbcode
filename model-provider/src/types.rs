use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration as StdDuration;

use orbcode_protocol::{
    EffortLevel, ProviderId, ProviderToolDefinition, TokenUsage, TranscriptBlock,
    TranscriptMessage, TurnContext,
};
use serde_json::{Map, Value};
use tokio::time::{Duration, sleep};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpenAiWireMode {
    #[default]
    ChatCompletions,
    Responses,
}

#[derive(Clone, Debug)]
pub struct ProviderRequest {
    pub session_id: String,
    pub prompt: String,
    pub context: TurnContext,
    pub messages: Vec<TranscriptMessage>,
    pub system_prompt: String,
    pub tools: Vec<ProviderToolDefinition>,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub auth_token: Option<String>,
    pub disable_thinking: bool,
    pub effort: Option<EffortLevel>,
    pub options: ProviderRequestOptions,
}

/// Per-request knobs that mirror the TypeScript client's tunable options.
///
/// Defaults leave the request unchanged from the historical behavior — every
/// field is optional so config callers can opt in piecewise. Unsupported
/// values for a provider (e.g. `temperature` on Anthropic's count-tokens
/// endpoint) are explicitly skipped rather than silently sent.
#[derive(Clone, Debug, Default)]
pub struct ProviderRequestOptions {
    /// Per-session Anthropic extended-thinking budget. When set it takes
    /// precedence over the effort-derived budget for this request. Providers
    /// without a numeric thinking-budget contract ignore it; validation lives
    /// at the app-server control boundary.
    pub max_thinking_tokens: Option<u32>,
    /// Caps the assistant turn's output tokens. Maps to `max_tokens` on both
    /// Anthropic and OpenAI request bodies. When `None`, the provider uses
    /// the historical 4096 + thinking-budget default.
    pub max_output_tokens: Option<u32>,
    /// Sampling temperature passed through to the provider when set.
    pub temperature: Option<f32>,
    /// Anthropic beta feature gates (`anthropic-beta` header + `anthropic_beta`
    /// body field via `extra_body`).
    pub anthropic_betas: Vec<String>,
    /// JSON object merged into the Anthropic request body (e.g.
    /// `CLAUDE_CODE_EXTRA_BODY` parity).
    pub extra_body: Map<String, Value>,
    /// Anthropic `metadata` body field (e.g. `user_id` analytics envelope).
    pub metadata: Option<Value>,
    /// Override the outgoing `User-Agent` header.
    pub user_agent: Option<String>,
    /// Additional headers to attach to provider HTTP requests
    /// (`ANTHROPIC_CUSTOM_HEADERS` parity).
    pub custom_headers: Vec<(String, String)>,
    /// Client-generated request id, sent as `x-client-request-id` so callers
    /// can correlate timeouts with server logs.
    pub request_id: Option<String>,
    /// Transport timeout (`API_TIMEOUT_MS` parity).
    pub timeout: Option<StdDuration>,
    /// SDK-level retry count handed to the HTTP client builder. The core
    /// retry loop continues to live in `core::retry` — this is the
    /// per-attempt cap for transient transport failures.
    pub max_retries: Option<u32>,
    /// Outbound proxy URL applied when the HTTP client is built.
    pub proxy: Option<String>,
    /// Optional bypass list attached to the selected concrete proxy.
    pub proxy_no_proxy: Option<String>,
    /// Tracks whether `proxy` and `proxy_no_proxy` were selected by
    /// `orbcode-config`. Core uses this to recalculate destination-aware system
    /// routes after a fallback or fixed endpoint changes the request URL.
    #[doc(hidden)]
    pub proxy_resolved_from_config: bool,
    /// Selects the OpenAI wire contract. API-key and compatible endpoints keep
    /// the historical Chat Completions default; ChatGPT OAuth uses Responses.
    pub openai_wire_mode: OpenAiWireMode,
    /// ChatGPT workspace/account header used only with the Responses mode.
    pub openai_account_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderRequestDebugSnapshot {
    pub provider: ProviderId,
    pub source: String,
    pub session_id: String,
    pub model: String,
    pub base_url: String,
    pub captured_at: String,
    pub recent_activity_json: String,
    pub previous_turn_json: String,
    pub body_json: String,
}

#[derive(Clone, Debug)]
pub struct ProviderResponse {
    pub provider: ProviderId,
    pub fallback_from: Option<ProviderId>,
    pub content: String,
    pub blocks: Vec<TranscriptBlock>,
    pub stop_reason: Option<String>,
    pub usage: TokenUsage,
    pub deltas: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderCancellationToken {
    flag: Option<Arc<AtomicBool>>,
}

impl ProviderCancellationToken {
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self { flag: Some(flag) }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            sleep(Duration::from_millis(10)).await;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderStreamEvent {
    MessageStart {
        provider: ProviderId,
        fallback_from: Option<ProviderId>,
        usage: TokenUsage,
    },
    ContentBlockStart {
        index: usize,
        block: ProviderContentBlockStart,
    },
    ContentBlockDelta {
        index: usize,
        delta: ProviderContentBlockDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        stop_reason: Option<String>,
        usage: TokenUsage,
    },
    MessageStop,
}

impl ProviderStreamEvent {
    pub fn starts_assistant_content(&self) -> bool {
        matches!(
            self,
            Self::ContentBlockStart { .. } | Self::ContentBlockDelta { .. }
        )
    }

    pub fn with_provider_metadata(
        mut self,
        provider: ProviderId,
        fallback_from: Option<ProviderId>,
    ) -> Self {
        if let Self::MessageStart {
            provider: event_provider,
            fallback_from: event_fallback_from,
            ..
        } = &mut self
        {
            *event_provider = provider;
            *event_fallback_from = fallback_from;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderContentBlockStart {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderContentBlockDelta {
    Text(String),
    Thinking(String),
    Signature(String),
    InputJson(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCompletion {
    pub provider: ProviderId,
    pub fallback_from: Option<ProviderId>,
    pub usage: TokenUsage,
    pub stop_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub summary: &'static str,
}
