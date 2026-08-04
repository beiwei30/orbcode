use orbcode_config::{AppConfig, OutboundProxyRoute};
use orbcode_protocol::ProviderId;

use orbcode_model_provider::ProviderRequest;

pub(crate) trait AppConfigProviderRequestExt {
    fn configure_provider_request(&self, provider: ProviderId, request: &mut ProviderRequest);
}

impl AppConfigProviderRequestExt for AppConfig {
    fn configure_provider_request(&self, provider: ProviderId, request: &mut ProviderRequest) {
        match provider {
            ProviderId::Anthropic => {
                request.model = self.provider_model_resolution(provider).request_model;
                request.base_url = self.anthropic_base_url();
                if let Some(auth_token) = self.anthropic_auth_token() {
                    request.api_key = None;
                    request.auth_token = Some(auth_token);
                } else if let Some(api_key) = self.anthropic_api_key() {
                    request.api_key = Some(api_key);
                    request.auth_token = None;
                } else {
                    request.api_key = None;
                    request.auth_token = self.anthropic_oauth_token();
                }
            }
            ProviderId::OpenAi => {
                request.model = self.provider_model_resolution(provider).request_model;
                request.base_url = self.openai_base_url();
                request.api_key = self.openai_api_key();
                request.auth_token = None;
            }
            _ => {}
        }
        self.apply_request_options(provider, request);
    }
}

/// Apply the transport and provider-specific request options shared by normal
/// turns and diagnostic probes after the request's final base URL is known.
pub fn apply_provider_request_options(
    config: &AppConfig,
    provider: ProviderId,
    request: &mut ProviderRequest,
) {
    config.apply_request_options(provider, request);
}

trait AppConfigProviderRequestOptionsExt {
    fn apply_request_options(&self, provider: ProviderId, request: &mut ProviderRequest);
}

impl AppConfigProviderRequestOptionsExt for AppConfig {
    fn apply_request_options(&self, provider: ProviderId, request: &mut ProviderRequest) {
        let options = &mut request.options;
        if options.max_output_tokens.is_none() {
            options.max_output_tokens = self.max_output_token_options().max_output_tokens_override;
        }
        if options.user_agent.is_none() {
            options.user_agent = self.provider_user_agent();
        }
        if options.proxy.is_none() || options.proxy_resolved_from_config {
            match self.outbound_proxy_route(&request.base_url) {
                OutboundProxyRoute::Direct => {
                    options.proxy = None;
                    options.proxy_no_proxy = None;
                }
                OutboundProxyRoute::Proxy { url, no_proxy } => {
                    options.proxy = Some(url);
                    options.proxy_no_proxy = no_proxy;
                }
            }
            options.proxy_resolved_from_config = true;
        }
        if options.timeout.is_none() {
            options.timeout = self.api_timeout();
        }
        if options.max_retries.is_none() {
            options.max_retries = self.api_max_retries();
        }
        if options.custom_headers.is_empty() {
            options.custom_headers = self.custom_headers();
        }
        match provider {
            ProviderId::Anthropic => {
                if options.anthropic_betas.is_empty() {
                    options.anthropic_betas = self.anthropic_betas();
                }
                if options.extra_body.is_empty() {
                    options.extra_body = self.extra_body();
                }
                if options.metadata.is_none() {
                    options.metadata = Some(self.anthropic_metadata(&request.session_id));
                }
            }
            _ => {
                if options.extra_body.is_empty() {
                    options.extra_body = self.extra_body();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use orbcode_config::{AppConfigOverrides, AuthManager, AuthMethod};
    use orbcode_model_provider::{ProviderRequest, ProviderRequestOptions};
    use orbcode_protocol::TurnContext;

    fn empty_request() -> ProviderRequest {
        ProviderRequest {
            session_id: "session".to_string(),
            prompt: String::new(),
            context: TurnContext::default(),
            messages: Vec::new(),
            system_prompt: String::new(),
            tools: Vec::new(),
            model: String::new(),
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: None,
            options: ProviderRequestOptions::default(),
        }
    }

    fn seal_provider_env(config: &mut AppConfig) {
        for key in [
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "CLAUDE_CODE_BETAS",
            "CLAUDE_CODE_EXTRA_BODY",
            "CLAUDE_CODE_EXTRA_METADATA",
            "ANTHROPIC_CUSTOM_HEADERS",
            "CLAUDE_CODE_USER_AGENT",
            "USER_AGENT",
            "API_TIMEOUT_MS",
            "API_MAX_RETRIES",
            "ANTHROPIC_PROXY_URL",
            "CLAUDE_CODE_PROXY",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ] {
            config.env_overrides.insert(key.to_string(), String::new());
        }
    }

    #[tokio::test]
    async fn anthropic_request_prefers_external_auth_token_over_api_key_and_oauth() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"external-token","ANTHROPIC_API_KEY":"api-key","CLAUDE_CODE_OAUTH_TOKEN":"oauth-token"}}"#,
        )
        .expect("settings");
        let mut config = AppConfig::load(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                ..Default::default()
            },
        )
        .await
        .expect("config");
        seal_provider_env(&mut config);

        let mut request = empty_request();
        config.configure_provider_request(ProviderId::Anthropic, &mut request);

        assert_eq!(request.auth_token.as_deref(), Some("external-token"));
        assert_eq!(request.api_key, None);
    }

    #[tokio::test]
    async fn anthropic_request_prefers_api_key_over_oauth() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"api-key","CLAUDE_CODE_OAUTH_TOKEN":"oauth-token"}}"#,
        )
        .expect("settings");
        let mut config = AppConfig::load(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                ..Default::default()
            },
        )
        .await
        .expect("config");
        seal_provider_env(&mut config);

        let mut request = empty_request();
        config.configure_provider_request(ProviderId::Anthropic, &mut request);

        assert_eq!(request.api_key.as_deref(), Some("api-key"));
        assert_eq!(request.auth_token, None);
    }

    #[tokio::test]
    async fn anthropic_request_uses_oauth_when_no_external_key_exists() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"CLAUDE_CODE_OAUTH_TOKEN":"oauth-token"}}"#,
        )
        .expect("settings");
        let mut config = AppConfig::load(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                ..Default::default()
            },
        )
        .await
        .expect("config");
        seal_provider_env(&mut config);

        let mut request = empty_request();
        config.configure_provider_request(ProviderId::Anthropic, &mut request);

        assert_eq!(request.auth_token.as_deref(), Some("oauth-token"));
        assert_eq!(request.api_key, None);
    }

    #[tokio::test]
    async fn anthropic_request_uses_stored_api_key_login_before_oauth() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"CLAUDE_CODE_OAUTH_TOKEN":"oauth-token"}}"#,
        )
        .expect("settings");
        AuthManager::new(home.path().to_path_buf())
            .login(
                ProviderId::Anthropic,
                AuthMethod::ApiKey,
                Some("stored-api-key".to_string()),
                None,
            )
            .await
            .expect("login");
        let mut config = AppConfig::load(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                ..Default::default()
            },
        )
        .await
        .expect("config");
        seal_provider_env(&mut config);

        let mut request = empty_request();
        config.configure_provider_request(ProviderId::Anthropic, &mut request);

        assert_eq!(request.api_key.as_deref(), Some("stored-api-key"));
        assert_eq!(request.auth_token, None);
    }

    #[tokio::test]
    async fn provider_request_preserves_programmatic_proxy_override() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let mut config = AppConfig::load(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                ..Default::default()
            },
        )
        .await
        .expect("config");
        seal_provider_env(&mut config);

        let mut request = empty_request();
        request.options.proxy = Some("http://programmatic-proxy.invalid:7000".to_string());
        request.options.proxy_no_proxy = Some("programmatic.internal".to_string());
        config.configure_provider_request(ProviderId::Anthropic, &mut request);

        assert_eq!(
            request.options.proxy.as_deref(),
            Some("http://programmatic-proxy.invalid:7000")
        );
        assert_eq!(
            request.options.proxy_no_proxy.as_deref(),
            Some("programmatic.internal")
        );
        assert!(!request.options.proxy_resolved_from_config);
    }

    #[tokio::test]
    async fn anthropic_request_inherits_options_from_config_env() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        std::fs::write(
            home.path().join("settings.json"),
            r#"{
                "env": {
                    "ANTHROPIC_API_KEY": "api-key",
                    "CLAUDE_CODE_MAX_OUTPUT_TOKENS": "4096",
                    "CLAUDE_CODE_BETAS": "context-1m-2025-01-14,prompt-caching-2024-07-31",
                    "CLAUDE_CODE_EXTRA_BODY": "{\"flag\":true}",
                    "ANTHROPIC_CUSTOM_HEADERS": "X-Cc-Test: yes",
                    "CLAUDE_CODE_USER_AGENT": "orbcode/test",
                    "API_TIMEOUT_MS": "12345",
                    "https_proxy": "http://proxy.invalid:9000",
                    "no_proxy": "internal.example"
                }
            }"#,
        )
        .expect("settings");
        let mut config = AppConfig::load(
            cwd.path(),
            AppConfigOverrides {
                home_dir: Some(home.path().to_path_buf()),
                ..Default::default()
            },
        )
        .await
        .expect("config");
        seal_provider_env(&mut config);

        let mut request = empty_request();
        config.configure_provider_request(ProviderId::Anthropic, &mut request);

        assert_eq!(request.options.max_output_tokens, Some(4096));
        assert_eq!(
            request.options.anthropic_betas,
            vec![
                "context-1m-2025-01-14".to_string(),
                "prompt-caching-2024-07-31".to_string(),
            ]
        );
        assert_eq!(
            request.options.extra_body.get("flag"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            request.options.custom_headers,
            vec![("X-Cc-Test".to_string(), "yes".to_string())]
        );
        assert_eq!(request.options.user_agent.as_deref(), Some("orbcode/test"));
        assert_eq!(
            request.options.timeout,
            Some(std::time::Duration::from_millis(12345))
        );
        assert_eq!(
            request.options.proxy.as_deref(),
            Some("http://proxy.invalid:9000")
        );
        assert_eq!(
            request.options.proxy_no_proxy.as_deref(),
            Some("internal.example,localhost,127.0.0.1,::1")
        );
        assert!(request.options.proxy_resolved_from_config);
        let metadata = request
            .options
            .metadata
            .as_ref()
            .expect("metadata populated");
        let user_id = metadata["user_id"]
            .as_str()
            .expect("metadata.user_id is a JSON-encoded string");
        let parsed: serde_json::Value =
            serde_json::from_str(user_id).expect("user_id parses as JSON");
        assert_eq!(parsed["session_id"], serde_json::json!("session"));
    }
}
