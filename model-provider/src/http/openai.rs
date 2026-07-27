use reqwest::Client;

use orbcode_protocol::StreamErrorCategory;

use crate::{
    OpenAiStreamReader, ProviderCancellationToken, ProviderCompletion, ProviderError,
    ProviderErrorKind, ProviderRequest, ProviderStreamAccumulator, ProviderStreamSink,
    build_openai_request_body,
};
use orbcode_protocol::ProviderId;

use super::{
    anthropic::apply_request_option_headers, build_provider_http_client, parse_http_error,
    provider_interrupted_error, provider_transport_error_for, rate_limit_from_headers,
};

pub async fn stream_openai_request(
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    let client = build_provider_http_client(&request.options)?;
    let url = openai_chat_completions_url(&request.base_url);
    let http_request = build_openai_http_request(&client, request, &url)?;
    let response = tokio::select! {
        response = client.execute(http_request) => response
            .map_err(|error| provider_transport_error_for(ProviderId::OpenAi, &error, "failed to send OpenAI request"))?,
        _ = cancellation.cancelled() => return Err(provider_interrupted_error()),
    };

    let http_status = response.status().as_u16();
    if !response.status().is_success() {
        let rate_limit = rate_limit_from_headers(response.headers());
        let body = response.text().await.unwrap_or_default();
        return Err(
            parse_http_error(ProviderId::OpenAi, http_status, &body).with_rate_limit(rate_limit)
        );
    }

    let mut response = response;
    let mut stream = OpenAiStreamReader::new(request.model.clone());
    let mut accumulator = ProviderStreamAccumulator::new(ProviderId::OpenAi, None);

    while let Some(chunk) = tokio::select! {
        chunk = response.chunk() => chunk.map_err(|error| {
            provider_transport_error_for(ProviderId::OpenAi, &error, "failed to read OpenAI streaming response")
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

    Ok(ProviderCompletion {
        provider: ProviderId::OpenAi,
        fallback_from: None,
        usage: accumulator.usage(),
        stop_reason: accumulator.stop_reason().map(ToString::to_string),
    })
}

pub fn openai_chat_completions_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

pub fn build_openai_http_request(
    client: &Client,
    request: &ProviderRequest,
    url: &str,
) -> Result<reqwest::Request, ProviderError> {
    let mut request_builder = client
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        // Our own header — OpenAI has no session-id convention to match, unlike
        // the Anthropic path which must send `x-claude-code-session-id`.
        .header("x-orbcode-session-id", &request.session_id)
        .json(&build_openai_request_body(request));

    if let Some(api_key) = &request.api_key {
        request_builder = request_builder.bearer_auth(api_key);
    }
    request_builder = apply_request_option_headers(request_builder, request, false);

    request_builder.build().map_err(|error| ProviderError {
        kind: ProviderErrorKind::Fatal,
        category: StreamErrorCategory::Other,
        provider: Some(ProviderId::OpenAi),
        status: None,
        message: format!("failed to build OpenAI HTTP request: {error}"),
        suggestion: None,
        rate_limit: None,
    })
}
