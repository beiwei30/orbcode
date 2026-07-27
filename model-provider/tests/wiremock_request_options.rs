//! End-to-end regression coverage for the per-request option fields.
//!
//! These tests spin up a [`wiremock`] server, run a real `stream_*_request`
//! call against it, and then inspect the captured request to confirm the
//! options surfaced by `AppConfig` actually make it onto the wire. They guard
//! the boundary between [`crate::ProviderRequestOptions`] and the HTTP
//! transport layer — the unit tests in `adapter_tests.rs` already cover the
//! serialized body in isolation; these tests verify the full path including
//! headers, body, and the client builder.

use async_trait::async_trait;
use orbcode_model_provider::{
    ProviderCancellationToken, ProviderError, ProviderRequest, ProviderRequestOptions,
    ProviderStreamEvent, ProviderStreamSink, stream_anthropic_request, stream_openai_request,
};
use orbcode_protocol::TurnContext;
use serde_json::{Map, Value, json};
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
    "data: {\"id\":\"chatcmpl-test\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1,\"total_tokens\":3}}\n\n",
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
        session_id: "session-e2e".to_string(),
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

fn full_options() -> ProviderRequestOptions {
    let mut extra_body = Map::new();
    extra_body.insert("flag".to_string(), json!(true));
    extra_body.insert("nested".to_string(), json!({"k": "v"}));

    ProviderRequestOptions {
        max_output_tokens: Some(4096),
        temperature: Some(0.5),
        anthropic_betas: vec![
            "context-1m-2025-01-14".to_string(),
            "prompt-caching-2024-07-31".to_string(),
        ],
        extra_body,
        metadata: Some(json!({"user_id": "abc"})),
        user_agent: Some("orbcode/e2e".to_string()),
        custom_headers: vec![("X-Cc-Test".to_string(), "yes".to_string())],
        request_id: Some("req-12345".to_string()),
        timeout: Some(std::time::Duration::from_secs(30)),
        max_retries: Some(2),
        proxy: None,
    }
}

fn header_value<'a>(req: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    req.headers.get(name).and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn anthropic_stream_sends_full_request_options_to_wire() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(ANTHROPIC_SSE.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = base_request(server.uri(), "claude-sonnet-4-6");
    request.options = full_options();

    let mut sink = NullSink;
    stream_anthropic_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect("anthropic stream succeeds against wiremock");

    let received = server.received_requests().await.expect("wiremock captured");
    assert_eq!(received.len(), 1, "exactly one request hit the mock");
    let req = &received[0];

    assert_eq!(
        header_value(req, "anthropic-beta"),
        Some("context-1m-2025-01-14,prompt-caching-2024-07-31"),
        "anthropic-beta header carries the betas joined by comma"
    );
    assert_eq!(
        header_value(req, "x-client-request-id"),
        Some("req-12345"),
        "x-client-request-id surfaces the request id"
    );
    assert_eq!(
        header_value(req, "user-agent"),
        Some("orbcode/e2e"),
        "user-agent override reaches the wire"
    );
    assert_eq!(
        header_value(req, "x-cc-test"),
        Some("yes"),
        "custom headers are attached verbatim"
    );
    assert_eq!(
        header_value(req, "x-api-key"),
        Some("test-api-key"),
        "credentials are still applied alongside options"
    );

    let body: Value = serde_json::from_slice(&req.body).expect("JSON body parses");
    assert_eq!(body["max_tokens"], json!(4096));
    assert_eq!(body["temperature"], json!(0.5));
    assert_eq!(body["metadata"], json!({"user_id": "abc"}));
    assert_eq!(body["flag"], json!(true));
    assert_eq!(body["nested"], json!({"k": "v"}));
    let betas = body["anthropic_beta"]
        .as_array()
        .expect("anthropic_beta is an array");
    let names: Vec<&str> = betas.iter().filter_map(Value::as_str).collect();
    assert!(names.contains(&"context-1m-2025-01-14"));
    assert!(names.contains(&"prompt-caching-2024-07-31"));
}

#[tokio::test]
async fn openai_stream_applies_options_and_does_not_leak_anthropic_beta_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(OPENAI_SSE.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = base_request(server.uri(), "gpt-test");
    request.options = full_options();

    let mut sink = NullSink;
    stream_openai_request(&request, &mut sink, ProviderCancellationToken::default())
        .await
        .expect("openai stream succeeds against wiremock");

    let received = server.received_requests().await.expect("wiremock captured");
    assert_eq!(received.len(), 1, "exactly one request hit the mock");
    let req = &received[0];

    assert!(
        req.headers.get("anthropic-beta").is_none(),
        "anthropic-beta must not leak onto OpenAI-compatible endpoints"
    );
    assert_eq!(
        header_value(req, "x-client-request-id"),
        Some("req-12345"),
        "request id surfaces on OpenAI calls too"
    );
    assert_eq!(
        header_value(req, "user-agent"),
        Some("orbcode/e2e"),
        "user-agent override reaches OpenAI"
    );
    assert_eq!(
        header_value(req, "x-cc-test"),
        Some("yes"),
        "custom headers are attached to OpenAI calls"
    );
    assert_eq!(
        header_value(req, "authorization"),
        Some("Bearer test-api-key"),
        "bearer auth coexists with the option headers"
    );

    let body: Value = serde_json::from_slice(&req.body).expect("JSON body parses");
    assert_eq!(body["max_tokens"], json!(4096));
    assert_eq!(body["temperature"], json!(0.5));
    assert_eq!(body["flag"], json!(true));
    assert_eq!(body["nested"], json!({"k": "v"}));
    assert!(
        body.get("anthropic_beta").is_none(),
        "OpenAI body must not include anthropic_beta"
    );
    assert!(
        body.get("metadata").is_none(),
        "OpenAI body must not include Anthropic metadata envelope"
    );
}
