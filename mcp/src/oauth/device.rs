use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::time::{sleep, timeout};

use crate::error::McpError;
use crate::transport::http::HTTP_REQUEST_TIMEOUT;
use crate::types::{
    McpOAuthDeviceLoginInput, McpOAuthDeviceLoginSession, McpOAuthTokenInput, McpServerConfig,
};

use super::discovery::discover_oauth_metadata_struct;
use super::ssrf::{mcp_endpoint_is_internal, pinned_oauth_client};
use super::token::oauth_token_input_from_response;
use super::unix_timestamp_now;

#[derive(Debug, Deserialize)]
struct OAuthDeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    #[serde(alias = "verification_url")]
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: i64,
    #[serde(default)]
    interval: Option<u64>,
}

pub(crate) async fn start_mcp_oauth_device_login(
    config: &McpServerConfig,
    input: McpOAuthDeviceLoginInput,
) -> Result<McpOAuthDeviceLoginSession, McpError> {
    let discovery = if input.device_authorization_endpoint.is_none()
        || input.token_endpoint.is_none()
        || input.scopes.is_empty()
    {
        discover_oauth_metadata_struct(config, None).await?
    } else {
        None
    };
    let device_authorization_endpoint = input
        .device_authorization_endpoint
        .or_else(|| {
            discovery
                .as_ref()
                .and_then(|metadata| metadata.device_authorization_endpoint.clone())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let token_endpoint = input
        .token_endpoint
        .or_else(|| {
            discovery
                .as_ref()
                .and_then(|metadata| metadata.token_endpoint.clone())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let client_id = input.client_id.trim().to_string();
    if client_id.is_empty() {
        return Err(McpError::InvalidConfig(
            "device login requires a client id".into(),
        ));
    }
    let Some(device_authorization_endpoint) = device_authorization_endpoint else {
        return Err(McpError::InvalidConfig(
            "device login requires --device-authorization-endpoint or OAuth discovery metadata"
                .into(),
        ));
    };
    let Some(token_endpoint) = token_endpoint else {
        return Err(McpError::InvalidConfig(
            "device login requires --token-endpoint or OAuth discovery metadata".into(),
        ));
    };
    let scopes = input
        .scopes
        .into_iter()
        .map(|scope| scope.trim().to_string())
        .filter(|scope| !scope.is_empty())
        .collect::<Vec<_>>();
    let scopes = if scopes.is_empty() {
        discovery
            .as_ref()
            .map(|metadata| metadata.scopes.clone())
            .unwrap_or_default()
    } else {
        scopes
    };

    let mut form = vec![("client_id", client_id.clone())];
    if !scopes.is_empty() {
        form.push(("scope", scopes.join(" ")));
    }

    // Decide SSRF/TLS enforcement ONCE from the MCP endpoint the user connected
    // to (public → enforce). This decision is stored on the session so the later
    // token poll uses the same context instead of re-guessing from the token URL
    // literal — otherwise a deliberately-local MCP whose AS is a private DNS name
    // would be allowed at start but rejected at poll.
    let enforce = !mcp_endpoint_is_internal(&config.endpoint);
    let client = pinned_oauth_client(
        &device_authorization_endpoint,
        "device authorization endpoint",
        enforce,
    )
    .await?;
    let response = timeout(
        HTTP_REQUEST_TIMEOUT,
        client
            .post(&device_authorization_endpoint)
            .form(&form)
            .send(),
    )
    .await
    .map_err(|_| McpError::Timeout("oauth device authorization".to_string()))?
    .map_err(McpError::http)?
    .error_for_status()
    .map_err(McpError::http)?;

    let authorization: OAuthDeviceAuthorizationResponse =
        response.json().await.map_err(McpError::http)?;
    Ok(McpOAuthDeviceLoginSession {
        server_id: config.id.clone(),
        device_code: authorization.device_code,
        user_code: authorization.user_code,
        verification_uri: authorization.verification_uri,
        verification_uri_complete: authorization.verification_uri_complete,
        expires_at: unix_timestamp_now() + authorization.expires_in.max(0),
        interval_secs: authorization.interval.unwrap_or(5).max(1),
        token_endpoint,
        client_id,
        scopes,
        enforce_ssrf: enforce,
    })
}

pub(crate) async fn poll_mcp_oauth_device_token(
    session: &McpOAuthDeviceLoginSession,
) -> Result<McpOAuthTokenInput, McpError> {
    // Pin the token endpoint's address once, up front: the poll loop can run for
    // minutes, so re-resolving each iteration would reopen the rebinding window.
    // Reuse the enforcement decision made at login start (from the MCP endpoint)
    // rather than re-deriving from the URL literal.
    let client = pinned_oauth_client(
        &session.token_endpoint,
        "token endpoint",
        session.enforce_ssrf,
    )
    .await?;
    let mut interval_secs = session.interval_secs.max(1);
    loop {
        if unix_timestamp_now() >= session.expires_at {
            return Err(McpError::AuthRequired {
                server: session.server_id.clone(),
                reason: "OAuth device login expired before authorization completed".to_string(),
            });
        }
        let form = [
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:device_code".to_string(),
            ),
            ("device_code", session.device_code.clone()),
            ("client_id", session.client_id.clone()),
        ];
        let response = timeout(
            HTTP_REQUEST_TIMEOUT,
            client.post(&session.token_endpoint).form(&form).send(),
        )
        .await
        .map_err(|_| McpError::Timeout("oauth device token".to_string()))?
        .map_err(McpError::http)?;

        let status = response.status();
        let body: Value = response.json().await.map_err(McpError::http)?;
        if status.is_success() {
            let token: super::token::OAuthTokenResponse = serde_json::from_value(body)?;
            return Ok(oauth_token_input_from_response(
                token,
                Some(session.token_endpoint.clone()),
                Some(session.client_id.clone()),
                session.scopes.clone(),
            ));
        }

        let error = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown_error");
        match error {
            "authorization_pending" => {
                sleep(Duration::from_secs(interval_secs)).await;
            }
            "slow_down" => {
                interval_secs += 5;
                sleep(Duration::from_secs(interval_secs)).await;
            }
            "access_denied" => {
                return Err(McpError::AuthRequired {
                    server: session.server_id.clone(),
                    reason: "OAuth device login was denied".to_string(),
                });
            }
            "expired_token" => {
                return Err(McpError::AuthRequired {
                    server: session.server_id.clone(),
                    reason: "OAuth device login expired before authorization completed".to_string(),
                });
            }
            _ => {
                return Err(McpError::AuthRequired {
                    server: session.server_id.clone(),
                    reason: format!("OAuth device token request failed: {error}"),
                });
            }
        }
    }
}
