pub(crate) mod http;
pub mod stdio;
pub(crate) mod streamable_http;
pub(crate) mod websocket;

use std::collections::BTreeMap;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use tokio::time::timeout;

use crate::error::McpError;
use crate::transport::http::{HTTP_REQUEST_TIMEOUT, HttpMcpClient};
use crate::transport::stdio::{STDIO_REQUEST_TIMEOUT, STDIO_STARTUP_TIMEOUT, StdioMcpClient};
use crate::transport::websocket::{WEBSOCKET_REQUEST_TIMEOUT, WebSocketMcpClient};
use crate::types::{McpAuth, McpServerConfig};

pub(crate) async fn spawn_stdio_client(
    config: &McpServerConfig,
) -> Result<StdioMcpClient, McpError> {
    let env = effective_stdio_env(config)?;
    let mut client = StdioMcpClient::spawn_configured(
        &config.endpoint,
        &config.args,
        &env,
        config.cwd.as_deref(),
        STDIO_REQUEST_TIMEOUT,
    )
    .await?;
    match timeout(STDIO_STARTUP_TIMEOUT, client.initialize()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            let error = match error {
                McpError::Protocol(message) => {
                    McpError::Protocol(client.message_with_stderr(message).await)
                }
                McpError::Timeout(method) => McpError::Timeout(
                    client
                        .message_with_stderr(format!("startup {method}"))
                        .await,
                ),
                other => other,
            };
            let _ = client.shutdown().await;
            return Err(error);
        }
        Err(_) => {
            let message = client
                .message_with_stderr("stdio server startup timed out during initialize".to_string())
                .await;
            let _ = client.shutdown().await;
            return Err(McpError::Timeout(message));
        }
    }
    Ok(client)
}

pub(crate) fn http_client(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> Result<HttpMcpClient, McpError> {
    streamable_http_client(config, oauth_access_token)
}

pub(crate) fn streamable_http_client(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> Result<HttpMcpClient, McpError> {
    let headers = effective_http_headers(config, oauth_access_token)?;
    HttpMcpClient::new(&config.endpoint, headers, HTTP_REQUEST_TIMEOUT)
}

pub(crate) async fn websocket_client(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> Result<WebSocketMcpClient, McpError> {
    let headers = effective_http_headers(config, oauth_access_token)?;
    WebSocketMcpClient::connect(&config.endpoint, headers, WEBSOCKET_REQUEST_TIMEOUT).await
}

pub(crate) fn should_restart_stdio(error: &McpError) -> bool {
    matches!(
        error,
        McpError::Io(_) | McpError::Protocol(_) | McpError::Timeout(_)
    )
}

pub(crate) fn effective_stdio_env(
    config: &McpServerConfig,
) -> Result<BTreeMap<String, String>, McpError> {
    let mut env = config.env.clone();
    match &config.auth {
        McpAuth::None => {}
        McpAuth::BearerEnv { env_var } => {
            // Resolve precedence: explicit config.env wins; otherwise read process env so the
            // child sees the token without the user having to duplicate it in `env`.
            if !env.contains_key(env_var) {
                match std::env::var(env_var) {
                    Ok(value) => {
                        env.insert(env_var.clone(), value);
                    }
                    Err(_) => {
                        return Err(McpError::AuthRequired {
                            server: config.id.clone(),
                            reason: format!(
                                "environment variable `{env_var}` for bearer auth is not set"
                            ),
                        });
                    }
                }
            }
            // Expose a canonical name so stdio servers that expect MCP_BEARER_TOKEN can find it.
            if !env.contains_key("MCP_BEARER_TOKEN")
                && let Some(value) = env.get(env_var).cloned()
            {
                env.insert("MCP_BEARER_TOKEN".to_string(), value);
            }
        }
        McpAuth::Header { name, value } => {
            // Stdio transports cannot send HTTP headers, but configured header auth still
            // signals intent. Surface it as an MCP_HEADER_<NAME> env var so launched
            // adapters can forward it on their own transport.
            let key = format!("MCP_HEADER_{}", normalize_header_env_name(name));
            env.entry(key).or_insert_with(|| value.clone());
        }
    }
    Ok(env)
}

pub(crate) fn effective_http_headers(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> Result<HeaderMap, McpError> {
    let mut headers = HeaderMap::new();
    for (name, value) in &config.headers {
        insert_http_header(&mut headers, name, value)?;
    }

    match &config.auth {
        McpAuth::None => {
            if let Some(access_token) = oauth_access_token
                && !headers.contains_key(AUTHORIZATION)
            {
                let value =
                    HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|error| {
                        McpError::InvalidConfig(format!(
                            "invalid Authorization header value: {error}"
                        ))
                    })?;
                headers.insert(AUTHORIZATION, value);
            }
        }
        McpAuth::BearerEnv { env_var } => {
            let value = std::env::var(env_var).map_err(|_| McpError::AuthRequired {
                server: config.id.clone(),
                reason: format!("environment variable `{env_var}` for bearer auth is not set"),
            })?;
            let value = HeaderValue::from_str(&format!("Bearer {value}")).map_err(|error| {
                McpError::InvalidConfig(format!("invalid Authorization header value: {error}"))
            })?;
            headers.insert(AUTHORIZATION, value);
        }
        McpAuth::Header { name, value } => {
            insert_http_header(&mut headers, name, value)?;
        }
    }

    Ok(headers)
}

fn insert_http_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), McpError> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
        McpError::InvalidConfig(format!("invalid HTTP header `{name}`: {error}"))
    })?;
    let value = HeaderValue::from_str(value).map_err(|error| {
        McpError::InvalidConfig(format!("invalid value for HTTP header `{name}`: {error}"))
    })?;
    headers.insert(name, value);
    Ok(())
}

fn normalize_header_env_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}
