use orbcode_config::AppConfig;
use orbcode_model_provider::{
    AttemptDiscardDisposition, ProviderCancellationToken, ProviderCompletion, ProviderErrorKind,
    ProviderRequest, ProviderStreamEvent, ProviderStreamSink, default_jitter_factor, provider_for,
    retry_delay_ms_with_base,
};
use orbcode_protocol::{MessageRole, ProviderId, StreamErrorCategory, TranscriptBlock};

use crate::{CoreError, ProviderFailure, config_provider::AppConfigProviderRequestExt};

pub async fn execute_stream_with_retry_and_fallback(
    config: &AppConfig,
    request: ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, CoreError> {
    match try_provider_stream(
        config,
        config.default_provider,
        None,
        config.max_retries,
        &request,
        sink,
        cancellation.clone(),
    )
    .await
    {
        Ok(completion) => Ok(completion),
        Err(primary_error) => {
            if primary_error.kind == ProviderErrorKind::Retryable
                && let Some(fallback_provider) = config.fallback_provider
            {
                if primary_error.started_content {
                    let disposition = sink
                        .discard_attempt(
                        primary_error.provider,
                        fallback_provider,
                        &primary_error.message,
                    )
                    .await
                    .map_err(|error| {
                        CoreError::ProviderFailed(ProviderFailure {
                            message: format!(
                                "failed to discard partial response from {} before fallback to {}: {}",
                                primary_error.provider, fallback_provider, error.message
                            ),
                            category: primary_error.category,
                            suggestion: primary_error.suggestion.clone(),
                        })
                    })?;
                    if disposition == AttemptDiscardDisposition::ToolExecutionStarted {
                        return Err(CoreError::ProviderFailed(ProviderFailure {
                            message: format!(
                                "primary provider {} failed after {} attempt(s) [{}]: {}{}; fallback to {} suppressed because streamed tool execution started and the tool may already have produced side effects",
                                primary_error.provider,
                                primary_error.attempts,
                                primary_error.category.label(),
                                primary_error.message,
                                format_suggestion(primary_error.suggestion.as_deref()),
                                fallback_provider,
                            ),
                            category: primary_error.category,
                            suggestion: primary_error.suggestion,
                        }));
                    }
                }
                return try_provider_stream(
                        config,
                        fallback_provider,
                        Some(primary_error.provider),
                        config.max_retries,
                        &request,
                        sink,
                        cancellation,
                    )
                    .await
                    .map_err(|fallback_error| {
                        CoreError::RetryExhausted(ProviderFailure {
                            message: format!(
                                "primary provider {} failed after {} attempt(s) [{}]: {}{}; fallback {} failed after {} attempt(s) [{}]: {}{}",
                                primary_error.provider,
                                primary_error.attempts,
                                primary_error.category.label(),
                                primary_error.message,
                                format_suggestion(primary_error.suggestion.as_deref()),
                                fallback_error.provider,
                                fallback_error.attempts,
                                fallback_error.category.label(),
                                fallback_error.message,
                                format_suggestion(fallback_error.suggestion.as_deref()),
                            ),
                            category: StreamErrorCategory::RetryExhausted,
                            suggestion: fallback_error.suggestion,
                        })
                    });
            }

            let message = match primary_error.kind {
                ProviderErrorKind::Retryable => format!(
                    "provider {} failed after {} attempt(s) [{}]: {}{}",
                    primary_error.provider,
                    primary_error.attempts,
                    primary_error.category.label(),
                    primary_error.message,
                    format_suggestion(primary_error.suggestion.as_deref()),
                ),
                ProviderErrorKind::Fatal => format!(
                    "provider {} failed fatally [{}]: {}{}",
                    primary_error.provider,
                    primary_error.category.label(),
                    primary_error.message,
                    format_suggestion(primary_error.suggestion.as_deref()),
                ),
                ProviderErrorKind::Interrupted => format!(
                    "provider {} was interrupted after {} attempt(s) [{}]: {}{}",
                    primary_error.provider,
                    primary_error.attempts,
                    primary_error.category.label(),
                    primary_error.message,
                    format_suggestion(primary_error.suggestion.as_deref()),
                ),
            };
            Err(CoreError::ProviderFailed(ProviderFailure {
                message,
                category: primary_error.category,
                suggestion: primary_error.suggestion,
            }))
        }
    }
}

fn format_suggestion(suggestion: Option<&str>) -> String {
    match suggestion {
        Some(text) if !text.is_empty() => format!(" — {text}"),
        _ => String::new(),
    }
}

fn strip_thinking_blocks_for_fallback(request: &mut ProviderRequest) {
    for message in &mut request.messages {
        if message.role != MessageRole::Assistant {
            continue;
        }
        if message.blocks.is_empty() {
            continue;
        }
        message
            .blocks
            .retain(|block| !matches!(block, TranscriptBlock::Thinking { .. }));
        if message.blocks.is_empty() && !message.content.trim().is_empty() {
            message.blocks = vec![TranscriptBlock::Text {
                text: message.content.clone(),
            }];
        }
    }
}

#[derive(Debug)]
struct ProviderAttemptFailure {
    provider: ProviderId,
    kind: ProviderErrorKind,
    category: StreamErrorCategory,
    attempts: usize,
    message: String,
    suggestion: Option<String>,
    started_content: bool,
}

async fn try_provider_stream(
    config: &AppConfig,
    provider_id: ProviderId,
    fallback_from: Option<ProviderId>,
    max_retries: usize,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderAttemptFailure> {
    let provider = provider_for(provider_id);
    let mut attempts = 0;

    loop {
        attempts += 1;
        let mut provider_request = request.clone();
        config.configure_provider_request(provider_id, &mut provider_request);
        if fallback_from.is_some() {
            strip_thinking_blocks_for_fallback(&mut provider_request);
        }
        let mut attempt_sink = AttemptStreamSink::new(sink, provider_id, fallback_from);
        match provider
            .stream(&provider_request, &mut attempt_sink, cancellation.clone())
            .await
        {
            Ok(mut completion) => {
                attempt_sink
                    .finish_success()
                    .await
                    .map_err(|error| ProviderAttemptFailure {
                        provider: provider_id,
                        kind: error.kind,
                        category: error.category,
                        attempts,
                        message: error.message,
                        suggestion: error.suggestion,
                        started_content: attempt_sink.started_content,
                    })?;
                completion.provider = provider_id;
                completion.fallback_from = fallback_from;
                return Ok(completion);
            }
            Err(error)
                if error.kind == ProviderErrorKind::Retryable
                    && !attempt_sink.started_content
                    && attempts <= max_retries =>
            {
                // Honor the server's Retry-After when present; otherwise back
                // off exponentially with jitter. Mirrors the TypeScript client's
                // getRetryDelay. The base delay is configurable so tests can
                // collapse the schedule to 0.
                let delay_ms = retry_delay_ms_with_base(
                    attempts,
                    error.retry_after_secs(),
                    config.retry_base_delay_ms(),
                    config.retry_max_delay_ms(),
                    default_jitter_factor(),
                );
                if delay_ms > 0 {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                        _ = cancellation.cancelled() => {
                            return Err(ProviderAttemptFailure {
                                provider: provider_id,
                                kind: ProviderErrorKind::Interrupted,
                                category: StreamErrorCategory::Interrupted,
                                attempts,
                                message: "retry backoff interrupted".to_string(),
                                suggestion: None,
                                started_content: attempt_sink.started_content,
                            });
                        }
                    }
                }
            }
            Err(error) => {
                return Err(ProviderAttemptFailure {
                    provider: provider_id,
                    kind: error.kind,
                    category: error.category,
                    attempts,
                    message: error.message,
                    suggestion: error.suggestion,
                    started_content: attempt_sink.started_content,
                });
            }
        }
    }
}

struct AttemptStreamSink<'a> {
    inner: &'a mut dyn ProviderStreamSink,
    provider: ProviderId,
    fallback_from: Option<ProviderId>,
    buffered: Vec<ProviderStreamEvent>,
    committed: bool,
    started_content: bool,
}

impl<'a> AttemptStreamSink<'a> {
    fn new(
        inner: &'a mut dyn ProviderStreamSink,
        provider: ProviderId,
        fallback_from: Option<ProviderId>,
    ) -> Self {
        Self {
            inner,
            provider,
            fallback_from,
            buffered: Vec::new(),
            committed: false,
            started_content: false,
        }
    }

    async fn finish_success(&mut self) -> Result<(), orbcode_model_provider::ProviderError> {
        if !self.committed {
            self.flush_buffer().await?;
        }
        Ok(())
    }

    async fn flush_buffer(&mut self) -> Result<(), orbcode_model_provider::ProviderError> {
        self.committed = true;
        for event in std::mem::take(&mut self.buffered) {
            self.inner.emit(event).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ProviderStreamSink for AttemptStreamSink<'_> {
    async fn emit(
        &mut self,
        event: ProviderStreamEvent,
    ) -> Result<(), orbcode_model_provider::ProviderError> {
        let event = event.with_provider_metadata(self.provider, self.fallback_from);
        if event.starts_assistant_content() {
            self.started_content = true;
            if !self.committed {
                self.flush_buffer().await?;
            }
        }

        if self.committed {
            self.inner.emit(event).await
        } else {
            self.buffered.push(event);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_model_provider::ProviderRequest;
    use orbcode_protocol::{TranscriptMessage, TurnContext};

    fn make_request_with_thinking() -> ProviderRequest {
        ProviderRequest {
            session_id: "test".to_string(),
            prompt: "test".to_string(),
            context: TurnContext::default(),
            messages: vec![
                TranscriptMessage::from_blocks(
                    MessageRole::Assistant,
                    vec![
                        TranscriptBlock::Thinking {
                            text: "internal reasoning".to_string(),
                            signature: Some("sig-abc".to_string()),
                        },
                        TranscriptBlock::Text {
                            text: "visible text".to_string(),
                        },
                        TranscriptBlock::ToolUse {
                            id: "tool-1".to_string(),
                            name: "bash".to_string(),
                            input: "{}".to_string(),
                        },
                    ],
                ),
                TranscriptMessage::new(MessageRole::User, "follow up".to_string()),
            ],
            system_prompt: String::new(),
            tools: Vec::new(),
            model: "test".to_string(),
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: None,
            options: orbcode_model_provider::ProviderRequestOptions::default(),
        }
    }

    #[test]
    fn strip_thinking_blocks_removes_thinking_preserves_text_and_tools() {
        let mut request = make_request_with_thinking();
        strip_thinking_blocks_for_fallback(&mut request);

        let assistant = &request.messages[0];
        assert_eq!(assistant.blocks.len(), 2);
        assert!(matches!(
            &assistant.blocks[0],
            TranscriptBlock::Text { text } if text == "visible text"
        ));
        assert!(matches!(
            &assistant.blocks[1],
            TranscriptBlock::ToolUse { name, .. } if name == "bash"
        ));
    }

    #[test]
    fn strip_thinking_blocks_preserves_content_when_only_thinking() {
        let mut request = make_request_with_thinking();
        request.messages[0].blocks = vec![TranscriptBlock::Thinking {
            text: "only thinking".to_string(),
            signature: Some("sig".to_string()),
        }];
        request.messages[0].content = "fallback content".to_string();

        strip_thinking_blocks_for_fallback(&mut request);

        let assistant = &request.messages[0];
        assert_eq!(assistant.blocks.len(), 1);
        assert!(matches!(
            &assistant.blocks[0],
            TranscriptBlock::Text { text } if text == "fallback content"
        ));
    }

    #[test]
    fn strip_thinking_blocks_does_not_touch_user_messages() {
        let mut request = make_request_with_thinking();
        let user_msg = &request.messages[1];
        assert_eq!(user_msg.role, MessageRole::User);

        strip_thinking_blocks_for_fallback(&mut request);

        assert_eq!(request.messages[1].role, MessageRole::User);
        assert_eq!(request.messages[1].content, "follow up");
    }
}
