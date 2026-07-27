use orbcode_config::AppConfig;
use orbcode_model_provider::{
    ProviderCancellationToken, ProviderError, ProviderRequest, ProviderRequestOptions,
    StreamErrorCategory, probe_provider, suggestion_for,
};
use orbcode_protocol::{ProviderId, TurnContext};

use super::{DoctorCheck, DoctorStatus, is_truthy};

const PROVIDER_PROBE_CHECK: &str = "provider_probe";

/// Outcome of a single provider probe attempt. `MissingCredentials` is reached
/// without any network call: if nothing resolves to a usable key/token we know
/// the probe cannot succeed, so we report it as a (skippable) Warn instead of
/// firing a doomed request.
enum ProbeOutcome {
    Ok,
    MissingCredentials,
    Failed(ProviderError),
}

/// Live provider/auth probe. Fires one minimal request (capped to a single
/// output token, response discarded) through the real provider path so the
/// shared `ProviderError` classifier surfaces auth/network/rate-limit issues
/// ahead of a real turn. It does not start a tool loop, persist a session, or
/// inject any prompt markers.
///
/// The probe is opt-in (`ORBCODE_DOCTOR_PROBE`) and never blocks the rest of the
/// report: when disabled, when provider network access is off, or when no
/// credentials are configured, it reports Warn rather than Fail.
pub(super) async fn provider_probe_check(config: &AppConfig) -> DoctorCheck {
    if !probe_enabled(config) {
        return probe_skip(
            "skipped; set ORBCODE_DOCTOR_PROBE=1 to fire a live provider probe (one ~1-token request, no full turn)",
        );
    }
    if !config.provider_allow_network {
        return probe_skip(
            "skipped; provider network access is disabled — enable it to run a live probe",
        );
    }

    let default = config.default_provider;
    let default_outcome = probe_one(config, default).await;
    let (default_status, default_detail) = outcome_summary(default, &default_outcome);
    if matches!(default_outcome, ProbeOutcome::Ok) {
        return DoctorCheck {
            name: PROVIDER_PROBE_CHECK.to_string(),
            status: default_status,
            detail: default_detail,
        };
    }

    match config.fallback_provider {
        Some(fallback) if fallback != default => {
            let fallback_outcome = probe_one(config, fallback).await;
            let (fallback_status, fallback_detail) = outcome_summary(fallback, &fallback_outcome);
            if matches!(fallback_outcome, ProbeOutcome::Ok) {
                DoctorCheck {
                    name: PROVIDER_PROBE_CHECK.to_string(),
                    status: DoctorStatus::Warn,
                    detail: format!(
                        "default unavailable ({default_detail}); fallback OK ({fallback_detail})"
                    ),
                }
            } else {
                DoctorCheck {
                    name: PROVIDER_PROBE_CHECK.to_string(),
                    status: worst_status(default_status, fallback_status),
                    detail: format!("{default_detail}; {fallback_detail}"),
                }
            }
        }
        _ => DoctorCheck {
            name: PROVIDER_PROBE_CHECK.to_string(),
            status: default_status,
            detail: default_detail,
        },
    }
}

fn probe_skip(detail: &str) -> DoctorCheck {
    DoctorCheck {
        name: PROVIDER_PROBE_CHECK.to_string(),
        status: DoctorStatus::Warn,
        detail: detail.to_string(),
    }
}

fn probe_enabled(config: &AppConfig) -> bool {
    config
        .resolve_env("ORBCODE_DOCTOR_PROBE")
        .is_some_and(|value| is_truthy(&value))
}

async fn probe_one(config: &AppConfig, provider: ProviderId) -> ProbeOutcome {
    let request = build_probe_request(config, provider);
    if request.api_key.is_none() && request.auth_token.is_none() {
        return ProbeOutcome::MissingCredentials;
    }
    let report = probe_provider(provider, &request, ProviderCancellationToken::default()).await;
    match report.error {
        None => ProbeOutcome::Ok,
        Some(error) => ProbeOutcome::Failed(error),
    }
}

/// Build a minimal probe request for `provider` from config. The credential
/// precedence mirrors the session request path, but only reads config — it does
/// not depend on `core`'s internal request builder.
fn build_probe_request(config: &AppConfig, provider: ProviderId) -> ProviderRequest {
    let mut request = ProviderRequest {
        session_id: "doctor-probe".to_string(),
        prompt: "ping".to_string(),
        context: TurnContext::default(),
        messages: Vec::new(),
        system_prompt: String::new(),
        tools: Vec::new(),
        model: String::new(),
        base_url: String::new(),
        api_key: None,
        auth_token: None,
        // Leave thinking unset (no disable marker, no effort budget) so the
        // request body stays as vanilla as possible — some relays reject the
        // `thinking: {type: "disabled"}` shape, which would mask the real
        // auth/network/rate-limit signal we want from the probe.
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions {
            max_output_tokens: Some(1),
            ..ProviderRequestOptions::default()
        },
    };

    match provider {
        ProviderId::Anthropic => {
            request.model = config.provider_model_resolution(provider).request_model;
            request.base_url = config.anthropic_base_url();
            if let Some(token) = config.anthropic_auth_token() {
                request.auth_token = Some(token);
            } else if let Some(key) = config.anthropic_api_key() {
                request.api_key = Some(key);
            } else {
                request.auth_token = config.anthropic_oauth_token();
            }
        }
        ProviderId::OpenAi => {
            request.model = config.provider_model_resolution(provider).request_model;
            request.base_url = config.openai_base_url();
            request.api_key = config.openai_api_key();
        }
        _ => {}
    }

    request
}

fn outcome_summary(provider: ProviderId, outcome: &ProbeOutcome) -> (DoctorStatus, String) {
    match outcome {
        ProbeOutcome::Ok => (
            DoctorStatus::Pass,
            format!("{provider} live probe succeeded (1-token request, response discarded)"),
        ),
        ProbeOutcome::MissingCredentials => (
            DoctorStatus::Warn,
            format!(
                "{provider}: no usable credentials to probe — {}",
                suggestion_for(provider, StreamErrorCategory::Auth, None)
            ),
        ),
        ProbeOutcome::Failed(error) => (
            probe_status_for_category(error.category),
            format!("{provider} probe failed: {}", error.rendered_message()),
        ),
    }
}

/// Maps a probe error category to a doctor status. Auth problems are the user's
/// to fix (Fail); transient/environmental categories (network, rate-limit,
/// overload, server errors) and probe-request-shape issues stay Warn so the
/// probe never blocks the rest of the report on a passing setup.
fn probe_status_for_category(category: StreamErrorCategory) -> DoctorStatus {
    match category {
        StreamErrorCategory::Auth
        | StreamErrorCategory::AccountSuspended
        | StreamErrorCategory::UnsupportedProvider => DoctorStatus::Fail,
        _ => DoctorStatus::Warn,
    }
}

fn worst_status(a: DoctorStatus, b: DoctorStatus) -> DoctorStatus {
    fn rank(status: DoctorStatus) -> u8 {
        match status {
            DoctorStatus::Pass => 0,
            DoctorStatus::Fail => 2,
            _ => 1,
        }
    }
    if rank(a) >= rank(b) { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::tests::config;
    use orbcode_model_provider::ProviderErrorKind;

    fn probe_error(
        category: StreamErrorCategory,
        provider: ProviderId,
        message: &str,
    ) -> ProviderError {
        ProviderError {
            kind: ProviderErrorKind::Fatal,
            category,
            provider: None,
            status: None,
            message: message.to_string(),
            suggestion: None,
            rate_limit: None,
        }
        .with_provider(provider)
    }

    #[test]
    fn probe_status_classification_matches_severity() {
        assert_eq!(
            probe_status_for_category(StreamErrorCategory::Auth),
            DoctorStatus::Fail
        );
        assert_eq!(
            probe_status_for_category(StreamErrorCategory::AccountSuspended),
            DoctorStatus::Fail
        );
        for category in [
            StreamErrorCategory::Network,
            StreamErrorCategory::RateLimit,
            StreamErrorCategory::Overload,
            StreamErrorCategory::ServerError,
            StreamErrorCategory::InvalidRequest,
        ] {
            assert_eq!(
                probe_status_for_category(category),
                DoctorStatus::Warn,
                "{category:?} should be a Warn"
            );
        }
    }

    #[test]
    fn outcome_summary_classifies_expired_oauth_as_fail_with_suggestion() {
        let outcome = ProbeOutcome::Failed(probe_error(
            StreamErrorCategory::Auth,
            ProviderId::Anthropic,
            "OAuth token expired",
        ));
        let (status, detail) = outcome_summary(ProviderId::Anthropic, &outcome);
        assert_eq!(status, DoctorStatus::Fail);
        assert!(detail.contains("auth"));
        assert!(detail.to_ascii_lowercase().contains("oauth"));
    }

    #[test]
    fn outcome_summary_classifies_network_unreachable_as_warn() {
        let outcome = ProbeOutcome::Failed(probe_error(
            StreamErrorCategory::Network,
            ProviderId::Anthropic,
            "connection timed out",
        ));
        let (status, detail) = outcome_summary(ProviderId::Anthropic, &outcome);
        assert_eq!(status, DoctorStatus::Warn);
        assert!(detail.contains("network"));
        assert!(detail.contains("connectivity"));
    }

    #[test]
    fn outcome_summary_classifies_rate_limit_as_warn() {
        let outcome = ProbeOutcome::Failed(probe_error(
            StreamErrorCategory::RateLimit,
            ProviderId::Anthropic,
            "429 too many requests",
        ));
        let (status, detail) = outcome_summary(ProviderId::Anthropic, &outcome);
        assert_eq!(status, DoctorStatus::Warn);
        assert!(detail.contains("rate_limit"));
        assert!(detail.contains("rate limited"));
    }

    #[test]
    fn outcome_summary_treats_missing_credentials_as_warn_with_suggestion() {
        let (status, detail) =
            outcome_summary(ProviderId::Anthropic, &ProbeOutcome::MissingCredentials);
        assert_eq!(status, DoctorStatus::Warn);
        assert!(detail.contains("no usable credentials"));
        assert!(detail.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn build_probe_request_has_no_credentials_when_config_is_empty() {
        let mut config = config();
        for key in [
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            config.env_overrides.insert(key.to_string(), String::new());
        }
        let request = build_probe_request(&config, ProviderId::Anthropic);
        assert!(request.api_key.is_none());
        assert!(request.auth_token.is_none());
        assert_eq!(request.options.max_output_tokens, Some(1));
    }

    #[test]
    fn build_probe_request_prefers_auth_token_for_anthropic() {
        let mut config = config();
        config.env_overrides.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "external-token".to_string(),
        );
        let request = build_probe_request(&config, ProviderId::Anthropic);
        assert_eq!(request.auth_token.as_deref(), Some("external-token"));
        assert!(request.api_key.is_none());
    }

    #[tokio::test]
    async fn provider_probe_warns_when_provider_network_is_disabled() {
        let mut config = config();
        config
            .env_overrides
            .insert("ORBCODE_DOCTOR_PROBE".to_string(), "1".to_string());
        config.provider_allow_network = false;

        let check = provider_probe_check(&config).await;
        assert_eq!(check.name, "provider_probe");
        assert_eq!(check.status, DoctorStatus::Warn);
        assert!(check.detail.contains("network access is disabled"));
    }

    #[test]
    fn outcome_summary_classifies_account_suspended_as_fail() {
        let outcome = ProbeOutcome::Failed(probe_error(
            StreamErrorCategory::AccountSuspended,
            ProviderId::Anthropic,
            "account suspended",
        ));
        let (status, detail) = outcome_summary(ProviderId::Anthropic, &outcome);
        assert_eq!(status, DoctorStatus::Fail);
        assert!(detail.contains("account_suspended"));
    }
}
