use orbcode_config::{AppConfig, prompt_too_long_preflight_message};
use orbcode_model_provider::{
    CountTokensCacheKey, ProviderRequest, ProviderRequestOptions, provider_for,
};
use orbcode_protocol::{ProviderId, TranscriptMessage, TurnContext, token_count_with_estimation};
use orbcode_tools::InteractionToolVisibility;

use super::SessionManager;
use crate::{
    CoreError,
    config_provider::AppConfigProviderRequestExt,
    system_prompt::{append_dynamic_workflow_planning_section, build_system_prompt},
};

impl SessionManager {
    pub(super) async fn provider_request_for_session(
        &self,
        session_id: &str,
        prompt: &str,
        context: TurnContext,
        synthetic_messages: &[TranscriptMessage],
        expose_tools: bool,
        expose_network_tools: bool,
    ) -> Result<ProviderRequest, CoreError> {
        let mut session = self.load_session(session_id).await?;
        session.messages.extend_from_slice(synthetic_messages);
        let messages = self
            .model_visible_messages_with_tool_result_budget(session_id, session.messages)
            .await?;
        Ok(self
            .provider_request_for_messages(
                session_id,
                prompt,
                context,
                messages,
                expose_tools,
                expose_network_tools,
            )
            .await)
    }

    pub(super) async fn provider_request_for_messages(
        &self,
        session_id: &str,
        prompt: &str,
        context: TurnContext,
        messages: Vec<TranscriptMessage>,
        expose_tools: bool,
        expose_network_tools: bool,
    ) -> ProviderRequest {
        let permissions = self.permission_context();
        let config = self.effective_config();
        let resolution = config.provider_model_resolution(config.default_provider);
        let ask_user_question = self
            .active_turns
            .interaction_context(session_id)
            .await
            .is_some_and(|(_, interaction)| interaction.capabilities.fully_supported());
        let tools = self
            .tools
            .provider_definitions_with_mcp_for_session_and_interactions(
                expose_tools,
                expose_network_tools,
                &self.mcp,
                session_id,
                InteractionToolVisibility { ask_user_question },
            )
            .await
            .into_iter()
            .filter(|tool| permissions.tool_visible(&tool.name))
            .collect::<Vec<_>>();
        let mut system_prompt = build_system_prompt(&context);
        if let Some(section) = self.active_output_style().system_prompt_section() {
            system_prompt.push_str(&section);
        }
        append_dynamic_workflow_planning_section(&mut system_prompt, &tools);
        // `--append-system-prompt` (config.append_system_prompt) is loaded and
        // forwarded to child processes but was never concatenated into the
        // actual request system prompt — append it so the directive reaches the
        // model.
        if let Some(extra) = self.config.append_system_prompt.as_deref() {
            let extra = extra.trim();
            if !extra.is_empty() {
                system_prompt.push_str("\n\n");
                system_prompt.push_str(extra);
            }
        }
        let mut request = ProviderRequest {
            session_id: session_id.to_string(),
            prompt: prompt.to_string(),
            system_prompt,
            context,
            messages,
            tools,
            model: resolution.request_model,
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: self.runtime_effort_override(),
            options: ProviderRequestOptions::default(),
        };
        request.options.max_thinking_tokens = self.max_thinking_tokens();
        config.configure_provider_request(config.default_provider, &mut request);
        request
    }

    pub(super) async fn prompt_too_long_preflight_error(
        &self,
        request: &ProviderRequest,
        config: &AppConfig,
    ) -> Option<String> {
        let estimated_tokens = self
            .estimated_context_tokens_for_request(config.default_provider, request)
            .await;
        prompt_too_long_preflight_message(
            estimated_tokens,
            &request.model,
            &config.context_window_options(),
            &config.max_output_token_options(),
            &config.token_warning_options(),
        )
    }

    async fn estimated_context_tokens_for_request(
        &self,
        provider: ProviderId,
        request: &ProviderRequest,
    ) -> u32 {
        let config = self.effective_config();
        let count_tokens_request =
            self.count_tokens_request_for_provider(provider, request, &config);
        match self
            .count_tokens_cached(provider, &count_tokens_request)
            .await
        {
            Ok(Some(tokens)) => tokens.min(u32::MAX as usize) as u32,
            Ok(None) | Err(_) => token_count_with_estimation(&request.messages),
        }
    }

    /// Count tokens for `request`, memoizing the result per
    /// `(model, tool_schema_hash, message_hash)` so repeated estimations within
    /// a turn do not each pay a network round-trip. Cache hits and misses are
    /// recorded on the shared [`orbcode_model_provider::CountTokensCache`].
    pub(super) async fn count_tokens_cached(
        &self,
        provider: ProviderId,
        count_tokens_request: &ProviderRequest,
    ) -> Result<Option<usize>, orbcode_model_provider::ProviderError> {
        let key = count_tokens_cache_key(count_tokens_request);
        if let Some(tokens) = self.count_tokens_cache.get(key) {
            return Ok(Some(tokens));
        }
        let result = provider_for(provider)
            .count_tokens(count_tokens_request)
            .await;
        if let Ok(Some(tokens)) = &result {
            self.count_tokens_cache.insert(key, *tokens);
        }
        result
    }

    pub(super) fn count_tokens_request_for_provider(
        &self,
        provider: ProviderId,
        request: &ProviderRequest,
        config: &AppConfig,
    ) -> ProviderRequest {
        let mut request = request.clone();
        request.model = config.small_fast_model_name(provider);
        request
    }
}

/// Derive a stable cache key from the request's model, tool schemas, and
/// messages by structurally hashing the fields without large intermediate
/// JSON string allocations.
fn count_tokens_cache_key(request: &ProviderRequest) -> CountTokensCacheKey {
    CountTokensCacheKey::from_provider_request(request)
}
