use std::fmt;

use orbcode_protocol::{ProviderId, StreamErrorCategory};
use serde::Deserialize;

use crate::rate_limit::RateLimitMetadata;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderErrorKind {
    Retryable,
    Fatal,
    Interrupted,
}

#[derive(Clone, Debug)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub category: StreamErrorCategory,
    pub provider: Option<ProviderId>,
    pub status: Option<u16>,
    pub message: String,
    pub suggestion: Option<String>,
    /// Rate-limit metadata parsed from the provider response headers/body, when
    /// present. The retry loop uses this to honor `Retry-After` and unified
    /// rate-limit reset windows. Boxed to keep `ProviderError` small in the
    /// common (no-metadata) case.
    pub rate_limit: Option<Box<RateLimitMetadata>>,
}

impl ProviderError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            kind: ProviderErrorKind::Fatal,
            category: StreamErrorCategory::Other,
            provider: None,
            status: None,
            message: message.into(),
            suggestion: None,
            rate_limit: None,
        }
    }

    pub fn unsupported_provider(provider: ProviderId) -> Self {
        Self {
            kind: ProviderErrorKind::Fatal,
            category: StreamErrorCategory::UnsupportedProvider,
            provider: Some(provider),
            status: None,
            message: format!(
                "Provider '{provider}' is not supported. Use 'anthropic' or 'openai' instead."
            ),
            suggestion: Some(
                "Switch your provider setting to 'anthropic' or 'openai'. \
                 The provider ID is retained for config/transcript compatibility only."
                    .to_string(),
            ),
            rate_limit: None,
        }
    }

    pub fn interrupted(message: impl Into<String>) -> Self {
        Self {
            kind: ProviderErrorKind::Interrupted,
            category: StreamErrorCategory::Interrupted,
            provider: None,
            status: None,
            message: message.into(),
            suggestion: None,
            rate_limit: None,
        }
    }

    pub fn auth(provider: ProviderId, message: impl Into<String>) -> Self {
        let category = StreamErrorCategory::Auth;
        let message = message.into();
        let suggestion = suggestion_for_message(provider, category, None, &message);
        Self {
            kind: ProviderErrorKind::Fatal,
            category,
            provider: Some(provider),
            status: None,
            message,
            suggestion: Some(suggestion),
            rate_limit: None,
        }
    }

    /// Attach rate-limit metadata parsed from a provider response. A `None` or
    /// empty value leaves the error unchanged so callers can pass parsed
    /// metadata unconditionally.
    pub fn with_rate_limit(mut self, rate_limit: RateLimitMetadata) -> Self {
        if !rate_limit.is_empty() {
            self.rate_limit = Some(Box::new(rate_limit));
        }
        self
    }

    /// Seconds the server asked us to wait before retrying, if a `Retry-After`
    /// header was present on the response.
    pub fn retry_after_secs(&self) -> Option<u64> {
        self.rate_limit
            .as_ref()
            .and_then(|meta| meta.retry_after_secs)
    }

    pub fn with_provider(mut self, provider: ProviderId) -> Self {
        if self.provider.is_none() {
            self.provider = Some(provider);
        }
        if self.suggestion.is_none() {
            self.suggestion = Some(suggestion_for_message(
                provider,
                self.category,
                self.status,
                &self.message,
            ));
        }
        self
    }

    pub fn rendered_message(&self) -> String {
        let mut out = String::new();
        if let Some(provider) = self.provider {
            out.push('[');
            out.push_str(&provider.to_string());
            out.push_str("] ");
        }
        out.push_str(self.category.label());
        out.push_str(": ");
        out.push_str(&self.message);
        if let Some(suggestion) = &self.suggestion {
            out.push_str(" — ");
            out.push_str(suggestion);
        }
        out
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.rendered_message())
    }
}

impl std::error::Error for ProviderError {}

pub fn classify_http_error(status: u16, message: &str) -> ProviderErrorKind {
    classify_provider_error(None, Some(status), message).kind
}

#[derive(Clone, Debug)]
pub struct ClassifiedProviderError {
    pub kind: ProviderErrorKind,
    pub category: StreamErrorCategory,
    pub suggestion: Option<String>,
}

pub fn classify_provider_error(
    provider: Option<ProviderId>,
    status: Option<u16>,
    message: &str,
) -> ClassifiedProviderError {
    let category = detect_category(status, message);
    let kind = kind_for_category(category, status, message);
    let suggestion =
        provider.map(|provider| suggestion_for_message(provider, category, status, message));
    ClassifiedProviderError {
        kind,
        category,
        suggestion,
    }
}

fn detect_category(status: Option<u16>, message: &str) -> StreamErrorCategory {
    let lower = message.to_ascii_lowercase();

    if matches!(status, Some(401 | 403))
        || lower.contains("authentication_error")
        || lower.contains("permission_error")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("oauth")
            && (lower.contains("expired") || lower.contains("invalid") || lower.contains("missing"))
    {
        return StreamErrorCategory::Auth;
    }

    if matches!(status, Some(402)) {
        return StreamErrorCategory::AccountSuspended;
    }

    // Detect a permanent billing/quota-exhaustion failure BEFORE the RateLimit
    // branch below: the RateLimit branch matches any "quota" substring, so a
    // billing "quota exceeded" would otherwise be classified retryable and
    // retried through the whole backoff.
    if lower.contains("account suspended")
        || lower.contains("payment required")
        || (lower.contains("quota exceeded")
            && (lower.contains("account") || lower.contains("billing") || lower.contains("plan")))
    {
        return StreamErrorCategory::AccountSuspended;
    }

    if matches!(status, Some(429))
        || lower.contains("rate_limit")
        || lower.contains("rate limit")
        || lower.contains("rate-limit")
        || lower.contains("too many requests")
        || lower.contains("quota")
    {
        return StreamErrorCategory::RateLimit;
    }

    if matches!(status, Some(529))
        || lower.contains("overloaded")
        || lower.contains("server is overloaded")
        || lower.contains("temporarily unable")
    {
        return StreamErrorCategory::Overload;
    }

    if lower.contains("prompt is too long")
        || lower.contains("prompt too long")
        || lower.contains("context_length_exceeded")
        || lower.contains("maximum context length")
        || lower.contains("input is too long")
    {
        return StreamErrorCategory::PromptTooLong;
    }

    if lower.contains("max_tokens")
        && (lower.contains("exceed") || lower.contains("greater") || lower.contains("less"))
        || lower.contains("max output tokens")
        || lower.contains("max_output_tokens")
    {
        return StreamErrorCategory::MaxOutput;
    }

    if matches!(
        status,
        Some(400 | 404 | 405 | 406 | 410 | 411 | 413 | 414 | 415 | 422)
    ) || lower.contains("invalid_request_error")
        || lower.contains("invalid request")
        || lower.contains("model_not_found")
        || lower.contains("not_found_error")
    {
        return StreamErrorCategory::InvalidRequest;
    }

    if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("network")
        || lower.contains("eof")
        || lower.contains("reset by peer")
    {
        return StreamErrorCategory::Network;
    }

    if matches!(status, Some(500..=599)) {
        return StreamErrorCategory::ServerError;
    }

    if matches!(status, Some(408 | 409 | 425)) {
        return StreamErrorCategory::Network;
    }

    StreamErrorCategory::Other
}

fn kind_for_category(
    category: StreamErrorCategory,
    status: Option<u16>,
    message: &str,
) -> ProviderErrorKind {
    match category {
        StreamErrorCategory::RateLimit
        | StreamErrorCategory::Overload
        | StreamErrorCategory::Network
        | StreamErrorCategory::ServerError => ProviderErrorKind::Retryable,
        StreamErrorCategory::Interrupted => ProviderErrorKind::Interrupted,
        StreamErrorCategory::Auth
        | StreamErrorCategory::AccountSuspended
        | StreamErrorCategory::UnsupportedProvider
        | StreamErrorCategory::InvalidRequest
        | StreamErrorCategory::PromptTooLong
        | StreamErrorCategory::MaxOutput => ProviderErrorKind::Fatal,
        StreamErrorCategory::RetryExhausted | StreamErrorCategory::Other => {
            let lower = message.to_ascii_lowercase();
            if matches!(status, Some(408 | 409 | 425 | 429 | 500..=599))
                || lower.contains("timeout")
                || lower.contains("429")
                || lower.contains("529")
            {
                ProviderErrorKind::Retryable
            } else {
                ProviderErrorKind::Fatal
            }
        }
    }
}

pub fn suggestion_for(
    provider: ProviderId,
    category: StreamErrorCategory,
    _status: Option<u16>,
) -> &'static str {
    match (provider, category) {
        (ProviderId::Anthropic, StreamErrorCategory::Auth) => {
            "set ORBCODE_ANTHROPIC_AUTH_TOKEN (or ANTHROPIC_AUTH_TOKEN), ORBCODE_ANTHROPIC_API_KEY (or ANTHROPIC_API_KEY), or ORBCODE_OAUTH_TOKEN (or CLAUDE_CODE_OAUTH_TOKEN); use `orbcode auth status` to inspect stored credentials"
        }
        (ProviderId::OpenAi, StreamErrorCategory::Auth) => {
            "set ORBCODE_OPENAI_API_KEY (or OPENAI_API_KEY), and ORBCODE_OPENAI_BASE_URL (or OPENAI_BASE_URL) if using a custom endpoint"
        }
        (_, StreamErrorCategory::Auth) => {
            "verify the provider credentials in your environment or settings"
        }
        (_, StreamErrorCategory::RateLimit) => {
            "you are being rate limited; wait for the reset window or configure a fallback provider"
        }
        (_, StreamErrorCategory::AccountSuspended) => {
            "your account appears suspended or has a billing issue; check your account status and payment method at the provider dashboard"
        }
        (_, StreamErrorCategory::Overload) => {
            "the provider is overloaded; retry shortly or fall back to another provider"
        }
        (_, StreamErrorCategory::Network) => {
            "transient network error; check connectivity, proxy settings, and try again"
        }
        (_, StreamErrorCategory::ServerError) => {
            "the provider returned a server error; retry shortly"
        }
        (ProviderId::Anthropic, StreamErrorCategory::InvalidRequest) => {
            "the Anthropic API rejected the request; check the model id, headers, and message format"
        }
        (ProviderId::OpenAi, StreamErrorCategory::InvalidRequest) => {
            "the OpenAI-compatible endpoint rejected the request; check ORBCODE_OPENAI_MODEL (or OPENAI_MODEL), ORBCODE_OPENAI_BASE_URL (or OPENAI_BASE_URL), and tool/message schemas"
        }
        (_, StreamErrorCategory::InvalidRequest) => {
            "the provider rejected the request body; check model id and request shape"
        }
        (_, StreamErrorCategory::PromptTooLong) => {
            "shrink the conversation: run /compact, drop context files, or pick a model with a larger window"
        }
        (_, StreamErrorCategory::MaxOutput) => {
            "lower max output tokens for the request, or reduce the response length"
        }
        (_, StreamErrorCategory::UnsupportedProvider) => {
            "switch your provider setting to 'anthropic' or 'openai'"
        }
        (_, StreamErrorCategory::Interrupted) => {
            "the request was interrupted; rerun the turn to continue"
        }
        (_, StreamErrorCategory::RetryExhausted) => {
            "all retry attempts exhausted; consider increasing retry limits or checking provider availability"
        }
        (_, StreamErrorCategory::Other) => "review the provider response above for details",
    }
}

pub fn suggestion_for_message(
    provider: ProviderId,
    category: StreamErrorCategory,
    status: Option<u16>,
    message: &str,
) -> String {
    if provider == ProviderId::Anthropic
        && category == StreamErrorCategory::Auth
        && let Some(suggestion) = anthropic_auth_suggestion(message)
    {
        return suggestion.to_string();
    }
    suggestion_for(provider, category, status).to_string()
}

fn anthropic_auth_suggestion(message: &str) -> Option<&'static str> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("missing anthropic credentials") {
        return Some(
            "set ORBCODE_ANTHROPIC_AUTH_TOKEN, ORBCODE_ANTHROPIC_API_KEY, or ORBCODE_OAUTH_TOKEN (legacy: ANTHROPIC_AUTH_TOKEN, ANTHROPIC_API_KEY, CLAUDE_CODE_OAUTH_TOKEN); use `orbcode auth status` to inspect stored or blocked OAuth credentials",
        );
    }
    if lower.contains("expired")
        && (lower.contains("oauth") || lower.contains("token") || lower.contains("credential"))
    {
        return Some(
            "OAuth credentials appear expired; refresh the token, run `orbcode auth logout --provider anthropic` before logging in again, or set ORBCODE_ANTHROPIC_AUTH_TOKEN/ORBCODE_ANTHROPIC_API_KEY",
        );
    }
    if lower.contains("scope") || lower.contains("permission_error") && lower.contains("credential")
    {
        if lower.contains("profile") {
            return Some(
                "OAuth credentials lack the user:profile scope needed for profile/subscription access; re-login with profile scope or use an Anthropic API key",
            );
        }
        return Some(
            "OAuth credentials lack a required scope; re-login with the required Anthropic OAuth scopes or use ORBCODE_ANTHROPIC_API_KEY (or ANTHROPIC_API_KEY)",
        );
    }
    if lower.contains("subscription")
        || lower.contains("plan")
        || lower.contains("profile access")
        || lower.contains("account access")
    {
        return Some(
            "the authenticated account lacks the required Claude subscription/profile access; check the account, organization, and model access or use an Anthropic API key",
        );
    }
    None
}

pub fn parse_provider_error_body(provider: ProviderId, status: u16, body: &str) -> ProviderError {
    let extracted = extract_error_details(body, status);
    let classified = classify_provider_error(
        Some(provider),
        Some(status),
        &extracted.classification_message,
    );
    ProviderError {
        kind: classified.kind,
        category: classified.category,
        provider: Some(provider),
        status: Some(status),
        message: extracted.message,
        suggestion: classified.suggestion,
        rate_limit: None,
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: Option<ErrorDetail>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default, rename = "type")]
    error_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ErrorDetail {
    Object {
        #[serde(default)]
        message: Option<String>,
        #[serde(default, rename = "type")]
        error_type: Option<String>,
        #[serde(default)]
        code: Option<String>,
    },
    String(String),
}

struct ExtractedErrorDetails {
    message: String,
    classification_message: String,
}

fn extract_error_details(body: &str, status: u16) -> ExtractedErrorDetails {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        let message = format!("HTTP_STATUS/{status} provider request failed");
        return ExtractedErrorDetails {
            classification_message: message.clone(),
            message,
        };
    }

    if let Ok(parsed) = serde_json::from_str::<ErrorBody>(trimmed) {
        let mut classification_hints = Vec::new();
        push_classification_hint(&mut classification_hints, parsed.error_type.as_deref());
        push_classification_hint(&mut classification_hints, parsed.code.as_deref());
        let message = match &parsed.error {
            Some(ErrorDetail::Object {
                message,
                error_type,
                code,
            }) => {
                push_classification_hint(&mut classification_hints, error_type.as_deref());
                push_classification_hint(&mut classification_hints, code.as_deref());
                message.as_deref()
            }
            Some(ErrorDetail::String(s)) => Some(s.as_str()),
            None => None,
        }
        .or(parsed.message.as_deref());

        if let Some(msg) = message {
            let message = sanitize_provider_error_message(msg);
            return ExtractedErrorDetails {
                classification_message: classification_message(&message, &classification_hints),
                message,
            };
        }
    }

    let message = sanitize_provider_error_message(trimmed);
    ExtractedErrorDetails {
        classification_message: message.clone(),
        message,
    }
}

fn push_classification_hint(hints: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    let value = sanitize_provider_error_message(value);
    if value.is_empty() || value.eq_ignore_ascii_case("error") {
        return;
    }
    if hints
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&value))
    {
        return;
    }
    hints.push(value);
}

fn classification_message(message: &str, hints: &[String]) -> String {
    let mut parts = Vec::new();
    for hint in hints {
        if !message.eq_ignore_ascii_case(hint) {
            parts.push(hint.as_str());
        }
    }
    parts.push(message);
    parts.join(": ")
}

pub fn sanitize_provider_error_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Strip an SSE `data:` frame prefix only when it is line-anchored (start of
    // the message or start of a line). Using `rsplit("data:")` matched the
    // substring anywhere, truncating a legitimate error like
    // "failed to load data: connection reset" to " connection reset".
    let sse_payload = trimmed.strip_prefix("data:").or_else(|| {
        trimmed
            .rfind("\ndata:")
            .map(|index| &trimmed[index + "\ndata:".len()..])
    });
    let normalized = if let Some(payload) = sse_payload {
        let payload = payload.trim();
        if payload.len() < trimmed.len() && !payload.is_empty() {
            payload
        } else {
            trimmed
        }
    } else if let Some(index) = trimmed.find("HTTP_STATUS/") {
        trimmed[index..].trim()
    } else if let Some(payload) = trimmed.rsplit("event:error").next() {
        let payload = payload.trim_start_matches(':').trim();
        if payload.len() < trimmed.len() && !payload.is_empty() {
            payload
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    sanitize_provider_text(normalized)
}

fn sanitize_provider_text(message: &str) -> String {
    const MAX_PROVIDER_ERROR_CHARS: usize = 1_000;
    let mut clean = String::new();
    let mut pending_space = false;
    for character in message.chars() {
        if character.is_control() || character.is_whitespace() {
            pending_space = !clean.is_empty();
            continue;
        }
        if pending_space {
            clean.push(' ');
            pending_space = false;
        }
        clean.push(character);
    }

    let mut redact_next_bearer = false;
    let sanitized = clean
        .split_whitespace()
        .map(|word| {
            if redact_next_bearer {
                redact_next_bearer = false;
                return "[redacted]".to_string();
            }
            let lower = word.to_ascii_lowercase();
            if lower == "bearer" {
                redact_next_bearer = true;
                return word.to_string();
            }
            sanitize_provider_word(word)
        })
        .collect::<Vec<_>>()
        .join(" ");
    let truncated = sanitized.chars().count() > MAX_PROVIDER_ERROR_CHARS;
    let mut bounded = sanitized
        .chars()
        .take(MAX_PROVIDER_ERROR_CHARS)
        .collect::<String>();
    if truncated {
        bounded.pop();
        bounded.push('…');
    }
    bounded
}

fn sanitize_provider_word(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    let url_start = lower.find("https://").or_else(|| lower.find("http://"));
    if let Some(start) = url_start
        && let Ok(mut url) = url::Url::parse(&word[start..])
    {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        return format!("{}{}", &word[..start], url);
    }
    if [
        "code=",
        "state=",
        "verifier=",
        "challenge=",
        "token=",
        "api_key=",
        "account_id=",
        "email=",
        "plan=",
        "device_auth_id=",
        "callback_query=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        "[redacted]".to_string()
    } else {
        word.to_string()
    }
}
