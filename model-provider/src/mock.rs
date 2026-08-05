//! URL-driven mock provider, gated to test builds.
//!
//! Replaces the old `#retry:<provider>` / `#fatal:<provider>` prompt markers
//! that branched the production `stream`/`count_tokens` path on user prompt
//! content. Behavior is selected from the request `base_url` instead, e.g.
//! `mock://anthropic?scenario=retryable`, so retry/fallback/diagnostic tests
//! configure failures through provider config (env/settings) exactly like a
//! real endpoint — never by injecting tokens into the prompt.
//!
//! Supported `scenario` values:
//! - `success` (default): a minimal successful text stream.
//! - `retryable`: a retryable `ServerError` on every attempt.
//! - `fatal`: a fatal error on every attempt.
//! - `interrupted`: an interrupted stream on every attempt.
//! - `interrupt_after_text`: emits a partial text block, then interrupts.
//! - `ratelimit`: a retryable `RateLimit` error carrying a `429` plus
//!   `Retry-After` / unified rate-limit metadata.
//! - `auth`: a fatal `Auth` error (`401`) with a credentials suggestion.
//! - `account_suspended`: a fatal `AccountSuspended` error (`402`) with a
//!   billing suggestion.
//! - `retry_then_success`: fail retryably `attempts=<N>` times (keyed by
//!   `key=<id>` for cross-attempt counting), then succeed.
//! - `thinking`: a successful stream with a thinking block followed by text.
//! - `tool_use`: a stream that produces a single tool_use block; use
//!   `key=<tool_name>` to set the tool name (default `bash`) and
//!   `command=<cmd>` to override the bash command input, or `input=<json>` to
//!   override the full tool input.
//! - `many_deltas`: a stream that produces `attempts` (default 2000) text
//!   deltas in rapid succession. Useful for saturating bounded channels in
//!   slow-consumer / backpressure tests.
//! - `hang`: emits `MessageStart` + `ContentBlockStart` then blocks until the
//!   provider cancellation token fires (useful for SIGTERM tests).

use std::collections::HashMap;
use std::sync::Mutex;

use orbcode_protocol::{ProviderId, TokenUsage, TranscriptBlock};
use tokio::task::yield_now;

use crate::ProviderStreamSink;
use crate::error::{ProviderError, ProviderErrorKind, suggestion_for_message};
use crate::rate_limit::RateLimitMetadata;
use crate::types::{
    ProviderCancellationToken, ProviderCompletion, ProviderContentBlockDelta,
    ProviderContentBlockStart, ProviderRequest, ProviderStreamEvent,
};
use orbcode_protocol::StreamErrorCategory;

const MOCK_SCHEME: &str = "mock://";

pub(crate) fn is_mock_base_url(base_url: &str) -> bool {
    base_url.starts_with(MOCK_SCHEME)
}

struct MockScenario {
    scenario: String,
    attempts: usize,
    key: Option<String>,
    command: Option<String>,
    input: Option<String>,
}

fn parse_scenario(base_url: &str) -> MockScenario {
    let mut scenario = "success".to_string();
    let mut attempts = 1_usize;
    let mut key = None;
    let mut command = None;
    let mut input = None;
    if let Ok(url) = reqwest::Url::parse(base_url) {
        for (name, value) in url.query_pairs() {
            match name.as_ref() {
                "scenario" => scenario = value.to_string(),
                "attempts" => attempts = value.parse().unwrap_or(1),
                "key" => key = Some(value.to_string()),
                "command" => command = Some(value.to_string()),
                "input" => input = Some(value.to_string()),
                _ => {}
            }
        }
    }
    MockScenario {
        scenario,
        attempts,
        key,
        command,
        input,
    }
}

/// Per-`key` failure counters so `retry_then_success` can fail a fixed number
/// of times across the retry loop's repeated `stream` calls.
fn attempt_counter(key: &str) -> usize {
    static COUNTERS: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);
    let mut guard = COUNTERS.lock().expect("mock counter mutex");
    let map = guard.get_or_insert_with(HashMap::new);
    let entry = map.entry(key.to_string()).or_insert(0);
    *entry += 1;
    *entry
}

pub(crate) fn count_tokens_mock(
    _request: &ProviderRequest,
) -> Result<Option<usize>, ProviderError> {
    // Count-tokens is a preflight outside the retry loop; the mock never fails
    // it so scenario failures stay scoped to the streaming attempt.
    Ok(None)
}

pub(crate) async fn stream_mock(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    let scenario = parse_scenario(&request.base_url);
    match scenario.scenario.as_str() {
        "retryable" => Err(retryable_error(provider)),
        "fatal" => Err(fatal_error(provider)),
        "interrupted" => Err(ProviderError::interrupted(format!(
            "simulated interruption for {provider}"
        ))),
        "interrupt_after_text" => {
            stream_interrupt_after_text(provider, request, sink, cancellation).await
        }
        "ratelimit" => Err(rate_limit_error(provider)),
        "auth" => Err(auth_error(provider)),
        "account_suspended" => Err(account_suspended_error(provider)),
        "retry_then_success" => {
            let key = scenario
                .key
                .unwrap_or_else(|| format!("{provider}-{}", request.session_id));
            if attempt_counter(&key) <= scenario.attempts {
                Err(retryable_error(provider))
            } else {
                stream_success(provider, request, sink, cancellation).await
            }
        }
        "thinking" => stream_thinking(provider, request, sink, cancellation).await,
        "tool_use" => {
            let tool_name = scenario.key.unwrap_or_else(|| "bash".to_string());
            stream_tool_use(
                provider,
                request,
                sink,
                cancellation,
                &tool_name,
                scenario.command.as_deref(),
                scenario.input.as_deref(),
            )
            .await
        }
        "many_deltas" => {
            stream_many_deltas(provider, request, sink, cancellation, scenario.attempts).await
        }
        "hang" => stream_hang(provider, request, sink, cancellation).await,
        _ => stream_success(provider, request, sink, cancellation).await,
    }
}

fn retryable_error(provider: ProviderId) -> ProviderError {
    let category = StreamErrorCategory::ServerError;
    let message = format!("simulated retryable failure for {provider}");
    let suggestion = suggestion_for_message(provider, category, None, &message);
    ProviderError {
        kind: ProviderErrorKind::Retryable,
        category,
        provider: Some(provider),
        status: None,
        message,
        suggestion: Some(suggestion),
        rate_limit: None,
    }
}

fn fatal_error(provider: ProviderId) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Fatal,
        category: StreamErrorCategory::Other,
        provider: Some(provider),
        status: None,
        message: format!("simulated fatal failure for {provider}"),
        suggestion: None,
        rate_limit: None,
    }
}

fn rate_limit_error(provider: ProviderId) -> ProviderError {
    let category = StreamErrorCategory::RateLimit;
    let message = format!("simulated rate limit for {provider}");
    let suggestion = suggestion_for_message(provider, category, Some(429), &message);
    ProviderError {
        kind: ProviderErrorKind::Retryable,
        category,
        provider: Some(provider),
        status: Some(429),
        message,
        suggestion: Some(suggestion),
        rate_limit: None,
    }
    .with_rate_limit(RateLimitMetadata {
        retry_after_secs: Some(1),
        unified_status: Some("rejected".to_string()),
        unified_remaining: Some(0),
        ..Default::default()
    })
}

fn auth_error(provider: ProviderId) -> ProviderError {
    ProviderError::auth(provider, format!("simulated auth failure for {provider}"))
}

fn account_suspended_error(provider: ProviderId) -> ProviderError {
    let category = StreamErrorCategory::AccountSuspended;
    let message = format!("simulated account suspended for {provider}");
    let suggestion = suggestion_for_message(provider, category, Some(402), &message);
    ProviderError {
        kind: ProviderErrorKind::Fatal,
        category,
        provider: Some(provider),
        status: Some(402),
        message,
        suggestion: Some(suggestion),
        rate_limit: None,
    }
}

async fn stream_success(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    let content = format!("mock provider response for {provider}");
    stream_text(provider, request, sink, cancellation, content).await
}

async fn stream_interrupt_after_text(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    if cancellation.is_cancelled() {
        return Err(ProviderError::interrupted("mock stream interrupted"));
    }
    let partial = "partial text before provider interruption";
    let usage = TokenUsage::from_text(&request.prompt, partial);
    sink.emit(ProviderStreamEvent::MessageStart {
        provider,
        fallback_from: None,
        usage: TokenUsage::default(),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockStart {
        index: 0,
        block: ProviderContentBlockStart::Text {
            text: String::new(),
        },
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockDelta {
        index: 0,
        delta: ProviderContentBlockDelta::Text(partial.to_string()),
    })
    .await?;
    sink.emit(ProviderStreamEvent::MessageDelta {
        stop_reason: None,
        usage,
    })
    .await?;
    Err(ProviderError::interrupted(
        "simulated interruption after text",
    ))
}

async fn stream_text(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
    content: String,
) -> Result<ProviderCompletion, ProviderError> {
    if cancellation.is_cancelled() {
        return Err(ProviderError::interrupted("mock stream interrupted"));
    }
    let usage = TokenUsage::from_text(&request.prompt, &content);
    let mut start_usage = usage.clone();
    start_usage.output_tokens = 0;
    start_usage.refresh_total_from_components();

    sink.emit(ProviderStreamEvent::MessageStart {
        provider,
        fallback_from: None,
        usage: start_usage,
    })
    .await?;
    yield_now().await;
    sink.emit(ProviderStreamEvent::ContentBlockStart {
        index: 0,
        block: ProviderContentBlockStart::Text {
            text: String::new(),
        },
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockDelta {
        index: 0,
        delta: ProviderContentBlockDelta::Text(content.clone()),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockStop { index: 0 })
        .await?;
    sink.emit(ProviderStreamEvent::MessageDelta {
        stop_reason: Some("end_turn".to_string()),
        usage: usage.clone(),
    })
    .await?;
    sink.emit(ProviderStreamEvent::MessageStop).await?;

    Ok(ProviderCompletion {
        provider,
        fallback_from: None,
        usage,
        stop_reason: Some("end_turn".to_string()),
    })
}

async fn stream_thinking(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    if cancellation.is_cancelled() {
        return Err(ProviderError::interrupted("mock stream interrupted"));
    }
    let thinking = format!("mock thinking for {provider}");
    let content = format!("mock thinking response for {provider}");
    let usage = TokenUsage::from_text(&request.prompt, &content);
    let mut start_usage = usage.clone();
    start_usage.output_tokens = 0;
    start_usage.refresh_total_from_components();

    sink.emit(ProviderStreamEvent::MessageStart {
        provider,
        fallback_from: None,
        usage: start_usage,
    })
    .await?;
    yield_now().await;

    sink.emit(ProviderStreamEvent::ContentBlockStart {
        index: 0,
        block: ProviderContentBlockStart::Thinking {
            text: String::new(),
            signature: None,
        },
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockDelta {
        index: 0,
        delta: ProviderContentBlockDelta::Thinking(thinking.clone()),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockDelta {
        index: 0,
        delta: ProviderContentBlockDelta::Signature("mock-sig-abc123".to_string()),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockStop { index: 0 })
        .await?;

    sink.emit(ProviderStreamEvent::ContentBlockStart {
        index: 1,
        block: ProviderContentBlockStart::Text {
            text: String::new(),
        },
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockDelta {
        index: 1,
        delta: ProviderContentBlockDelta::Text(content.clone()),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockStop { index: 1 })
        .await?;

    sink.emit(ProviderStreamEvent::MessageDelta {
        stop_reason: Some("end_turn".to_string()),
        usage: usage.clone(),
    })
    .await?;
    sink.emit(ProviderStreamEvent::MessageStop).await?;

    Ok(ProviderCompletion {
        provider,
        fallback_from: None,
        usage,
        stop_reason: Some("end_turn".to_string()),
    })
}

async fn stream_tool_use(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
    tool_name: &str,
    command_override: Option<&str>,
    input_override: Option<&str>,
) -> Result<ProviderCompletion, ProviderError> {
    if cancellation.is_cancelled() {
        return Err(ProviderError::interrupted("mock stream interrupted"));
    }
    if let Some(tool_result) = last_tool_result_content(request) {
        return stream_text(
            provider,
            request,
            sink,
            cancellation,
            format!("Tool `{tool_name}` completed.\n\n{tool_result}"),
        )
        .await;
    }

    let input_json_owned;
    let input_json = if let Some(input) = input_override {
        input_json_owned = input.to_string();
        &input_json_owned
    } else if let Some(cmd) = command_override {
        input_json_owned = format!(r#"{{"command":"{cmd}"}}"#);
        &input_json_owned
    } else {
        match tool_name {
            "Agent" => r#"{"prompt":"say hello","description":"mock agent task"}"#,
            _ => r#"{"command":"echo mock-tool-output"}"#,
        }
    };
    let usage = TokenUsage::from_text(&request.prompt, input_json);
    let mut start_usage = usage.clone();
    start_usage.output_tokens = 0;
    start_usage.refresh_total_from_components();

    sink.emit(ProviderStreamEvent::MessageStart {
        provider,
        fallback_from: None,
        usage: start_usage,
    })
    .await?;
    yield_now().await;

    sink.emit(ProviderStreamEvent::ContentBlockStart {
        index: 0,
        block: ProviderContentBlockStart::ToolUse {
            id: format!("toolu_mock_{tool_name}"),
            name: tool_name.to_string(),
            input: String::new(),
        },
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockDelta {
        index: 0,
        delta: ProviderContentBlockDelta::InputJson(input_json.to_string()),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ContentBlockStop { index: 0 })
        .await?;

    sink.emit(ProviderStreamEvent::MessageDelta {
        stop_reason: Some("tool_use".to_string()),
        usage: usage.clone(),
    })
    .await?;
    sink.emit(ProviderStreamEvent::MessageStop).await?;

    Ok(ProviderCompletion {
        provider,
        fallback_from: None,
        usage,
        stop_reason: Some("tool_use".to_string()),
    })
}

fn last_tool_result_content(request: &ProviderRequest) -> Option<String> {
    request
        .messages
        .iter()
        .rev()
        .flat_map(|message| message.blocks.iter().rev())
        .find_map(|block| match block {
            TranscriptBlock::ToolResult { content, .. } => Some(content.to_string()),
            _ => None,
        })
}

async fn stream_many_deltas(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
    count: usize,
) -> Result<ProviderCompletion, ProviderError> {
    if cancellation.is_cancelled() {
        return Err(ProviderError::interrupted("mock stream interrupted"));
    }
    let delta_count = if count == 0 { 2000 } else { count };
    let usage = TokenUsage::from_text(&request.prompt, "x");
    let mut start_usage = usage.clone();
    start_usage.output_tokens = 0;
    start_usage.refresh_total_from_components();

    sink.emit(ProviderStreamEvent::MessageStart {
        provider,
        fallback_from: None,
        usage: start_usage,
    })
    .await?;

    sink.emit(ProviderStreamEvent::ContentBlockStart {
        index: 0,
        block: ProviderContentBlockStart::Text {
            text: String::new(),
        },
    })
    .await?;

    for i in 0..delta_count {
        if cancellation.is_cancelled() {
            return Err(ProviderError::interrupted("mock stream interrupted"));
        }
        let chunk = format!("delta-{i:04}-padding-to-fill-channel-buffers-quickly.");
        sink.emit(ProviderStreamEvent::ContentBlockDelta {
            index: 0,
            delta: ProviderContentBlockDelta::Text(chunk),
        })
        .await?;
        yield_now().await;
    }

    sink.emit(ProviderStreamEvent::ContentBlockStop { index: 0 })
        .await?;
    sink.emit(ProviderStreamEvent::MessageDelta {
        stop_reason: Some("end_turn".to_string()),
        usage: usage.clone(),
    })
    .await?;
    sink.emit(ProviderStreamEvent::MessageStop).await?;

    Ok(ProviderCompletion {
        provider,
        fallback_from: None,
        usage,
        stop_reason: Some("end_turn".to_string()),
    })
}

async fn stream_hang(
    provider: ProviderId,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    let usage = TokenUsage::from_text(&request.prompt, "");
    let mut start_usage = usage.clone();
    start_usage.output_tokens = 0;
    start_usage.refresh_total_from_components();

    sink.emit(ProviderStreamEvent::MessageStart {
        provider,
        fallback_from: None,
        usage: start_usage,
    })
    .await?;
    yield_now().await;
    sink.emit(ProviderStreamEvent::ContentBlockStart {
        index: 0,
        block: ProviderContentBlockStart::Text {
            text: String::new(),
        },
    })
    .await?;

    while !cancellation.is_cancelled() {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    Err(ProviderError::interrupted("mock hang cancelled"))
}
