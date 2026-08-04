use orbcode_config::{AppConfig, AuthManager, OutboundProxyRoute};
use orbcode_model_provider::{
    OpenAiWireMode, ProviderCancellationToken, ProviderCompletion, ProviderErrorKind,
    ProviderRequest, ProviderStreamEvent, ProviderStreamSink, default_jitter_factor, provider_for,
    retry_delay_ms_with_base,
};
use orbcode_protocol::{MessageRole, ProviderId, StreamErrorCategory, TranscriptBlock};

use crate::{CoreError, ProviderFailure, config_provider::AppConfigProviderRequestExt};

pub async fn execute_stream_with_retry_and_fallback(
    config: &AppConfig,
    auth: &AuthManager,
    request: ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, CoreError> {
    match try_provider_stream(
        config,
        auth,
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
                    sink.discard_attempt(
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
                }
                return try_provider_stream(
                        config,
                        auth,
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
    auth: &AuthManager,
    provider_id: ProviderId,
    fallback_from: Option<ProviderId>,
    max_retries: usize,
    request: &ProviderRequest,
    sink: &mut dyn ProviderStreamSink,
    cancellation: ProviderCancellationToken,
) -> Result<ProviderCompletion, ProviderAttemptFailure> {
    let provider = provider_for(provider_id);
    let mut attempts = 0;
    let mut recovered_from_unauthorized = false;

    loop {
        attempts += 1;
        let mut provider_request = request.clone();
        config.configure_provider_request(provider_id, &mut provider_request);
        if let Err(message) =
            configure_chatgpt_request(config, auth, provider_id, &mut provider_request).await
        {
            return Err(ProviderAttemptFailure {
                provider: provider_id,
                kind: ProviderErrorKind::Fatal,
                category: StreamErrorCategory::Auth,
                attempts,
                message,
                suggestion: Some(
                    "run `orbcode auth login --provider openai --method chatgpt` again".to_string(),
                ),
                started_content: false,
            });
        }
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
                if provider_id == ProviderId::OpenAi
                    && provider_request.options.openai_wire_mode == OpenAiWireMode::Responses
                    && error.status == Some(401)
                    && !attempt_sink.started_content
                    && !recovered_from_unauthorized =>
            {
                recovered_from_unauthorized = true;
                match auth.refresh_chatgpt_oauth().await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return Err(ProviderAttemptFailure {
                            provider: provider_id,
                            kind: ProviderErrorKind::Fatal,
                            category: StreamErrorCategory::Auth,
                            attempts,
                            message:
                                "ChatGPT access token was rejected and the saved login disappeared"
                                    .to_string(),
                            suggestion: Some(
                                "run `orbcode auth login --provider openai --method chatgpt` again"
                                    .to_string(),
                            ),
                            started_content: false,
                        });
                    }
                    Err(refresh_error) => {
                        return Err(ProviderAttemptFailure {
                            provider: provider_id,
                            kind: ProviderErrorKind::Fatal,
                            category: StreamErrorCategory::Auth,
                            attempts,
                            message: format!(
                                "ChatGPT access token was rejected and refresh failed: {refresh_error}"
                            ),
                            suggestion: Some(
                                "run `orbcode auth login --provider openai --method chatgpt` again"
                                    .to_string(),
                            ),
                            started_content: false,
                        });
                    }
                }
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
                let suggestion =
                    if provider_request.options.openai_wire_mode == OpenAiWireMode::Responses {
                        chatgpt_error_suggestion(error.status, &error.message).or(error.suggestion)
                    } else {
                        error.suggestion
                    };
                return Err(ProviderAttemptFailure {
                    provider: provider_id,
                    kind: error.kind,
                    category: error.category,
                    attempts,
                    message: error.message,
                    suggestion,
                    started_content: attempt_sink.started_content,
                });
            }
        }
    }
}

fn chatgpt_error_suggestion(status: Option<u16>, message: &str) -> Option<String> {
    let normalized = message.to_ascii_lowercase();
    if status == Some(401) {
        return Some(
            "run `orbcode auth login --provider openai --method chatgpt` again".to_string(),
        );
    }
    if status == Some(403) {
        return Some(
            "verify that this ChatGPT account and workspace include Codex access, then sign in again"
                .to_string(),
        );
    }
    if normalized.contains("insufficient_quota") || normalized.contains("usage_not_included") {
        return Some(
            "this ChatGPT plan does not currently include the requested Codex usage or model; check plan limits or choose an available model"
                .to_string(),
        );
    }
    if status == Some(429) {
        return Some(
            "the ChatGPT subscription usage limit was reached; wait for its reset window or choose another provider"
                .to_string(),
        );
    }
    None
}

async fn configure_chatgpt_request(
    config: &AppConfig,
    auth: &AuthManager,
    provider: ProviderId,
    request: &mut ProviderRequest,
) -> Result<(), String> {
    if provider != ProviderId::OpenAi || request.api_key.is_some() {
        return Ok(());
    }
    let credentials = auth
        .resolve_chatgpt_oauth()
        .await
        .map_err(|error| error.to_string())?;
    let Some(credentials) = credentials else {
        return Ok(());
    };
    request.base_url = auth.chatgpt_codex_base_url().to_string();
    if request.options.proxy_resolved_from_config {
        match config.outbound_proxy_route(&request.base_url) {
            OutboundProxyRoute::Direct => {
                request.options.proxy = None;
                request.options.proxy_no_proxy = None;
            }
            OutboundProxyRoute::Proxy { url, no_proxy } => {
                request.options.proxy = Some(url);
                request.options.proxy_no_proxy = no_proxy;
            }
        }
    }
    request.api_key = None;
    request.auth_token = Some(credentials.access_token);
    request.options.openai_wire_mode = OpenAiWireMode::Responses;
    request.options.openai_account_id = credentials.account_id;
    if !config.provider_model_is_explicit() {
        request.model = "gpt-5.6-sol".to_string();
    }
    Ok(())
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
    use orbcode_config::{AppConfigOverrides, sealed_provider_env_overrides};
    use orbcode_model_provider::{OpenAiWireMode, ProviderRequest};
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

    #[tokio::test]
    async fn chatgpt_credentials_select_responses_endpoint_and_subscription_default_model() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        std::fs::write(
            home.path().join("auth.json"),
            format!(
                r#"{{"entries":[{{"provider":"openai","method":"chatgpt","source":{{"kind":"chatgpt_oauth","credentials":{{"id_token":"id","access_token":"access","refresh_token":"refresh","expires_at":{},"account_id":"account-123","email":null,"plan_type":"plus"}}}},"updated_at":"2026-08-03T00:00:00Z"}}]}}"#,
                chrono::Utc::now().timestamp_millis() + 60 * 60 * 1000
            ),
        )
        .expect("auth store");
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"https_proxy":"http://settings-proxy.invalid:9000"}}"#,
        )
        .expect("settings");
        let config = AppConfig::load(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                default_provider: Some(ProviderId::OpenAi),
                env_overrides: sealed_provider_env_overrides(),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("config");
        let auth = AuthManager::new(home.path().to_path_buf());
        let mut request = make_request_with_thinking();
        request.api_key = None;
        request.auth_token = None;
        request.base_url = "http://plain-endpoint.invalid".to_string();
        request.options.proxy_resolved_from_config = true;

        configure_chatgpt_request(&config, &auth, ProviderId::OpenAi, &mut request)
            .await
            .expect("configure");

        assert_eq!(request.model, "gpt-5.6-sol");
        assert_eq!(request.base_url, orbcode_config::CHATGPT_CODEX_BASE_URL);
        assert_eq!(request.auth_token.as_deref(), Some("access"));
        assert_eq!(request.options.openai_wire_mode, OpenAiWireMode::Responses);
        assert_eq!(
            request.options.proxy.as_deref(),
            Some("http://settings-proxy.invalid:9000")
        );
        assert_eq!(
            request.options.openai_account_id.as_deref(),
            Some("account-123")
        );
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

    #[test]
    fn chatgpt_errors_have_subscription_specific_recovery_hints() {
        assert!(
            chatgpt_error_suggestion(Some(403), "forbidden")
                .expect("403 hint")
                .contains("workspace")
        );
        assert!(
            chatgpt_error_suggestion(Some(429), "rate limited")
                .expect("429 hint")
                .contains("subscription usage limit")
        );
        assert!(
            chatgpt_error_suggestion(Some(400), "usage_not_included")
                .expect("entitlement hint")
                .contains("plan")
        );
    }
}
