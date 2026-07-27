mod anthropic;
mod openai;

use orbcode_protocol::ProviderId;

use orbcode_protocol::StreamErrorCategory;

use crate::{
    ProviderError, ProviderErrorKind, ProviderRequestOptions, RateLimitMetadata,
    classify_provider_error, parse_provider_error_body,
};

/// Parse rate-limit metadata from a response's header map. Header values that
/// are not valid UTF-8 are skipped, matching the lenient lookup the parser
/// expects.
pub(crate) fn rate_limit_from_headers(headers: &reqwest::header::HeaderMap) -> RateLimitMetadata {
    RateLimitMetadata::from_lookup(|name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(std::string::ToString::to_string)
    })
}

pub use anthropic::{
    anthropic_messages_url, build_anthropic_http_request, count_tokens_anthropic,
    count_tokens_anthropic_with_haiku_fallback, count_tokens_bedrock,
    count_tokens_via_haiku_fallback, stream_anthropic_request,
};
pub use openai::{build_openai_http_request, openai_chat_completions_url, stream_openai_request};

/// Build the shared HTTP client. With no per-request transport options this
/// is a defaulted `http1_only` client (preserved for callers that don't have
/// a request available, e.g. unit tests). When `options` is provided the
/// builder applies the configured timeout, retry hint (recorded for callers
/// that wrap retry above this layer), and proxy URL.
pub fn build_anthropic_http_client() -> Result<reqwest::Client, ProviderError> {
    build_provider_http_client(&ProviderRequestOptions::default())
}

pub fn build_provider_http_client(
    options: &ProviderRequestOptions,
) -> Result<reqwest::Client, ProviderError> {
    let mut builder = reqwest::Client::builder().http1_only();
    if let Some(timeout) = options.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(proxy_url) = options.proxy.as_deref().filter(|value| !value.is_empty()) {
        let proxy = reqwest::Proxy::all(proxy_url).map_err(|error| ProviderError {
            kind: ProviderErrorKind::Fatal,
            category: StreamErrorCategory::Other,
            provider: None,
            status: None,
            message: format!("invalid proxy URL `{proxy_url}`: {error}"),
            suggestion: Some(
                "set HTTPS_PROXY/HTTP_PROXY (or the `proxy` setting) to a URL such as `http://host:port`."
                    .to_string(),
            ),
            rate_limit: None,
        })?;
        builder = builder.proxy(proxy);
    }
    if let Some(user_agent) = options
        .user_agent
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        builder = builder.user_agent(user_agent);
    }
    builder.build().map_err(|error| ProviderError {
        kind: ProviderErrorKind::Fatal,
        category: StreamErrorCategory::Other,
        provider: None,
        status: None,
        message: format!("failed to build provider HTTP client: {error}"),
        suggestion: None,
        rate_limit: None,
    })
}

pub fn provider_transport_error(error: &reqwest::Error, context: &str) -> ProviderError {
    let provider = None;
    let status = error.status().map(|status| status.as_u16());
    let message = format!("{context}: {error}");
    transport_error(provider, status, message)
}

pub fn provider_transport_error_for(
    provider: ProviderId,
    error: &reqwest::Error,
    context: &str,
) -> ProviderError {
    let status = error.status().map(|status| status.as_u16());
    let message = format!("{context}: {error}");
    transport_error(Some(provider), status, message)
}

fn transport_error(
    provider: Option<ProviderId>,
    status: Option<u16>,
    message: String,
) -> ProviderError {
    let classified = classify_provider_error(provider, status, &message);
    ProviderError {
        kind: classified.kind,
        category: classified.category,
        provider,
        status,
        message,
        suggestion: classified.suggestion,
        rate_limit: None,
    }
}

pub fn extract_http_error_message(status: u16, body: &str) -> String {
    parse_provider_error_body(ProviderId::Anthropic, status, body).message
}

pub fn parse_http_error(provider: ProviderId, status: u16, body: &str) -> ProviderError {
    parse_provider_error_body(provider, status, body)
}

pub(super) fn provider_interrupted_error() -> ProviderError {
    ProviderError::interrupted("provider stream interrupted")
}
