//! Server-side SSRF guard and DNS-rebinding defense for OAuth HTTP requests.
//!
//! OAuth discovery/registration/token endpoints are (transitively)
//! attacker-influenceable: they arrive via the MCP server's `WWW-Authenticate`
//! challenge and its resource / authorization-server metadata documents. A
//! *public* MCP server must not be able to steer a server-side OAuth request at
//! an internal address (`127.0.0.1`, RFC1918, `169.254.169.254`, ...).
//!
//! Resolving a host, checking the resolved IPs, and then handing the URL to a
//! fresh `reqwest::Client` leaves a DNS-rebinding window: the client re-resolves
//! the name at connect time, so an attacker domain can answer with a public IP
//! during the check and a loopback/metadata IP for the actual request (and a
//! `refresh` request may fire much later still). To close the window we resolve
//! once, validate the resolved addresses, and *pin* them onto the client via
//! `resolve_to_addrs`, so the connection targets exactly the IPs we vetted —
//! the same technique the WebFetch tool uses.

use crate::error::McpError;

/// Whether an IP address is internal — loopback, RFC1918 private, link-local
/// (incl. the `169.254.169.254` cloud-metadata endpoint), CGNAT, unspecified,
/// or IPv6 unique/link-local — and therefore an SSRF target.
pub(super) fn ip_is_internal(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1]))
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return ip_is_internal(std::net::IpAddr::V4(mapped));
            }
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Whether a host literal (name or IP) is internal, without DNS resolution.
pub(super) fn host_is_internal_literal(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host == "metadata"
        || host == "metadata.google.internal"
        || host.ends_with(".internal")
    {
        return true;
    }
    let candidate = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&host);
    candidate
        .parse::<std::net::IpAddr>()
        .is_ok_and(ip_is_internal)
}

/// Whether the MCP endpoint the user deliberately connected to is itself an
/// internal/local host. When it is, a local OAuth authorization server is
/// expected (local dev), so the SSRF guard is not enforced; when the MCP
/// endpoint is public, the guard blocks a discovery document that tries to pivot
/// the OAuth exchange to an internal address.
pub(crate) fn mcp_endpoint_is_internal(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(host_is_internal_literal))
        .unwrap_or(false)
}

fn oauth_ssrf_error(what: &str, url: &str) -> McpError {
    McpError::Protocol(format!(
        "refusing OAuth {what} `{url}`: it targets a private, loopback, link-local, \
         or cloud-metadata address"
    ))
}

/// Enforce that a public-flow OAuth endpoint uses TLS. RFC 8414 §2 requires the
/// issuer to be `https`, and RFC 6749 §3.1 requires the authorization/token
/// endpoints (and thus the credentials that flow over them — auth code, client
/// secret, access/refresh tokens) to be sent over TLS. A malicious public MCP
/// server must not be able to advertise an `http://` endpoint and downgrade the
/// exchange to plaintext. Skipped when `enforce` is false (a deliberately-local
/// MCP/loopback dev flow, where `http` is expected).
fn ensure_oauth_scheme_secure(
    url: &reqwest::Url,
    what: &str,
    enforce: bool,
) -> Result<(), McpError> {
    if enforce && url.scheme() != "https" {
        return Err(McpError::Protocol(format!(
            "refusing OAuth {what} `{url}`: a public OAuth flow must use https \
             (RFC 8414 §2, RFC 6749 §3.1); plaintext http would expose the \
             authorization code, client secret, and tokens"
        )));
    }
    Ok(())
}

/// Public entry point for the browser flow's authorization endpoint, which is
/// opened in the user's browser rather than fetched via [`pinned_oauth_client`]
/// and so needs its own TLS gate when the MCP endpoint is public.
pub(super) fn ensure_oauth_url_scheme_secure(
    url_str: &str,
    what: &str,
    enforce: bool,
) -> Result<(), McpError> {
    let url = reqwest::Url::parse(url_str).map_err(|error| {
        McpError::Protocol(format!("invalid OAuth {what} `{url_str}`: {error}"))
    })?;
    ensure_oauth_scheme_secure(&url, what, enforce)
}

/// Resolve `url_str`'s host, optionally reject internal addresses, and return a
/// redirect-disabled `reqwest::Client` **pinned** to exactly the resolved
/// addresses.
///
/// When `enforce` is set (a public MCP endpoint), the host — and every address
/// it resolves to — must not be internal, so a malicious discovery document
/// cannot point the OAuth exchange at `127.0.0.1`, `169.254.169.254`, or an
/// RFC1918 service. When `enforce` is false (a deliberately-local MCP endpoint),
/// the internal check is skipped but the addresses are still pinned so the
/// request cannot be re-resolved to a different host mid-flight.
///
/// The returned client must be used for the request to `url_str`; pinning is
/// keyed on that URL's host.
pub(super) async fn pinned_oauth_client(
    url_str: &str,
    what: &str,
    enforce: bool,
) -> Result<reqwest::Client, McpError> {
    let url = reqwest::Url::parse(url_str).map_err(|error| {
        McpError::Protocol(format!("invalid OAuth {what} `{url_str}`: {error}"))
    })?;
    // A public flow must use TLS (finding: plaintext-http downgrade).
    ensure_oauth_scheme_secure(&url, what, enforce)?;
    let host = url
        .host_str()
        .ok_or_else(|| McpError::Protocol(format!("OAuth {what} `{url_str}` has no host")))?;
    if enforce && host_is_internal_literal(host) {
        return Err(oauth_ssrf_error(what, url_str));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| {
            McpError::Http(format!(
                "DNS resolution failed for OAuth {what} `{host}`: {error}"
            ))
        })?
        .collect();
    if addrs.is_empty() {
        return Err(McpError::Http(format!(
            "no addresses resolved for OAuth {what} `{host}`"
        )));
    }
    if enforce && addrs.iter().map(|addr| addr.ip()).any(ip_is_internal) {
        return Err(oauth_ssrf_error(what, url_str));
    }
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // Disable the automatic system proxy: `resolve_to_addrs` only pins the
        // address for a DIRECT connection. With a proxy in effect, reqwest
        // connects to the proxy and the target host is resolved proxy-side,
        // which would defeat the SSRF/rebinding guard we just applied. Only
        // `no_proxy()` turns off automatic system-proxy detection.
        .no_proxy()
        .resolve_to_addrs(host, &addrs)
        .build()
        .map_err(|error| McpError::Http(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_oauth_url_scheme_secure, host_is_internal_literal, mcp_endpoint_is_internal,
        pinned_oauth_client,
    };

    #[test]
    fn internal_host_and_endpoint_context_detection() {
        assert!(host_is_internal_literal("127.0.0.1"));
        assert!(host_is_internal_literal("localhost"));
        assert!(host_is_internal_literal("169.254.169.254"));
        assert!(host_is_internal_literal("metadata.google.internal"));
        assert!(host_is_internal_literal("10.0.0.5"));
        assert!(!host_is_internal_literal("api.example.com"));

        assert!(mcp_endpoint_is_internal("http://127.0.0.1:8080/mcp"));
        assert!(mcp_endpoint_is_internal("http://localhost/mcp"));
        assert!(!mcp_endpoint_is_internal("https://api.example.com/mcp"));
    }

    #[test]
    fn public_flow_requires_https_endpoints() {
        // Public flow (enforce=true): plaintext http is rejected, https accepted.
        assert!(
            ensure_oauth_url_scheme_secure("http://as.example.com/token", "token endpoint", true)
                .is_err()
        );
        assert!(
            ensure_oauth_url_scheme_secure("https://as.example.com/token", "token endpoint", true)
                .is_ok()
        );
        // Local flow (enforce=false): http is allowed for loopback/dev.
        assert!(
            ensure_oauth_url_scheme_secure("http://127.0.0.1:9000/token", "token endpoint", false)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn pinned_client_rejects_internal_targets_when_enforced() {
        // Literal internal addresses are rejected up front (https, so the TLS
        // gate passes and we exercise the internal-address check specifically).
        assert!(
            pinned_oauth_client("https://169.254.169.254/token", "token endpoint", true)
                .await
                .is_err()
        );
        assert!(
            pinned_oauth_client("https://127.0.0.1:9000/device", "device endpoint", true)
                .await
                .is_err()
        );
        // A name that resolves to loopback is rejected once resolved.
        assert!(
            pinned_oauth_client("https://localhost/token", "token endpoint", true)
                .await
                .is_err()
        );
        // A deliberately-local endpoint (enforce=false) is allowed and pinned.
        assert!(
            pinned_oauth_client("http://127.0.0.1:9000/device", "device endpoint", false)
                .await
                .is_ok()
        );
    }
}
