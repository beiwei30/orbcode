use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use reqwest::header::ACCEPT;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::McpError;
use crate::transport::http::HTTP_REQUEST_TIMEOUT;
use crate::types::{
    McpOAuthBrowserLoginInput, McpOAuthBrowserLoginSession, McpOAuthTokenInput, McpServerConfig,
};

use super::discovery::discover_oauth_metadata_struct;
use super::ssrf::{ensure_oauth_url_scheme_secure, mcp_endpoint_is_internal, pinned_oauth_client};
use super::token::{OAuthTokenResponse, oauth_token_input_from_response};
use super::{OAuthClientRegistrationResponse, unix_timestamp_now};

pub(crate) async fn start_mcp_oauth_browser_login(
    config: &McpServerConfig,
    input: McpOAuthBrowserLoginInput,
) -> Result<McpOAuthBrowserLoginSession, McpError> {
    let client_id_input = input
        .client_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let registration_endpoint_input = input
        .registration_endpoint
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    // Dynamic client registration is needed when no client id was supplied.
    let needs_registration = client_id_input.is_none();
    let discovery = if input.authorization_endpoint.is_none()
        || input.token_endpoint.is_none()
        || input.scopes.is_empty()
        || (needs_registration && registration_endpoint_input.is_none())
    {
        discover_oauth_metadata_struct(config, None).await?
    } else {
        None
    };
    let authorization_endpoint = input
        .authorization_endpoint
        .or_else(|| {
            discovery
                .as_ref()
                .and_then(|metadata| metadata.authorization_endpoint.clone())
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
    let registration_endpoint = registration_endpoint_input
        .or_else(|| {
            discovery
                .as_ref()
                .and_then(|metadata| metadata.registration_endpoint.clone())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(authorization_endpoint) = authorization_endpoint else {
        return Err(McpError::InvalidConfig(
            "browser login requires --authorization-endpoint or OAuth discovery metadata".into(),
        ));
    };
    let Some(token_endpoint) = token_endpoint else {
        return Err(McpError::InvalidConfig(
            "browser login requires --token-endpoint or OAuth discovery metadata".into(),
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

    // Decide SSRF/TLS enforcement ONCE from the MCP endpoint the user connected
    // to (public → enforce), and carry it on the session so the later token
    // exchange uses the same context rather than re-guessing from a URL literal.
    let enforce = !mcp_endpoint_is_internal(&config.endpoint);
    // The authorization endpoint is opened in the user's browser (not fetched via
    // `pinned_oauth_client`), so gate its scheme here: a public flow must be
    // https, or the redirect carrying the code would traverse plaintext.
    ensure_oauth_url_scheme_secure(&authorization_endpoint, "authorization endpoint", enforce)?;

    // Bind the loopback callback listener before registration so the redirect
    // URI registered with the authorization server matches the one we listen on.
    let listener =
        tokio::net::TcpListener::bind(("127.0.0.1", input.redirect_port.unwrap_or(0))).await?;
    let redirect_uri = format!(
        "http://{}/callback",
        listener.local_addr().map_err(McpError::Io)?
    );

    let (client_id, client_secret) = match client_id_input {
        Some(client_id) => (client_id, None),
        None => {
            let Some(registration_endpoint) = registration_endpoint else {
                return Err(McpError::InvalidConfig(
                    "browser login requires a client id or an OAuth dynamic client registration \
                     endpoint (RFC 7591)"
                        .into(),
                ));
            };
            // Enforce the SSRF/TLS guard on the (attacker-influenceable)
            // registration endpoint whenever the MCP endpoint is public.
            let registration =
                register_oauth_client(&registration_endpoint, &redirect_uri, &scopes, enforce)
                    .await?;
            (registration.client_id, registration.client_secret)
        }
    };

    let state = oauth_state();
    let (code_verifier, code_challenge) = generate_pkce();
    let mut url = reqwest::Url::parse(&authorization_endpoint).map_err(|error| {
        McpError::InvalidConfig(format!(
            "invalid OAuth authorization endpoint `{authorization_endpoint}`: {error}"
        ))
    })?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", &client_id);
        query.append_pair("redirect_uri", &redirect_uri);
        query.append_pair("state", &state);
        // PKCE (RFC 7636): bind this authorization request to a secret verifier
        // so an intercepted code cannot be redeemed by another party.
        query.append_pair("code_challenge", &code_challenge);
        query.append_pair("code_challenge_method", "S256");
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
    }

    Ok(McpOAuthBrowserLoginSession {
        server_id: config.id.clone(),
        authorization_url: url.to_string(),
        redirect_uri,
        expires_at: unix_timestamp_now() + 600,
        token_endpoint,
        client_id,
        client_secret,
        scopes,
        state,
        code_verifier,
        listener,
        enforce_ssrf: enforce,
    })
}

pub(crate) async fn complete_mcp_oauth_browser_login(
    session: McpOAuthBrowserLoginSession,
) -> Result<(String, McpOAuthTokenInput), McpError> {
    let code = wait_for_oauth_browser_code(&session).await?;
    let token = exchange_oauth_authorization_code(
        &session.token_endpoint,
        &session.client_id,
        session.client_secret.as_deref(),
        &session.redirect_uri,
        &code,
        &session.code_verifier,
        session.enforce_ssrf,
    )
    .await?;
    let input = oauth_token_input_from_response(
        token,
        Some(session.token_endpoint.clone()),
        Some(session.client_id.clone()),
        session.scopes.clone(),
    );
    Ok((session.server_id, input))
}

async fn wait_for_oauth_browser_code(
    session: &McpOAuthBrowserLoginSession,
) -> Result<String, McpError> {
    let remaining = session.expires_at.saturating_sub(unix_timestamp_now());
    let wait = Duration::from_secs(remaining.max(1) as u64);
    let (mut stream, _) = timeout(wait, session.listener.accept())
        .await
        .map_err(|_| McpError::Timeout("oauth browser callback".to_string()))??;
    let request = read_http_callback_request(&mut stream).await?;
    let result = oauth_callback_code(&request, &session.state);
    let (status, body) = match &result {
        Ok(_) => (
            "200 OK",
            "OAuth login complete. You can return to Orb Code.".to_string(),
        ),
        Err(error) => ("400 Bad Request", error.to_string()),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    result
}

async fn read_http_callback_request(stream: &mut TcpStream) -> Result<String, McpError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if bytes.len() > 16 * 1024 {
            return Err(McpError::Protocol(
                "OAuth browser callback request is too large".to_string(),
            ));
        }
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn oauth_callback_code(request: &str, expected_state: &str) -> Result<String, McpError> {
    let request_line = request.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" {
        return Err(McpError::Protocol(
            "OAuth browser callback must use GET".to_string(),
        ));
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|error| {
        McpError::Protocol(format!("invalid OAuth browser callback URL: {error}"))
    })?;
    if let Some(error) = url
        .query_pairs()
        .find(|(name, _)| name == "error")
        .map(|(_, value)| value.to_string())
    {
        return Err(McpError::AuthRequired {
            server: "browser-login".to_string(),
            reason: format!("OAuth browser login failed: {error}"),
        });
    }
    let state = url
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.to_string())
        .unwrap_or_default();
    if state != expected_state {
        return Err(McpError::Protocol(
            "OAuth browser callback state did not match".to_string(),
        ));
    }
    url.query_pairs()
        .find(|(name, _)| name == "code")
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            McpError::Protocol("OAuth browser callback did not include code".to_string())
        })
}

pub(crate) async fn register_oauth_client(
    registration_endpoint: &str,
    redirect_uri: &str,
    scopes: &[String],
    enforce_ssrf: bool,
) -> Result<OAuthClientRegistrationResponse, McpError> {
    let mut body = json!({
        "client_name": "orbcode",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if !scopes.is_empty() {
        body["scope"] = Value::String(scopes.join(" "));
    }
    let client =
        pinned_oauth_client(registration_endpoint, "registration endpoint", enforce_ssrf).await?;
    let response = timeout(
        HTTP_REQUEST_TIMEOUT,
        client
            .post(registration_endpoint)
            .header(ACCEPT, "application/json")
            .json(&body)
            .send(),
    )
    .await
    .map_err(|_| McpError::Timeout("oauth dynamic client registration".to_string()))?
    .map_err(|error| McpError::Http(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        let detail = detail.trim();
        return Err(McpError::Protocol(format!(
            "OAuth dynamic client registration failed with status {status}{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }
    let registration: OAuthClientRegistrationResponse = response
        .json()
        .await
        .map_err(|error| McpError::Http(error.to_string()))?;
    let client_id = registration.client_id.trim().to_string();
    if client_id.is_empty() {
        return Err(McpError::Protocol(
            "OAuth dynamic client registration response did not include a client_id".to_string(),
        ));
    }
    Ok(OAuthClientRegistrationResponse {
        client_id,
        client_secret: registration
            .client_secret
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

async fn exchange_oauth_authorization_code(
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
    enforce_ssrf: bool,
) -> Result<OAuthTokenResponse, McpError> {
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("client_id", client_id.to_string()),
        ("code_verifier", code_verifier.to_string()),
    ];
    if let Some(client_secret) = client_secret {
        form.push(("client_secret", client_secret.to_string()));
    }
    // Pin the token endpoint's resolved address at request time, reusing the
    // enforcement decision captured on the session at login start. The browser
    // login session may sit open for minutes before the code arrives, so re-pin
    // here rather than trust a resolution performed during discovery.
    let client = pinned_oauth_client(token_endpoint, "token endpoint", enforce_ssrf).await?;
    let response = timeout(
        HTTP_REQUEST_TIMEOUT,
        client.post(token_endpoint).form(&form).send(),
    )
    .await
    .map_err(|_| McpError::Timeout("oauth authorization code token".to_string()))?
    .map_err(|error| McpError::Http(error.to_string()))?
    .error_for_status()
    .map_err(|error| McpError::Http(error.to_string()))?;
    response
        .json()
        .await
        .map_err(|error| McpError::Http(error.to_string()))
}

/// A cryptographically random CSRF `state` value. The previous
/// `pid + nanos` derivation was predictable, which — combined with the missing
/// PKCE backstop — weakened the flow against code-injection/CSRF.
fn oauth_state() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a PKCE (RFC 7636) `(code_verifier, code_challenge)` pair using the
/// `S256` method: a 32-byte random verifier and its base64url-encoded SHA-256.
fn generate_pkce() -> (String, String) {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

#[cfg(test)]
mod tests {
    use super::{generate_pkce, oauth_state};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use sha2::{Digest, Sha256};

    #[test]
    fn generate_pkce_produces_valid_s256_challenge() {
        let (verifier, challenge) = generate_pkce();
        // Verifier and challenge must be non-trivial and URL-safe base64.
        assert!(
            verifier.len() >= 43,
            "verifier too short: {}",
            verifier.len()
        );
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(
            challenge, expected,
            "challenge must be base64url(sha256(verifier))"
        );
        // Two invocations must differ (randomized).
        let (other_verifier, _) = generate_pkce();
        assert_ne!(verifier, other_verifier);
    }

    #[test]
    fn oauth_state_is_random_and_unpredictable() {
        let a = oauth_state();
        let b = oauth_state();
        assert_ne!(a, b, "state must be randomized per flow");
        assert!(a.len() >= 43, "state should carry sufficient entropy");
        // The old predictable `orbcode-<pid>-<nanos>` shape is gone.
        assert!(!a.starts_with("orbcode-"));
    }
}
