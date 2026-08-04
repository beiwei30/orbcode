mod accumulator;
mod adapters;
mod count_tokens_cache;
mod debug;
mod error;
mod http;
#[cfg(any(test, feature = "mock-provider"))]
mod mock;
mod model;
mod probe;
mod rate_limit;
mod request;
mod stream;
mod types;
mod usage;

pub use accumulator::{ProviderStreamAccumulator, render_blocks_for_display};
pub use adapters::{is_provider_active, provider_for, supported_providers};
pub use count_tokens_cache::{CountTokensCache, CountTokensCacheKey, DEFAULT_COUNT_TOKENS_TTL};
pub use debug::ProviderDebugTrace;
pub use error::{
    ClassifiedProviderError, ProviderError, ProviderErrorKind, classify_http_error,
    classify_provider_error, parse_provider_error_body, sanitize_provider_error_message,
    suggestion_for, suggestion_for_message,
};
pub use http::{
    anthropic_messages_url, build_anthropic_http_client, build_anthropic_http_request,
    build_openai_http_request, build_provider_http_client, count_tokens_anthropic,
    count_tokens_anthropic_with_haiku_fallback, count_tokens_bedrock,
    count_tokens_via_haiku_fallback, extract_http_error_message, openai_chat_completions_url,
    provider_transport_error, stream_anthropic_request, stream_openai_request,
};
pub use model::{AttemptDiscardDisposition, ModelProvider, ProviderStreamSink};
pub use orbcode_protocol::StreamErrorCategory;
pub use probe::{ProbeResult, ProviderProbeReport, probe_provider};
pub use rate_limit::{
    BASE_RETRY_DELAY_MS, DEFAULT_MAX_RETRY_DELAY_MS, RateLimitMetadata, default_jitter_factor,
    retry_delay_ms, retry_delay_ms_with_base,
};
pub use request::{
    build_anthropic_count_tokens_request_body, build_anthropic_request_body,
    build_bedrock_count_tokens_request_body, build_openai_request_body, debug_request_summary,
    debug_response_summary, provider_request_debug_snapshot, provider_visible_messages_value,
    render_pre_user_instructions, strip_search_extra_tools_fields,
};
pub use stream::{
    AnthropicStreamReader, OpenAiStreamReader, decode_stream_line,
    provider_stream_event_from_sse_frame,
};
pub use types::{
    ProviderCancellationToken, ProviderCompletion, ProviderContentBlockDelta,
    ProviderContentBlockStart, ProviderDescriptor, ProviderRequest, ProviderRequestDebugSnapshot,
    ProviderRequestOptions, ProviderResponse, ProviderStreamEvent,
};
pub use usage::{merge_usage, usage_from_value};

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod tests;
