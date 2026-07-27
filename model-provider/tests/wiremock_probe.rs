//! End-to-end probe tests against a [`wiremock`] server.
//!
//! These drive `probe_provider` through the real HTTP path (Anthropic adapter →
//! HTTP client → wiremock) so that status-code classification, header parsing,
//! and `ProbeResult` mapping are exercised exactly as production would see them.

use orbcode_model_provider::{
    ProbeResult, ProviderCancellationToken, ProviderRequest, ProviderRequestOptions,
    StreamErrorCategory, probe_provider,
};
use orbcode_protocol::{ProviderId, TurnContext};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn probe_request(base_url: String) -> ProviderRequest {
    ProviderRequest {
        session_id: "probe-e2e".to_string(),
        prompt: "ping".to_string(),
        context: TurnContext::default(),
        messages: Vec::new(),
        system_prompt: String::new(),
        tools: Vec::new(),
        model: "claude-sonnet-4-6".to_string(),
        base_url,
        api_key: Some("test-api-key".to_string()),
        auth_token: None,
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions {
            max_output_tokens: Some(1),
            ..ProviderRequestOptions::default()
        },
    }
}

#[tokio::test]
async fn probe_429_with_retry_after_classifies_as_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "42")
                .insert_header("anthropic-ratelimit-unified-status", "rejected")
                .insert_header("anthropic-ratelimit-unified-remaining", "0")
                .set_body_string(
                    r#"{"type":"error","error":{"type":"rate_limit_error","message":"rate limit exceeded"}}"#,
                ),
        )
        .mount(&server)
        .await;

    let request = probe_request(server.uri());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;

    assert!(!report.is_ok());
    let error = report.error.as_ref().expect("probe surfaces error");
    assert_eq!(error.category, StreamErrorCategory::RateLimit);
    assert_eq!(error.status, Some(429));
    assert_eq!(error.retry_after_secs(), Some(42));

    match report.classify() {
        ProbeResult::RateLimited {
            retry_after_seconds,
        } => {
            assert_eq!(retry_after_seconds, Some(42));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn probe_429_without_retry_after_still_classifies_as_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_string(
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"too many requests"}}"#,
        ))
        .mount(&server)
        .await;

    let request = probe_request(server.uri());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;

    match report.classify() {
        ProbeResult::RateLimited {
            retry_after_seconds,
        } => {
            assert_eq!(
                retry_after_seconds, None,
                "no Retry-After header means retry_after_seconds is None"
            );
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn probe_402_classifies_as_account_suspended() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(402).set_body_string(
            r#"{"type":"error","error":{"type":"billing_error","message":"payment required"}}"#,
        ))
        .mount(&server)
        .await;

    let request = probe_request(server.uri());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;

    assert!(!report.is_ok());
    let error = report.error.as_ref().expect("probe surfaces error");
    assert_eq!(error.category, StreamErrorCategory::AccountSuspended);
    assert_eq!(error.status, Some(402));

    assert!(
        matches!(report.classify(), ProbeResult::AccountSuspended),
        "402 should classify as AccountSuspended"
    );
}

#[tokio::test]
async fn probe_200_success_classifies_as_ok() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "event:message_start\ndata:{\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":5,\"output_tokens\":1,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\n\
             event:content_block_start\ndata:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
             event:content_block_delta\ndata:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
             event:content_block_stop\ndata:{\"type\":\"content_block_stop\",\"index\":0}\n\n\
             event:message_delta\ndata:{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n\
             event:message_stop\ndata:{\"type\":\"message_stop\"}\n\n",
        ))
        .mount(&server)
        .await;

    let request = probe_request(server.uri());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;

    assert!(report.is_ok());
    assert!(report.error.is_none());
    assert!(matches!(report.classify(), ProbeResult::Ok));
}

#[tokio::test]
async fn probe_401_classifies_as_auth_failed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(
                r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
            ),
        )
        .mount(&server)
        .await;

    let request = probe_request(server.uri());
    let report = probe_provider(
        ProviderId::Anthropic,
        &request,
        ProviderCancellationToken::default(),
    )
    .await;

    assert!(!report.is_ok());
    let error = report.error.as_ref().expect("probe surfaces error");
    assert_eq!(error.category, StreamErrorCategory::Auth);
    assert_eq!(error.status, Some(401));

    match report.classify() {
        ProbeResult::Failed(err) => {
            assert_eq!(err.category, StreamErrorCategory::Auth);
        }
        other => panic!("expected Failed(Auth), got {other:?}"),
    }
}
