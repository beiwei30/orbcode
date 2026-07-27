//! Timeout, transport-error, and stream-error classification tests.
//!
//! These spin up a [`wiremock`] server with deliberate delays, error status
//! codes, or mid-stream error SSE events and verify that the provider layer
//! produces errors with the correct category and retryability — the
//! preconditions the core retry/fallback loop depends on.

use std::time::Duration;

use async_trait::async_trait;
use orbcode_model_provider::{
    ProviderCancellationToken, ProviderError, ProviderErrorKind, ProviderRequest,
    ProviderRequestOptions, ProviderStreamEvent, ProviderStreamSink, StreamErrorCategory,
    stream_anthropic_request, stream_openai_request,
};
use orbcode_protocol::TurnContext;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ANTHROPIC_SSE: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

const OPENAI_SSE: &str = concat!(
    "data: {\"id\":\"chatcmpl-t\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-t\",\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-t\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1,\"total_tokens\":3}}\n\n",
    "data: [DONE]\n\n",
);

struct NullSink;

#[async_trait]
impl ProviderStreamSink for NullSink {
    async fn emit(&mut self, _event: ProviderStreamEvent) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn base_request(base_url: String, model: &str) -> ProviderRequest {
    ProviderRequest {
        session_id: "session-timeout".to_string(),
        prompt: "hi".to_string(),
        context: TurnContext::default(),
        messages: Vec::new(),
        system_prompt: String::new(),
        tools: Vec::new(),
        model: model.to_string(),
        base_url,
        api_key: Some("test-api-key".to_string()),
        auth_token: None,
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    }
}

// ---------------------------------------------------------------------------
// 1. Anthropic: transport timeout produces an error (not success)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_transport_timeout_produces_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(ANTHROPIC_SSE.as_bytes().to_vec(), "text/event-stream")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&server)
        .await;

    let mut request = base_request(server.uri(), "claude-sonnet-4-6");
    request.options.timeout = Some(Duration::from_millis(200));

    let mut sink = NullSink;
    let result =
        stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default()).await;

    assert!(
        result.is_err(),
        "server delay exceeding client timeout must produce an error"
    );
}

// ---------------------------------------------------------------------------
// 2. OpenAI: transport timeout produces an error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_transport_timeout_produces_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(OPENAI_SSE.as_bytes().to_vec(), "text/event-stream")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&server)
        .await;

    let mut request = base_request(server.uri(), "gpt-test");
    request.options.timeout = Some(Duration::from_millis(200));

    let mut sink = NullSink;
    let result =
        stream_openai_request(&request, &mut sink, ProviderCancellationToken::default()).await;

    assert!(
        result.is_err(),
        "server delay exceeding client timeout must produce an error"
    );
}

// ---------------------------------------------------------------------------
// 3. Slow response within timeout succeeds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn slow_response_within_timeout_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(ANTHROPIC_SSE.as_bytes().to_vec(), "text/event-stream")
                .set_delay(Duration::from_millis(50)),
        )
        .mount(&server)
        .await;

    let mut request = base_request(server.uri(), "claude-sonnet-4-6");
    request.options.timeout = Some(Duration::from_secs(5));

    let mut sink = NullSink;
    let result =
        stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default()).await;

    assert!(
        result.is_ok(),
        "delay shorter than timeout must succeed: {:?}",
        result.err()
    );
    let completion = result.unwrap();
    assert_eq!(completion.stop_reason.as_deref(), Some("end_turn"));
}

// ---------------------------------------------------------------------------
// 4. HTTP 408 (Request Timeout) is retryable Network error
//    This is how real provider-side timeouts surface — as an HTTP status.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_408_request_timeout_is_retryable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(408).set_body_string(
            r#"{"type":"error","error":{"type":"timeout","message":"request timed out"}}"#,
        ))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("408 should surface as an error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.status, Some(408));
}

// ---------------------------------------------------------------------------
// 5. Anthropic 502 is retryable ServerError
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_502_is_retryable_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("502 should be an error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::ServerError);
    assert_eq!(error.status, Some(502));
}

// ---------------------------------------------------------------------------
// 6. Anthropic 503 is retryable ServerError
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_503_is_retryable_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("503 should be an error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::ServerError);
    assert_eq!(error.status, Some(503));
}

// ---------------------------------------------------------------------------
// 7. Stream error event mid-stream: overloaded_error is retryable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_stream_overload_error_is_retryable() {
    let sse_with_error = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
    );

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_with_error.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("overloaded_error mid-stream should surface as error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::Overload);
}

// ---------------------------------------------------------------------------
// 8. Stream error event: rate_limit_error is retryable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_stream_rate_limit_error_is_retryable() {
    let sse_with_error = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"Rate limited\"}}\n\n",
    );

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_with_error.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("rate_limit_error mid-stream should surface as error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::RateLimit);
}

// ---------------------------------------------------------------------------
// 9. Stream error event: authentication_error is fatal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_stream_auth_error_is_fatal() {
    let sse_with_error = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"Invalid API key\"}}\n\n",
    );

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_with_error.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("auth error should surface");

    assert_eq!(error.kind, ProviderErrorKind::Fatal);
    assert_eq!(error.category, StreamErrorCategory::Auth);
}
