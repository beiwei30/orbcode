use serde::Deserialize;
use tokio::time::timeout;

use crate::error::McpError;
use crate::store::StoredMcpOAuthToken;
use crate::transport::http::HTTP_REQUEST_TIMEOUT;
use crate::types::{McpOAuthStatusEntry, McpOAuthTokenInput};

use super::ssrf::pinned_oauth_client;
use super::{mask_secret, unix_timestamp_now};

#[derive(Debug, Deserialize)]
pub(super) struct OAuthTokenResponse {
    pub(super) access_token: String,
    #[serde(default)]
    pub(super) refresh_token: Option<String>,
    #[serde(default)]
    pub(super) expires_in: Option<i64>,
    #[serde(default)]
    pub(super) scope: Option<String>,
}

pub(crate) fn mcp_oauth_status_entry(
    server_id: &str,
    token: &StoredMcpOAuthToken,
) -> McpOAuthStatusEntry {
    let expired = is_mcp_oauth_token_expired(token);
    McpOAuthStatusEntry {
        server_id: server_id.to_string(),
        source_summary: format!("stored:{}", mask_secret(&token.access_token)),
        usable: !token.access_token.trim().is_empty() && !expired,
        expired,
        has_refresh_token: token.refresh_token.is_some(),
        has_token_endpoint: token.token_endpoint.is_some(),
        expires_at: token.expires_at,
        scopes: token.scopes.clone(),
        updated_at: token.updated_at,
    }
}

pub(crate) async fn refresh_mcp_oauth_token(
    server_id: &str,
    token: &StoredMcpOAuthToken,
) -> Result<StoredMcpOAuthToken, McpError> {
    let Some(refresh_token) = token.refresh_token.as_deref() else {
        return Err(McpError::AuthRequired {
            server: server_id.to_string(),
            reason: "stored OAuth access token is expired and no refresh token is available"
                .to_string(),
        });
    };
    let Some(token_endpoint) = token.token_endpoint.as_deref() else {
        return Err(McpError::AuthRequired {
            server: server_id.to_string(),
            reason: "stored OAuth access token is expired and no token endpoint is configured"
                .to_string(),
        });
    };

    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
    ];
    if let Some(client_id) = token.client_id.as_deref() {
        form.push(("client_id", client_id.to_string()));
    }

    // A refresh may fire long after discovery, so re-validate and pin the token
    // endpoint's address at request time to defeat DNS rebinding. Enforcement is
    // read from the token's OWN frozen `enforce_ssrf` (captured when the token was
    // obtained), not re-derived from the live server config — so a later
    // public→local endpoint edit cannot downgrade this refresh, and a
    // deliberately-local login whose AS uses a private DNS name is still allowed.
    let client = pinned_oauth_client(token_endpoint, "token endpoint", token.enforce_ssrf).await?;
    let response = timeout(
        HTTP_REQUEST_TIMEOUT,
        client.post(token_endpoint).form(&form).send(),
    )
    .await
    .map_err(|_| McpError::Timeout("oauth token refresh".to_string()))?
    .map_err(|error| McpError::Http(error.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(McpError::AuthRequired {
            server: server_id.to_string(),
            reason: format!(
                "OAuth refresh failed with status {status}{}",
                if detail.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail.trim())
                }
            ),
        });
    }

    let refresh: OAuthTokenResponse = response
        .json()
        .await
        .map_err(|error| McpError::Http(error.to_string()))?;
    let input = oauth_token_input_from_response(
        refresh,
        token.token_endpoint.clone(),
        token.client_id.clone(),
        token.scopes.clone(),
    );

    Ok(StoredMcpOAuthToken {
        access_token: input.access_token,
        refresh_token: input.refresh_token.or_else(|| token.refresh_token.clone()),
        token_endpoint: input.token_endpoint,
        client_id: input.client_id,
        expires_at: input.expires_at,
        scopes: input.scopes,
        updated_at: Some(unix_timestamp_now()),
        // Preserve the frozen enforcement decision across refreshes.
        enforce_ssrf: token.enforce_ssrf,
    })
}

pub(super) fn oauth_token_input_from_response(
    token: OAuthTokenResponse,
    token_endpoint: Option<String>,
    client_id: Option<String>,
    fallback_scopes: Vec<String>,
) -> McpOAuthTokenInput {
    let now = unix_timestamp_now();
    let scopes = token
        .scope
        .as_deref()
        .map(split_oauth_scope)
        .filter(|scopes| !scopes.is_empty())
        .unwrap_or(fallback_scopes);
    McpOAuthTokenInput {
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        token_endpoint,
        client_id,
        expires_at: token.expires_in.map(|seconds| now + seconds.max(0)),
        scopes,
    }
}

/// Refresh a token slightly before its real expiry to absorb clock skew and
/// request latency: a token that is valid for only a few more seconds would
/// otherwise 401 at the server mid-request.
const TOKEN_EXPIRY_SKEW_SECONDS: i64 = 30;

pub(crate) fn is_mcp_oauth_token_expired(token: &StoredMcpOAuthToken) -> bool {
    token
        .expires_at
        .is_some_and(|expires_at| expires_at - TOKEN_EXPIRY_SKEW_SECONDS <= unix_timestamp_now())
}

fn split_oauth_scope(scope: &str) -> Vec<String> {
    scope
        .split_whitespace()
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoredMcpOAuthToken;

    fn token_expiring_at(expires_at: Option<i64>) -> StoredMcpOAuthToken {
        StoredMcpOAuthToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            token_endpoint: None,
            client_id: None,
            expires_at,
            scopes: Vec::new(),
            updated_at: None,
            enforce_ssrf: true,
        }
    }

    #[tokio::test]
    async fn refresh_uses_the_tokens_frozen_enforcement_not_the_live_config() {
        // A token minted while the MCP endpoint was public carries enforce_ssrf=true.
        // Refresh must honor THAT (reject a plaintext-http token endpoint), even if
        // the server's endpoint has since been edited to a local one — proving the
        // decision is read from the token, not re-derived from the live config.
        let token = StoredMcpOAuthToken {
            access_token: "old".to_string(),
            refresh_token: Some("refresh".to_string()),
            token_endpoint: Some("http://as.example.com/token".to_string()),
            client_id: None,
            expires_at: None,
            scopes: Vec::new(),
            updated_at: None,
            enforce_ssrf: true,
        };
        let error = refresh_mcp_oauth_token("server", &token)
            .await
            .expect_err("a public token must reject a plaintext-http endpoint on refresh");
        assert!(
            error.to_string().contains("https"),
            "expected an https-required error, got: {error}"
        );
    }

    #[test]
    fn token_within_skew_window_is_treated_as_expired() {
        let now = unix_timestamp_now();
        // Valid for another 10s, but inside the 30s skew buffer → refresh now.
        assert!(is_mcp_oauth_token_expired(&token_expiring_at(Some(
            now + 10
        ))));
        // Comfortably in the future → still valid.
        assert!(!is_mcp_oauth_token_expired(&token_expiring_at(Some(
            now + 300
        ))));
        // No expiry → never expires.
        assert!(!is_mcp_oauth_token_expired(&token_expiring_at(None)));
    }
}
