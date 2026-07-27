//! End-to-end coverage for rate-limit header capture, retryable error
//! classification, and count-tokens behavior (Bedrock adapter + Haiku
//! fallback) against a [`wiremock`] server.
//!
//! These complement the in-crate unit tests by driving the real HTTP path so
//! that header parsing, error attachment, and request routing are exercised
//! exactly as production would see them.

use async_trait::async_trait;
use orbcode_model_provider::{
    ProviderCancellationToken, ProviderError, ProviderErrorKind, ProviderRequest,
    ProviderRequestOptions, ProviderStreamEvent, ProviderStreamSink, StreamErrorCategory,
    count_tokens_anthropic, count_tokens_anthropic_with_haiku_fallback, count_tokens_bedrock,
    stream_anthropic_request,
};
use orbcode_protocol::TurnContext;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct NullSink;

#[async_trait]
impl ProviderStreamSink for NullSink {
    async fn emit(&mut self, _event: ProviderStreamEvent) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn base_request(base_url: String, model: &str) -> ProviderRequest {
    ProviderRequest {
        session_id: "session-rl".to_string(),
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

#[tokio::test]
async fn anthropic_429_attaches_retry_after_and_unified_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "7")
                .insert_header("anthropic-ratelimit-unified-status", "rejected")
                .insert_header("anthropic-ratelimit-unified-remaining", "0")
                .set_body_string(
                    r#"{"type":"error","error":{"type":"rate_limit_error","message":"rate limit"}}"#,
                ),
        )
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("429 should surface as a provider error");

    assert_eq!(error.category, StreamErrorCategory::RateLimit);
    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.status, Some(429));
    assert_eq!(
        error.retry_after_secs(),
        Some(7),
        "Retry-After header is honored verbatim"
    );
    let meta = error.rate_limit.expect("rate-limit metadata captured");
    assert_eq!(meta.unified_status.as_deref(), Some("rejected"));
    assert_eq!(meta.unified_remaining, Some(0));
}

#[tokio::test]
async fn anthropic_429_preserves_mixed_retry_metadata_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "9")
                .insert_header("anthropic-ratelimit-unified-reset", "4102444800")
                .insert_header("anthropic-ratelimit-unified-status", "allowed_warning")
                .insert_header("anthropic-ratelimit-unified-remaining", "3")
                .insert_header("x-should-retry", "false")
                .set_body_string(
                    r#"{"type":"error","error":{"type":"rate_limit_error","message":"please wait"}}"#,
                ),
        )
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("429 should surface as a provider error");

    assert_eq!(error.category, StreamErrorCategory::RateLimit);
    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.retry_after_secs(), Some(9));
    let meta = error.rate_limit.expect("rate-limit metadata captured");
    assert_eq!(meta.unified_reset_unix, Some(4_102_444_800));
    assert_eq!(meta.unified_status.as_deref(), Some("allowed_warning"));
    assert_eq!(meta.unified_remaining, Some(3));
    assert_eq!(meta.should_retry, Some(false));
}

#[tokio::test]
async fn anthropic_5xx_is_retryable_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream unavailable"))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("503 should surface as a provider error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::ServerError);
    assert_eq!(error.status, Some(503));
    assert!(
        error.retry_after_secs().is_none(),
        "no Retry-After header means no retry-after seconds"
    );
}

#[tokio::test]
async fn anthropic_529_overload_is_retryable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(529).set_body_string(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"overloaded"}}"#,
        ))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let mut sink = NullSink;
    let error = stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect_err("529 should surface as a provider error");

    assert_eq!(error.kind, ProviderErrorKind::Retryable);
    assert_eq!(error.category, StreamErrorCategory::Overload);
}

#[tokio::test]
async fn count_tokens_anthropic_returns_input_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages/count_tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"input_tokens":321}"#))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-sonnet-4-6");
    let count = count_tokens_anthropic(&request)
        .await
        .expect("count-tokens call succeeds");
    assert_eq!(count, Some(321));
}

#[tokio::test]
async fn count_tokens_bedrock_parses_input_tokens_field() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/claude-sonnet-4-6/count-tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"inputTokens":654}"#))
        .mount(&server)
        .await;

    let request = base_request("https://unused".to_string(), "claude-sonnet-4-6");
    let count = count_tokens_bedrock(&request, &server.uri())
        .await
        .expect("bedrock count-tokens call succeeds");
    assert_eq!(count, Some(654));
}

#[tokio::test]
async fn count_tokens_bedrock_returns_none_on_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/claude-sonnet-4-6/count-tokens"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let request = base_request("https://unused".to_string(), "claude-sonnet-4-6");
    let count = count_tokens_bedrock(&request, &server.uri())
        .await
        .expect("bedrock count-tokens returns Ok even on failure");
    assert_eq!(
        count, None,
        "Bedrock failures degrade to None, not an error"
    );
}

#[tokio::test]
async fn count_tokens_falls_back_to_haiku_when_primary_fails() {
    let server = MockServer::start().await;
    // The primary model's count-tokens endpoint fails (non-success → None).
    Mock::given(method("POST"))
        .and(path("/v1/messages/count_tokens"))
        .and(body_string_contains("\"claude-opus-4-8\""))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;
    // The Haiku fallback model succeeds.
    Mock::given(method("POST"))
        .and(path("/v1/messages/count_tokens"))
        .and(body_string_contains("\"claude-haiku-4-5\""))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"input_tokens":99}"#))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-opus-4-8");
    let count = count_tokens_anthropic_with_haiku_fallback(&request, "claude-haiku-4-5")
        .await
        .expect("fallback count-tokens succeeds");
    assert_eq!(
        count,
        Some(99),
        "primary 404 falls back to the Haiku estimate"
    );
}

#[tokio::test]
async fn count_tokens_skips_fallback_when_already_haiku() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages/count_tokens"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;

    let request = base_request(server.uri(), "claude-haiku-4-5");
    let count = count_tokens_anthropic_with_haiku_fallback(&request, "claude-haiku-4-5")
        .await
        .expect("returns Ok(None) rather than looping");
    assert_eq!(count, None);

    let received = server.received_requests().await.expect("captured");
    assert_eq!(
        received.len(),
        1,
        "no second attempt is made when already on the fallback model"
    );
}
