use reqwest::Client;
use serde::Deserialize;

use orbcode_protocol::StreamErrorCategory;

use crate::{
    AnthropicStreamReader, ProviderCancellationToken, ProviderCompletion,
    ProviderContentBlockDelta, ProviderContentBlockStart, ProviderError, ProviderErrorKind,
    ProviderRequest, ProviderStreamAccumulator, ProviderStreamEvent, ProviderStreamSink,
    build_anthropic_count_tokens_request_body, build_anthropic_request_body,
    build_bedrock_count_tokens_request_body,
};
use orbcode_protocol::ProviderId;

#[derive(Deserialize)]
struct CountTokensResponse {
    #[serde(default)]
    input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct BedrockCountTokensResponse {
    #[serde(default, rename = "inputTokens", alias = "input_tokens")]
    input_tokens: Option<u64>,
}

fn saturating_provider_token_count(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

use super::{
    build_provider_http_client, parse_http_error, provider_interrupted_error,
    provider_transport_error_for, rate_limit_from_headers,
};

const MISSING_ANTHROPIC_CREDENTIALS: &str = "missing Anthropic credentials; set ORBCODE_ANTHROPIC_AUTH_TOKEN (or ANTHROPIC_AUTH_TOKEN), ORBCODE_ANTHROPIC_API_KEY (or ANTHROPIC_API_KEY), or ORBCODE_OAUTH_TOKEN (or CLAUDE_CODE_OAUTH_TOKEN), use `orbcode auth login --provider anthropic` to store credentials, or inspect `orbcode auth status` for blocked OAuth credentials";

pub async fn count_tokens_anthropic(
    request: &ProviderRequest,
) -> Result<Option<usize>, ProviderError> {
    // Check for credentials
    if request.api_key.is_none() && request.auth_token.is_none() {
        return Err(ProviderError::auth(
            ProviderId::Anthropic,
            MISSING_ANTHROPIC_CREDENTIALS,
        ));
    }

    let client = build_provider_http_client(&request.options)?;
    let base_url = request.base_url.trim_end_matches('/');
    let url = format!("{base_url}/v1/messages/count_tokens");

    let body = build_anthropic_count_tokens_request_body(request);

    let http_request = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .build()
        .map_err(|error| {
            provider_transport_error_for(
                ProviderId::Anthropic,
                &error,
                "failed to build count-tokens request",
            )
        })?;

    let response = client.execute(http_request).await.map_err(|error| {
        provider_transport_error_for(
            ProviderId::Anthropic,
            &error,
            "failed to send count-tokens request",
        )
    })?;

    if !response.status().is_success() {
        let _body_text = response.text().await.unwrap_or_default();
        return Ok(None);
    }

    let response_body: CountTokensResponse = response.json().await.map_err(|error| {
        provider_transport_error_for(
            ProviderId::Anthropic,
            &error,
            "failed to parse count-tokens response",
        )
    })?;

    Ok(response_body
        .input_tokens
        .map(saturating_provider_token_count))
}

/// Count tokens through a Bedrock-compatible count-tokens endpoint.
///
/// `bedrock_endpoint` is the base URL of the Bedrock runtime (or a mock); the
/// model-scoped count-tokens path is appended. Mirrors TypeScript's
/// `countTokensWithBedrock`, which returns `null` (here `Ok(None)`) on any
/// failure so the caller can fall back to a heuristic estimate. The Bedrock
/// `CountTokens` response carries the count as `inputTokens`.
pub async fn count_tokens_bedrock(
    request: &ProviderRequest,
    bedrock_endpoint: &str,
) -> Result<Option<usize>, ProviderError> {
    let client = build_provider_http_client(&request.options)?;
    let base = bedrock_endpoint.trim_end_matches('/');
    let url = format!("{base}/model/{}/count-tokens", request.model);
    let body = build_bedrock_count_tokens_request_body(request);

    let http_request = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .build()
        .map_err(|error| {
            provider_transport_error_for(
                ProviderId::Anthropic,
                &error,
                "failed to build Bedrock count-tokens request",
            )
        })?;

    let response = match client.execute(http_request).await {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };

    if !response.status().is_success() {
        let _ = response.text().await;
        return Ok(None);
    }

    let response_body: BedrockCountTokensResponse = match response.json().await {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    Ok(response_body
        .input_tokens
        .map(saturating_provider_token_count))
}

/// Count tokens with a Haiku-class fallback model.
///
/// Mirrors TypeScript's `countTokensViaHaikuFallback`: when the main model's
/// count-tokens call yields no usable number, re-issue the request against the
/// small/fast model so the caller still gets an estimate. The tool-search field
/// stripping already happens inside the request-body builders.
pub async fn count_tokens_via_haiku_fallback(
    request: &ProviderRequest,
    haiku_model: &str,
) -> Result<Option<usize>, ProviderError> {
    let mut fallback = request.clone();
    fallback.model = haiku_model.to_string();
    count_tokens_anthropic(&fallback).await
}

/// Count tokens for the request's model, falling back to `haiku_model` when the
/// primary count returns no usable value. When the request is already on the
/// fallback model, no second attempt is made.
pub async fn count_tokens_anthropic_with_haiku_fallback(
    request: &ProviderRequest,
    haiku_model: &str,
) -> Result<Option<usize>, ProviderError> {
    if let Some(count) = count_tokens_anthropic(request).await? {
        return Ok(Some(count));
    }
    if request.model == haiku_model {
        return Ok(None);
    }
    count_tokens_via_haiku_fallback(request, haiku_model).await
}

pub async fn stream_anthropic_request(
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    if request.api_key.is_none() && request.auth_token.is_none() {
        return Err(ProviderError::auth(
            ProviderId::Anthropic,
            MISSING_ANTHROPIC_CREDENTIALS,
        ));
    }

    let client = build_provider_http_client(&request.options)?;
    let url = anthropic_messages_url(&request.base_url);
    let http_request = build_anthropic_http_request(&client, request, &url)?;
    let response = tokio::select! {
        response = client.execute(http_request) => response
            .map_err(|error| provider_transport_error_for(ProviderId::Anthropic, &error, "failed to send Anthropic request"))?,
        _ = cancellation.cancelled() => return Err(provider_interrupted_error()),
    };

    let http_status = response.status().as_u16();
    if !response.status().is_success() {
        let rate_limit = rate_limit_from_headers(response.headers());
        let body = response.text().await.unwrap_or_default();
        return Err(
            parse_http_error(ProviderId::Anthropic, http_status, &body).with_rate_limit(rate_limit)
        );
    }

    let mut response = response;
    let mut stream = AnthropicStreamReader::default();
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::Anthropic, None);

    while let Some(chunk) = tokio::select! {
        chunk = response.chunk() => chunk.map_err(|error| {
            provider_transport_error_for(ProviderId::Anthropic, &error, "failed to read Anthropic streaming response")
        })?,
        _ = cancellation.cancelled() => return Err(provider_interrupted_error()),
    } {
        for event in stream.push_chunk_events(chunk.as_ref())? {
            accumulator.apply(&event);
            sink.emit(event).await?;
        }
    }
    for event in stream.finish_events()? {
        accumulator.apply(&event);
        sink.emit(event).await?;
    }

    if accumulator.content().is_empty() && !stream.plain_output().trim().is_empty() {
        let content = stream.plain_output().trim().to_string();
        let index = 0;
        let start = ProviderStreamEvent::ContentBlockStart {
            index,
            block: ProviderContentBlockStart::Text {
                text: String::new(),
            },
        };
        accumulator.apply(&start);
        sink.emit(start).await?;
        for delta in chunk_text(&content) {
            let event = ProviderStreamEvent::ContentBlockDelta {
                index,
                delta: ProviderContentBlockDelta::Text(delta),
            };
            accumulator.apply(&event);
            sink.emit(event).await?;
        }
        let stop = ProviderStreamEvent::ContentBlockStop { index };
        accumulator.apply(&stop);
        sink.emit(stop).await?;
    }

    Ok(ProviderCompletion {
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: accumulator.usage(),
        stop_reason: accumulator.stop_reason().map(ToString::to_string),
    })
}

pub fn anthropic_messages_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1/messages") || trimmed.ends_with("/messages") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/messages")
    }
}

pub fn build_anthropic_http_request(
    client: &Client,
    request: &ProviderRequest,
    url: &str,
) -> Result<reqwest::Request, ProviderError> {
    let mut request_builder = client
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("anthropic-version", "2023-06-01")
        // Deliberately NOT rebranded: the Anthropic API recognises this header
        // from the TypeScript CLI, so it is a compatibility contract. The OpenAI
        // path sends `x-orbcode-session-id` instead — that one has no upstream
        // counterpart, so it carries our own name.
        .header("x-claude-code-session-id", &request.session_id)
        .json(&build_anthropic_request_body(request));

    if let Some(api_key) = &request.api_key {
        request_builder = request_builder.header("x-api-key", api_key);
    }
    if let Some(auth_token) = &request.auth_token {
        request_builder = request_builder.bearer_auth(auth_token);
    }
    request_builder = apply_request_option_headers(request_builder, request, true);

    request_builder.build().map_err(|error| ProviderError {
        kind: ProviderErrorKind::Fatal,
        category: StreamErrorCategory::Other,
        provider: Some(ProviderId::Anthropic),
        status: None,
        message: format!("failed to build Anthropic HTTP request: {error}"),
        suggestion: None,
        rate_limit: None,
    })
}

pub(super) fn apply_request_option_headers(
    mut builder: reqwest::RequestBuilder,
    request: &ProviderRequest,
    include_anthropic_beta: bool,
) -> reqwest::RequestBuilder {
    let options = &request.options;
    if include_anthropic_beta && !options.anthropic_betas.is_empty() {
        builder = builder.header("anthropic-beta", options.anthropic_betas.join(","));
    }
    if let Some(request_id) = options.request_id.as_deref().filter(|v| !v.is_empty()) {
        builder = builder.header("x-client-request-id", request_id);
    }
    if let Some(user_agent) = options.user_agent.as_deref().filter(|v| !v.is_empty()) {
        builder = builder.header("user-agent", user_agent);
    }
    for (name, value) in &options.custom_headers {
        if name.is_empty() {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
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
mod numeric_tests {
    use super::saturating_provider_token_count;

    #[test]
    fn provider_token_count_saturates_to_platform_limit() {
        assert_eq!(saturating_provider_token_count(u64::MAX), usize::MAX);
    }
}
