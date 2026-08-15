use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
use serde::Deserialize;
use serde_json::json;
use tokio::time::timeout;

use crate::error::McpError;
use crate::transport::effective_http_headers;
use crate::transport::http::HTTP_REQUEST_TIMEOUT;
use crate::types::{McpDiagnosticCheck, McpDiagnosticStatus, McpServerConfig};

use super::mcp_check;
use super::ssrf::{mcp_endpoint_is_internal, pinned_oauth_client};

#[derive(Debug, Deserialize)]
struct OAuthProtectedResourceMetadata {
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthAuthorizationServerMetadata {
    #[serde(default)]
    issuer: Option<String>,
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    device_authorization_endpoint: Option<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OAuthDiscoveryMetadata {
    pub(super) resource_metadata_url: String,
    pub(super) resource: Option<String>,
    pub(super) authorization_server: String,
    pub(super) issuer: Option<String>,
    pub(super) authorization_endpoint: Option<String>,
    pub(super) token_endpoint: Option<String>,
    pub(super) device_authorization_endpoint: Option<String>,
    pub(super) registration_endpoint: Option<String>,
    pub(super) scopes: Vec<String>,
}

pub(crate) async fn diagnose_oauth_discovery(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> McpDiagnosticCheck {
    match discover_oauth_metadata_struct(config, oauth_access_token).await {
        Ok(Some(metadata)) => mcp_check("oauth", McpDiagnosticStatus::Pass, metadata.detail()),
        Ok(None) => mcp_check(
            "oauth",
            McpDiagnosticStatus::Warn,
            "server required auth but did not advertise OAuth resource metadata",
        ),
        Err(error) => mcp_check("oauth", McpDiagnosticStatus::Fail, error.to_string()),
    }
}

pub(super) async fn discover_oauth_metadata_struct(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> Result<Option<OAuthDiscoveryMetadata>, McpError> {
    let headers = effective_http_headers(config, oauth_access_token)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(McpError::http)?;
    let response = timeout(
        HTTP_REQUEST_TIMEOUT,
        client
            .post(&config.endpoint)
            .headers(headers)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "orbcode",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                },
            }))
            .send(),
    )
    .await
    .map_err(|_| McpError::Timeout("oauth discovery initialize".to_string()))?
    .map_err(McpError::http)?;

    if response.status() != reqwest::StatusCode::UNAUTHORIZED
        && response.status() != reqwest::StatusCode::FORBIDDEN
    {
        return Ok(None);
    }

    let Some(challenge) = response
        .headers()
        .get(WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(None);
    };
    let Some(resource_metadata_url) = www_authenticate_param(challenge, "resource_metadata") else {
        return Ok(None);
    };

    // The `resource_metadata` URL is attacker-influenced (it comes straight from
    // the server's WWW-Authenticate header). Require it to be same-origin with
    // the MCP endpoint so a malicious server cannot redirect the OAuth exchange
    // to a host it does not control (RFC 9728 §3.1).
    ensure_same_origin(
        &config.endpoint,
        &resource_metadata_url,
        "resource metadata URL",
    )?;

    // When the MCP endpoint is public, enforce the SSRF guard on every
    // discovery-doc-derived URL; a deliberately-local MCP endpoint expects a
    // local authorization server. Each server-side fetch below uses a client
    // *pinned* to the addresses validated here, so a rebinding attack cannot
    // swap in an internal IP between the check and the request.
    let enforce_ssrf = !mcp_endpoint_is_internal(&config.endpoint);

    // The `resource_metadata` URL is same-origin with the (user-chosen) MCP
    // endpoint, but still pin its resolved address to foreclose rebinding.
    let resource_client = pinned_oauth_client(
        &resource_metadata_url,
        "resource metadata URL",
        enforce_ssrf,
    )
    .await?;
    let protected: OAuthProtectedResourceMetadata = timeout(
        HTTP_REQUEST_TIMEOUT,
        resource_client.get(&resource_metadata_url).send(),
    )
    .await
    .map_err(|_| McpError::Timeout("oauth protected resource metadata".to_string()))?
    .map_err(McpError::http)?
    .error_for_status()
    .map_err(McpError::http)?
    .json()
    .await
    .map_err(McpError::http)?;

    let Some(authorization_server) = protected.authorization_servers.first() else {
        return Err(McpError::Protocol(format!(
            "OAuth protected resource metadata `{resource_metadata_url}` did not list authorization_servers"
        )));
    };
    // The authorization server comes from the (attacker-influenceable) resource
    // metadata. Resolve + validate its metadata URL and pin the resulting client
    // so the fetch targets exactly the vetted address (SSRF, incl. cloud metadata).
    let auth_metadata_url = authorization_server_metadata_url(authorization_server)?;
    let auth_client = pinned_oauth_client(
        &auth_metadata_url,
        "authorization server metadata URL",
        enforce_ssrf,
    )
    .await?;
    let auth: OAuthAuthorizationServerMetadata = timeout(
        HTTP_REQUEST_TIMEOUT,
        auth_client.get(&auth_metadata_url).send(),
    )
    .await
    .map_err(|_| McpError::Timeout("oauth authorization server metadata".to_string()))?
    .map_err(McpError::http)?
    .error_for_status()
    .map_err(McpError::http)?
    .json()
    .await
    .map_err(McpError::http)?;

    // RFC 8414 §3.3: authenticate the metadata document by requiring its
    // `issuer` to equal the authorization server identifier we fetched it from.
    // Once authenticated, its endpoints are legitimate even if cross-domain (a
    // same-origin requirement would wrongly reject valid multi-domain
    // deployments). Each endpoint is additionally SSRF-guarded here as an early
    // rejection; the token / device / registration endpoints are re-validated
    // and pinned at their actual server-side request sites, so a public MCP
    // server cannot pivot the exchange to an internal host.
    ensure_issuer_matches(authorization_server, auth.issuer.as_deref())?;
    for (endpoint, what) in [
        (
            auth.authorization_endpoint.as_deref(),
            "authorization endpoint",
        ),
        (auth.token_endpoint.as_deref(), "token endpoint"),
        (
            auth.device_authorization_endpoint.as_deref(),
            "device authorization endpoint",
        ),
        (
            auth.registration_endpoint.as_deref(),
            "registration endpoint",
        ),
    ] {
        if let Some(endpoint) = endpoint {
            // Resolve + validate now (early rejection); the returned pinned
            // client is discarded because the actual request happens elsewhere.
            pinned_oauth_client(endpoint, what, enforce_ssrf).await?;
        }
    }

    let scopes = if protected.scopes_supported.is_empty() {
        auth.scopes_supported
    } else {
        protected.scopes_supported
    };
    Ok(Some(OAuthDiscoveryMetadata {
        resource_metadata_url,
        resource: protected.resource,
        authorization_server: authorization_server.clone(),
        issuer: auth.issuer,
        authorization_endpoint: auth.authorization_endpoint,
        token_endpoint: auth.token_endpoint,
        device_authorization_endpoint: auth.device_authorization_endpoint,
        registration_endpoint: auth.registration_endpoint,
        scopes,
    }))
}

impl OAuthDiscoveryMetadata {
    fn detail(&self) -> String {
        let mut parts = vec![format!("resource_metadata={}", self.resource_metadata_url)];
        if let Some(resource) = self.resource.as_deref() {
            parts.push(format!("resource={resource}"));
        }
        parts.push(format!(
            "authorization_server={}",
            self.authorization_server
        ));
        if let Some(issuer) = self.issuer.as_deref() {
            parts.push(format!("issuer={issuer}"));
        }
        if let Some(endpoint) = self.authorization_endpoint.as_deref() {
            parts.push(format!("authorization_endpoint={endpoint}"));
        }
        if let Some(endpoint) = self.token_endpoint.as_deref() {
            parts.push(format!("token_endpoint={endpoint}"));
        }
        if let Some(endpoint) = self.device_authorization_endpoint.as_deref() {
            parts.push(format!("device_authorization_endpoint={endpoint}"));
        }
        if let Some(endpoint) = self.registration_endpoint.as_deref() {
            parts.push(format!("registration_endpoint={endpoint}"));
        }
        if !self.scopes.is_empty() {
            parts.push(format!("scopes={}", self.scopes.join(",")));
        }
        parts.join(" ")
    }
}

fn www_authenticate_param(challenge: &str, name: &str) -> Option<String> {
    let mut rest = challenge;
    loop {
        let offset = rest.find(name)?;
        let after_name = &rest[offset + name.len()..];
        let after_equals = after_name.trim_start();
        let after_equals = after_equals.strip_prefix('=')?.trim_start();
        if let Some(stripped) = after_equals.strip_prefix('"') {
            let mut value = String::new();
            let mut escaped = false;
            for ch in stripped.chars() {
                if escaped {
                    value.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    return Some(value);
                } else {
                    value.push(ch);
                }
            }
            return None;
        }
        let value = after_equals
            .split([',', ' '])
            .next()
            .unwrap_or_default()
            .trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
        rest = &after_name[1..];
    }
}

/// RFC 8414 §3.3: the `issuer` in the authorization-server metadata MUST be
/// identical to the authorization server identifier used to build the metadata
/// URL. This authenticates the metadata document's origin (a cross-domain
/// endpoint set is then legitimate — RFC 8414 does not require same-origin
/// endpoints, so the previous same-origin check both under- and over-constrained).
///
/// The comparison is exact, by Unicode code point: RFC 8414 §3.3 requires the
/// values to be identical, so no case-folding or trailing-slash normalization
/// is permitted. Normalizing would let a spoofed metadata document differing
/// only in case or a trailing `/` pass as the legitimate issuer.
fn ensure_issuer_matches(authorization_server: &str, issuer: Option<&str>) -> Result<(), McpError> {
    let issuer = issuer.ok_or_else(|| {
        McpError::Protocol(
            "OAuth authorization server metadata is missing the required `issuer` (RFC 8414 §3.3)"
                .to_string(),
        )
    })?;
    if issuer != authorization_server {
        return Err(McpError::Protocol(format!(
            "OAuth issuer `{issuer}` does not match the authorization server \
             `{authorization_server}` (RFC 8414 §3.3)"
        )));
    }
    Ok(())
}

fn ensure_same_origin(reference: &str, candidate: &str, what: &str) -> Result<(), McpError> {
    let reference = reqwest::Url::parse(reference).map_err(|error| {
        McpError::Protocol(format!(
            "invalid OAuth reference URL `{reference}`: {error}"
        ))
    })?;
    let parsed = reqwest::Url::parse(candidate).map_err(|error| {
        McpError::Protocol(format!("invalid OAuth {what} `{candidate}`: {error}"))
    })?;
    let same_origin = parsed.scheme() == reference.scheme()
        && parsed.host_str() == reference.host_str()
        && parsed.port_or_known_default() == reference.port_or_known_default();
    if !same_origin {
        return Err(McpError::Protocol(format!(
            "OAuth {what} `{candidate}` is not same-origin with `{}`; \
             refusing to follow a cross-origin OAuth redirect",
            reference.as_str()
        )));
    }
    Ok(())
}

fn authorization_server_metadata_url(authorization_server: &str) -> Result<String, McpError> {
    if authorization_server.contains("/.well-known/oauth-authorization-server") {
        return Ok(authorization_server.to_string());
    }
    let mut url = reqwest::Url::parse(authorization_server).map_err(|error| {
        McpError::InvalidConfig(format!(
            "invalid OAuth authorization server `{authorization_server}`: {error}"
        ))
    })?;
    let original_path = url.path().trim_matches('/');
    let mut path = "/.well-known/oauth-authorization-server".to_string();
    if !original_path.is_empty() {
        path.push('/');
        path.push_str(original_path);
    }
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::{ensure_issuer_matches, ensure_same_origin};

    #[test]
    fn issuer_must_equal_authorization_server() {
        // RFC 8414 §3.3 — issuer identical to the AS identifier (exact,
        // code-point comparison).
        assert!(
            ensure_issuer_matches("https://as.example.com", Some("https://as.example.com")).is_ok()
        );
        // RFC 8414 §3.3 requires an *exact* match: a trailing-slash difference
        // is NOT tolerated (normalization would admit spoofed metadata).
        assert!(
            ensure_issuer_matches("https://as.example.com/", Some("https://as.example.com"))
                .is_err()
        );
        // A case-only difference is likewise rejected — no case-folding.
        assert!(
            ensure_issuer_matches("https://as.example.com", Some("https://AS.EXAMPLE.COM"))
                .is_err()
        );
        // A mismatched issuer (spoofed metadata) is rejected.
        assert!(
            ensure_issuer_matches("https://as.example.com", Some("https://evil.example")).is_err()
        );
        // A missing issuer is rejected (the field is mandatory).
        assert!(ensure_issuer_matches("https://as.example.com", None).is_err());
    }

    #[test]
    fn same_origin_accepts_matching_and_rejects_cross_origin() {
        assert!(
            ensure_same_origin(
                "https://api.example.com/mcp",
                "https://api.example.com/.well-known/oauth-protected-resource",
                "resource metadata URL"
            )
            .is_ok()
        );

        // Different host is rejected (the SSRF/redirect vector).
        assert!(
            ensure_same_origin(
                "https://api.example.com/mcp",
                "https://attacker.example/.well-known/oauth-protected-resource",
                "resource metadata URL"
            )
            .is_err()
        );

        // Scheme downgrade is rejected.
        assert!(
            ensure_same_origin(
                "https://api.example.com/mcp",
                "http://api.example.com/meta",
                "resource metadata URL"
            )
            .is_err()
        );

        // Different port is rejected.
        assert!(
            ensure_same_origin(
                "https://api.example.com/mcp",
                "https://api.example.com:8443/meta",
                "resource metadata URL"
            )
            .is_err()
        );
    }
}
