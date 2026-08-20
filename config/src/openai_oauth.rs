//! ChatGPT/Codex OAuth protocol support.
//!
//! This module deliberately isolates the non-API-key OpenAI contract. ChatGPT
//! access tokens are valid only for the fixed Codex backend and must never be
//! forwarded to an `OPENAI_BASE_URL` override.

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

use crate::{ConfigError, OutboundProxyConfig, OutboundProxyRoute};

pub const OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_OAUTH_ISSUER: &str = "https://auth.openai.com";
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_OAUTH_SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const MAX_OAUTH_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_PROVIDER_ERROR_CHARS: usize = 240;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptOAuthCredentials {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix epoch milliseconds.
    pub expires_at: i64,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

impl ChatGptOAuthCredentials {
    /// Whether the saved credentials contain every value required both to
    /// issue a Codex Responses request and to refresh it later.
    pub fn is_usable(&self) -> bool {
        !self.access_token.trim().is_empty()
            && !self.refresh_token.trim().is_empty()
            && self
                .account_id
                .as_deref()
                .is_some_and(|account_id| !account_id.trim().is_empty())
    }

    pub(crate) fn needs_refresh(&self) -> bool {
        Utc::now().timestamp_millis() + 5 * 60 * 1000 >= self.expires_at
    }
}

#[derive(Clone, Debug)]
pub struct OpenAiOAuthOptions {
    pub issuer: String,
    pub client_id: String,
    pub codex_base_url: String,
    pub originator: String,
    pub callback_ports: Vec<u16>,
    pub browser_timeout: Duration,
    pub device_timeout: Duration,
    pub proxy_config: OutboundProxyConfig,
}

impl Default for OpenAiOAuthOptions {
    fn default() -> Self {
        Self {
            issuer: OPENAI_OAUTH_ISSUER.to_string(),
            client_id: OPENAI_OAUTH_CLIENT_ID.to_string(),
            codex_base_url: CHATGPT_CODEX_BASE_URL.to_string(),
            originator: "orbcode".to_string(),
            callback_ports: vec![1455, 1457],
            browser_timeout: Duration::from_secs(5 * 60),
            device_timeout: Duration::from_secs(15 * 60),
            proxy_config: OutboundProxyConfig::default(),
        }
    }
}

pub struct ChatGptBrowserLoginSession {
    pub authorization_url: String,
    pub redirect_uri: String,
    listener: TcpListener,
    state: String,
    code_verifier: String,
    options: OpenAiOAuthOptions,
}

impl fmt::Debug for ChatGptBrowserLoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatGptBrowserLoginSession")
            .field("authorization_url", &self.authorization_url)
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ChatGptDeviceLoginSession {
    pub verification_uri: String,
    pub user_code: String,
    pub interval_secs: u64,
    device_auth_id: String,
    options: OpenAiOAuthOptions,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(deserialize_with = "deserialize_interval")]
    interval: String,
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    authorization_code: String,
    code_verifier: String,
    #[allow(dead_code)]
    code_challenge: String,
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(value) => Ok(value),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err(serde::de::Error::custom(
            "device interval must be a string or number",
        )),
    }
}

pub(crate) async fn start_browser_login(
    options: OpenAiOAuthOptions,
) -> Result<ChatGptBrowserLoginSession, ConfigError> {
    let (listener, port) = bind_callback_listener(&options.callback_ports).await?;
    let redirect_uri = format!("http://localhost:{port}/auth/callback");
    let state = random_url_safe_value();
    let code_verifier = random_url_safe_value();
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(code_verifier.as_bytes()));
    let mut authorization_url = Url::parse(&format!(
        "{}/oauth/authorize",
        options.issuer.trim_end_matches('/')
    ))
    .map_err(|error| ConfigError::Config(format!("invalid OpenAI OAuth issuer: {error}")))?;
    authorization_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &options.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", OPENAI_OAUTH_SCOPES)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", &state)
        .append_pair("originator", &options.originator);

    Ok(ChatGptBrowserLoginSession {
        authorization_url: authorization_url.to_string(),
        redirect_uri,
        listener,
        state,
        code_verifier,
        options,
    })
}

pub(crate) async fn complete_browser_login(
    session: ChatGptBrowserLoginSession,
) -> Result<ChatGptOAuthCredentials, ConfigError> {
    let callback = tokio::time::timeout(session.options.browser_timeout, async {
        let (mut socket, _) = session.listener.accept().await?;
        let mut buffer = vec![0_u8; 16 * 1024];
        let size = socket.read(&mut buffer).await?;
        let outcome = parse_browser_callback(&buffer[..size], &session.state);
        let (status, body) = if outcome.is_ok() {
            ("200 OK", "ChatGPT sign-in completed. You may close this window.")
        } else {
            ("400 Bad Request", "ChatGPT sign-in failed. Return to the terminal for details.")
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await?;
        outcome
    })
    .await
    .map_err(|_| ConfigError::Config("ChatGPT OAuth callback timed out".to_string()))??;

    exchange_authorization_code(
        &session.options,
        &callback,
        &session.redirect_uri,
        &session.code_verifier,
    )
    .await
}

fn parse_browser_callback(request: &[u8], expected_state: &str) -> Result<String, ConfigError> {
    let request = String::from_utf8_lossy(request);
    let mut parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = parts.next();
    let target = parts.next();
    let version = parts.next();
    if method != Some("GET")
        || !version.is_some_and(|version| version.starts_with("HTTP/"))
        || parts.next().is_some()
    {
        return Err(ConfigError::Config(
            "invalid ChatGPT OAuth callback request".to_string(),
        ));
    }
    let Some(target) = target.filter(|target| target.starts_with('/')) else {
        return Err(ConfigError::Config(
            "invalid ChatGPT OAuth callback request target".to_string(),
        ));
    };
    let callback_url = Url::parse(&format!("http://localhost{target}")).map_err(|_| {
        ConfigError::Config("invalid ChatGPT OAuth callback request target".to_string())
    })?;
    if callback_url.path() != "/auth/callback" || callback_url.fragment().is_some() {
        return Err(ConfigError::Config(
            "invalid ChatGPT OAuth callback request target".to_string(),
        ));
    }
    let query = callback_url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    if query.get("state").map(String::as_str) != Some(expected_state) {
        return Err(ConfigError::Config(
            "ChatGPT OAuth callback state mismatch; login was cancelled".to_string(),
        ));
    }
    if let Some(error) = query.get("error") {
        let reason = query
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error);
        return Err(ConfigError::Config(format!(
            "ChatGPT OAuth callback was rejected: {}",
            sanitize_provider_message(reason)
        )));
    }
    query
        .get("code")
        .filter(|code| !code.trim().is_empty())
        .cloned()
        .ok_or_else(|| ConfigError::Config("ChatGPT OAuth callback omitted the code".to_string()))
}

pub(crate) async fn start_device_login(
    options: OpenAiOAuthOptions,
) -> Result<ChatGptDeviceLoginSession, ConfigError> {
    let base = options.issuer.trim_end_matches('/');
    let endpoint = format!("{base}/api/accounts/deviceauth/usercode");
    let client = oauth_http_client(&options, &endpoint)?;
    let response = client
        .post(endpoint)
        .json(&serde_json::json!({ "client_id": options.client_id }))
        .send()
        .await
        .map_err(|error| oauth_transport_error("ChatGPT device authorization request", error))?;
    let response = checked_oauth_response("ChatGPT device authorization request", response).await?;
    let payload = response
        .json::<DeviceCodeResponse>()
        .await
        .map_err(|error| {
            ConfigError::Config(format!(
                "invalid ChatGPT device authorization response: {error}"
            ))
        })?;
    if payload.device_auth_id.trim().is_empty() || payload.user_code.trim().is_empty() {
        return Err(ConfigError::Config(
            "invalid ChatGPT device authorization response: required fields were empty".to_string(),
        ));
    }
    let interval_secs = payload.interval.trim().parse::<u64>().map_err(|error| {
        ConfigError::Config(format!(
            "invalid ChatGPT device authorization polling interval: {error}"
        ))
    })?;
    if interval_secs == 0 {
        return Err(ConfigError::Config(
            "invalid ChatGPT device authorization polling interval: must be positive".to_string(),
        ));
    }
    Ok(ChatGptDeviceLoginSession {
        verification_uri: format!("{base}/codex/device"),
        user_code: payload.user_code,
        interval_secs,
        device_auth_id: payload.device_auth_id,
        options,
    })
}

pub(crate) async fn complete_device_login(
    session: ChatGptDeviceLoginSession,
) -> Result<ChatGptOAuthCredentials, ConfigError> {
    let base = session.options.issuer.trim_end_matches('/');
    let poll_url = format!("{base}/api/accounts/deviceauth/token");
    let client = oauth_http_client(&session.options, &poll_url)?;
    let authorization = tokio::time::timeout(session.options.device_timeout, async {
        loop {
            let response = client
                .post(&poll_url)
                .json(&serde_json::json!({
                    "device_auth_id": session.device_auth_id,
                    "user_code": session.user_code,
                }))
                .send()
                .await
                .map_err(|error| {
                    oauth_transport_error("ChatGPT device authorization poll", error)
                })?;
            if response.status().is_success() {
                break response
                    .json::<DeviceAuthorizationResponse>()
                    .await
                    .map_err(|error| {
                        ConfigError::Config(format!(
                            "invalid ChatGPT device authorization response: {error}"
                        ))
                    });
            }
            if !matches!(response.status().as_u16(), 403 | 404) {
                return Err(oauth_status_error("ChatGPT device authorization", response).await);
            }
            tokio::time::sleep(Duration::from_secs(session.interval_secs)).await;
        }
    })
    .await
    .map_err(|_| ConfigError::Config("ChatGPT device authorization timed out".to_string()))??;

    if authorization.authorization_code.trim().is_empty()
        || authorization.code_verifier.trim().is_empty()
        || authorization.code_challenge.trim().is_empty()
    {
        return Err(ConfigError::Config(
            "invalid ChatGPT device authorization response: required fields were empty".to_string(),
        ));
    }

    exchange_authorization_code(
        &session.options,
        &authorization.authorization_code,
        &format!("{base}/deviceauth/callback"),
        &authorization.code_verifier,
    )
    .await
}

pub(crate) async fn refresh_credentials(
    options: &OpenAiOAuthOptions,
    current: &ChatGptOAuthCredentials,
) -> Result<ChatGptOAuthCredentials, ConfigError> {
    let endpoint = format!("{}/oauth/token", options.issuer.trim_end_matches('/'));
    let client = oauth_http_client(options, &endpoint)?;
    let response = client
        .post(endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", current.refresh_token.as_str()),
            ("client_id", options.client_id.as_str()),
        ])
        .send()
        .await
        .map_err(|error| oauth_transport_error("refresh ChatGPT token", error))?;
    let response = checked_oauth_response("ChatGPT token refresh", response).await?;
    let tokens = response
        .json::<TokenResponse>()
        .await
        .map_err(|error| ConfigError::Config(format!("invalid token refresh response: {error}")))?;
    credentials_from_response(tokens, Some(current))
        .map_err(|error| ConfigError::Config(format!("ChatGPT token refresh failed: {error}")))
}

async fn exchange_authorization_code(
    options: &OpenAiOAuthOptions,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<ChatGptOAuthCredentials, ConfigError> {
    let endpoint = format!("{}/oauth/token", options.issuer.trim_end_matches('/'));
    let client = oauth_http_client(options, &endpoint)?;
    let response = client
        .post(endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", options.client_id.as_str()),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .map_err(|error| oauth_transport_error("ChatGPT token exchange", error))?;
    let response = checked_oauth_response("ChatGPT token exchange", response).await?;
    let tokens = response.json::<TokenResponse>().await.map_err(|error| {
        ConfigError::Config(format!("invalid token exchange response: {error}"))
    })?;
    credentials_from_response(tokens, None)
        .map_err(|error| ConfigError::Config(format!("ChatGPT token exchange failed: {error}")))
}

fn credentials_from_response(
    tokens: TokenResponse,
    previous: Option<&ChatGptOAuthCredentials>,
) -> Result<ChatGptOAuthCredentials, ConfigError> {
    if tokens.access_token.trim().is_empty() {
        return Err(ConfigError::Config(
            "ChatGPT token response omitted access_token".to_string(),
        ));
    }
    let id_token = tokens
        .id_token
        .filter(|value| !value.trim().is_empty())
        .or_else(|| previous.map(|value| value.id_token.clone()))
        .ok_or_else(|| {
            ConfigError::Config("ChatGPT token response omitted id_token".to_string())
        })?;
    let id_claims = parse_jwt_claims(&id_token);
    let access_claims = parse_jwt_claims(&tokens.access_token);
    let account_id = id_claims
        .as_ref()
        .and_then(extract_account_id)
        .or_else(|| access_claims.as_ref().and_then(extract_account_id))
        .or_else(|| previous.and_then(|value| value.account_id.clone()));
    let email = id_claims
        .as_ref()
        .and_then(|claims| string_claim(claims, "email"))
        .or_else(|| previous.and_then(|value| value.email.clone()));
    let plan_type = id_claims
        .as_ref()
        .and_then(extract_plan_type)
        .or_else(|| access_claims.as_ref().and_then(extract_plan_type))
        .or_else(|| previous.and_then(|value| value.plan_type.clone()));
    let expiry_seconds = tokens
        .expires_in
        .unwrap_or(3600)
        .max(1)
        .saturating_mul(1000);
    Ok(ChatGptOAuthCredentials {
        id_token,
        access_token: tokens.access_token,
        refresh_token: tokens
            .refresh_token
            .filter(|value| !value.trim().is_empty())
            .or_else(|| previous.map(|value| value.refresh_token.clone()))
            .ok_or_else(|| {
                ConfigError::Config("ChatGPT token response omitted refresh_token".to_string())
            })?,
        expires_at: Utc::now().timestamp_millis().saturating_add(expiry_seconds),
        account_id,
        email,
        plan_type,
    })
}

fn parse_jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn extract_account_id(claims: &Value) -> Option<String> {
    string_claim(claims, "chatgpt_account_id")
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|auth| string_claim(auth, "chatgpt_account_id"))
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(Value::as_array)
                .and_then(|organizations| organizations.first())
                .and_then(|organization| string_claim(organization, "id"))
        })
}

fn extract_plan_type(claims: &Value) -> Option<String> {
    string_claim(claims, "chatgpt_plan_type").or_else(|| {
        claims
            .get("https://api.openai.com/auth")
            .and_then(|auth| string_claim(auth, "chatgpt_plan_type"))
    })
}

fn string_claim(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn bind_callback_listener(ports: &[u16]) -> Result<(TcpListener, u16), ConfigError> {
    let mut last_error = None;
    for port in ports.iter().copied() {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                let actual_port = listener.local_addr()?.port();
                return Ok((listener, actual_port));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(ConfigError::Config(format!(
        "unable to bind ChatGPT OAuth callback port: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "no callback ports configured".to_string())
    )))
}

fn random_url_safe_value() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn oauth_http_client(options: &OpenAiOAuthOptions, endpoint: &str) -> Result<Client, ConfigError> {
    let mut builder = Client::builder()
        // OpenAI's auth endpoints reject some anonymous HTTP client traffic.
        // Keep this explicit, matching the Codex/OpenCode login clients rather
        // than relying on transport-specific default headers.
        .user_agent(format!("orbcode/{}", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        // The shared resolver below owns process and system proxy discovery.
        .no_proxy();
    builder = match options.proxy_config.resolve(endpoint) {
        OutboundProxyRoute::Direct => builder.no_proxy(),
        OutboundProxyRoute::Proxy { url, no_proxy } => {
            let mut proxy = reqwest::Proxy::all(&url).map_err(|_| {
                ConfigError::Config("invalid outbound proxy selected for OpenAI OAuth".to_string())
            })?;
            if let Some(no_proxy) = no_proxy.as_deref().and_then(reqwest::NoProxy::from_string) {
                proxy = proxy.no_proxy(Some(no_proxy));
            }
            builder.proxy(proxy)
        }
    };
    builder
        .build()
        .map_err(|error| ConfigError::Config(format!("failed to build OAuth client: {error}")))
}

async fn checked_oauth_response(
    operation: &str,
    response: reqwest::Response,
) -> Result<reqwest::Response, ConfigError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(oauth_status_error(operation, response).await)
    }
}

async fn oauth_status_error(operation: &str, response: reqwest::Response) -> ConfigError {
    let status = response.status();
    let body = bounded_response_body(response).await;
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| {
            string_claim(&value, "error_description")
                .or_else(|| string_claim(&value, "error"))
                .or_else(|| string_claim(&value, "message"))
        })
        .map(|message| sanitize_provider_message(&message))
        .unwrap_or_else(|| "request rejected".to_string());
    let proxy_hint = if status == reqwest::StatusCode::FORBIDDEN {
        "; if browser login works but this request is rejected, check settings.json env.https_proxy/env.http_proxy, process proxy variables, or the macOS system proxy"
    } else {
        ""
    };
    ConfigError::Config(format!(
        "{operation} failed with {status}: {message}{proxy_hint}"
    ))
}

fn oauth_transport_error(operation: &str, error: reqwest::Error) -> ConfigError {
    ConfigError::Config(format!(
        "{operation} failed: {}",
        sanitize_provider_message(&error.to_string())
    ))
}

async fn bounded_response_body(mut response: reqwest::Response) -> String {
    let mut body = Vec::new();
    while body.len() < MAX_OAUTH_ERROR_BODY_BYTES {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) | Err(_) => break,
        };
        let remaining = MAX_OAUTH_ERROR_BODY_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    String::from_utf8_lossy(&body).into_owned()
}

fn sanitize_provider_message(message: &str) -> String {
    let mut clean = String::new();
    let mut chars = message.chars().peekable();
    let mut pending_space = false;
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.next_if_eq(&'[').is_some() {
                for sequence in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&sequence) {
                        break;
                    }
                }
            }
            continue;
        }
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

    let sanitized = clean
        .split_whitespace()
        .map(sanitize_provider_word)
        .collect::<Vec<_>>()
        .join(" ");
    let mut bounded = sanitized
        .chars()
        .take(MAX_PROVIDER_ERROR_CHARS + 1)
        .collect::<String>();
    if bounded.chars().count() > MAX_PROVIDER_ERROR_CHARS {
        bounded.pop();
        bounded.push('…');
    }
    if bounded.is_empty() {
        "request rejected".to_string()
    } else {
        bounded
    }
}

fn sanitize_provider_word(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    let url_start = lower.find("https://").or_else(|| lower.find("http://"));
    if let Some(start) = url_start
        && let Ok(mut url) = Url::parse(&word[start..])
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
        "token=",
        "access_token=",
        "refresh_token=",
        "id_token=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        "[redacted]".to_string()
    } else {
        word.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn jwt(payload: Value) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).expect("payload"));
        format!("header.{payload}.signature")
    }

    #[test]
    fn extracts_nested_account_and_plan_claims() {
        let claims = parse_jwt_claims(&jwt(serde_json::json!({
            "email": "dev@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-123",
                "chatgpt_plan_type": "plus"
            }
        })))
        .expect("claims");
        assert_eq!(extract_account_id(&claims).as_deref(), Some("account-123"));
        assert_eq!(extract_plan_type(&claims).as_deref(), Some("plus"));
        assert_eq!(
            string_claim(&claims, "email").as_deref(),
            Some("dev@example.com")
        );
    }

    #[test]
    fn malformed_jwt_is_ignored() {
        assert_eq!(parse_jwt_claims("not-a-jwt"), None);
        assert_eq!(parse_jwt_claims("a.invalid.b"), None);
    }

    #[test]
    fn oauth_client_applies_shared_proxy_config_without_echoing_credentials() {
        let settings = std::collections::BTreeMap::from([(
            "https_proxy".to_string(),
            "http://proxy-user:proxy-secret@[invalid".to_string(),
        )]);
        let options = OpenAiOAuthOptions {
            proxy_config: OutboundProxyConfig::from_sources(&settings, |_| None),
            ..OpenAiOAuthOptions::default()
        };
        let error = oauth_http_client(&options, "https://auth.openai.com/oauth/token")
            .expect_err("proxy URL is invalid");
        let message = error.to_string();
        assert!(message.contains("invalid outbound proxy"));
        assert!(!message.contains("proxy-secret"));
    }

    #[tokio::test]
    async fn browser_login_uses_pkce_state_and_configured_originator() {
        let options = OpenAiOAuthOptions {
            issuer: "http://127.0.0.1:9".to_string(),
            callback_ports: vec![0],
            ..OpenAiOAuthOptions::default()
        };
        let session = start_browser_login(options).await.expect("start");
        let url = Url::parse(&session.authorization_url).expect("url");
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            query.get("client_id").map(|v| v.as_ref()),
            Some(OPENAI_OAUTH_CLIENT_ID)
        );
        assert_eq!(
            query.get("code_challenge_method").map(|v| v.as_ref()),
            Some("S256")
        );
        assert_eq!(query.get("originator").map(|v| v.as_ref()), Some("orbcode"));
        assert!(query.get("state").is_some_and(|value| !value.is_empty()));
        assert!(session.redirect_uri.contains("/auth/callback"));
    }

    #[tokio::test]
    async fn browser_login_exchanges_callback_and_extracts_metadata() {
        let server = MockServer::start().await;
        let id_token = jwt(serde_json::json!({
            "email": "dev@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-123",
                "chatgpt_plan_type": "plus"
            }
        }));
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(header(
                "user-agent",
                format!("orbcode/{}", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id_token": id_token,
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600
            })))
            .expect(1)
            .mount(&server)
            .await;
        let session = start_browser_login(OpenAiOAuthOptions {
            issuer: server.uri(),
            callback_ports: vec![0],
            browser_timeout: Duration::from_secs(2),
            ..OpenAiOAuthOptions::default()
        })
        .await
        .expect("start");
        let authorization_url = Url::parse(&session.authorization_url).expect("authorization");
        let state = authorization_url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("state");
        let callback_url = format!("{}?code=code-123&state={state}", session.redirect_uri);
        let completion = tokio::spawn(complete_browser_login(session));
        let callback = reqwest::get(callback_url).await.expect("callback");
        assert!(callback.status().is_success());
        let credentials = completion.await.expect("join").expect("complete");
        assert_eq!(credentials.account_id.as_deref(), Some("account-123"));
        assert_eq!(credentials.email.as_deref(), Some("dev@example.com"));
        assert_eq!(credentials.plan_type.as_deref(), Some("plus"));
    }

    #[tokio::test]
    async fn device_login_completes_custom_device_protocol() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/accounts/deviceauth/usercode"))
            .and(header(
                "user-agent",
                format!("orbcode/{}", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_auth_id": "device-1",
                "user_code": "ABCD-EFGH",
                "interval": "1"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/accounts/deviceauth/token"))
            .and(header(
                "user-agent",
                format!("orbcode/{}", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "authorization_code": "code-123",
                "code_verifier": "verifier",
                "code_challenge": "challenge"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(header(
                "user-agent",
                format!("orbcode/{}", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id_token": jwt(serde_json::json!({"chatgpt_account_id":"account-123"})),
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;
        let options = OpenAiOAuthOptions {
            issuer: server.uri(),
            device_timeout: Duration::from_secs(2),
            ..OpenAiOAuthOptions::default()
        };
        let session = start_device_login(options).await.expect("device code");
        assert_eq!(session.user_code, "ABCD-EFGH");
        let credentials = complete_device_login(session).await.expect("complete");
        assert_eq!(credentials.account_id.as_deref(), Some("account-123"));
    }
}
