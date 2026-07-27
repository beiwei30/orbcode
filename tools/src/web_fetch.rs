use std::time::Instant;

use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION};
use serde_json::json;
use url::Url;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    output::{MAX_WEB_OUTPUT_CHARS, truncate_tool_output},
    payload::{field_or_raw, parse_payload, string_field},
    permissions::{require_network, require_tools},
    web_cache::{self, CachedContent},
};

/// Counts WebFetch network attempts so tests can assert a cache hit avoids the
/// network entirely. Incremented immediately before each HTTP send.
#[cfg(test)]
pub(crate) static WEB_FETCH_NETWORK_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

const MAX_URL_LENGTH: usize = 2000;
const MAX_HTTP_CONTENT_LENGTH: usize = 10 * 1024 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 60;
const MAX_REDIRECTS: usize = 10;
const MAX_MARKDOWN_LENGTH: usize = 100_000;
const WEB_FETCH_USER_AGENT: &str = "Claude-User (claude-code/0.1; +https://support.anthropic.com/)";

pub(crate) fn validate_url(raw: &str) -> Result<Url, ToolError> {
    if raw.len() > MAX_URL_LENGTH {
        return Err(ToolError::InvalidInput(format!(
            "URL exceeds maximum length of {MAX_URL_LENGTH} characters"
        )));
    }

    let normalized = if !raw.contains("://") {
        format!("https://{raw}")
    } else if raw.starts_with("http://") {
        raw.replacen("http://", "https://", 1)
    } else {
        raw.to_string()
    };

    let parsed = Url::parse(&normalized)
        .map_err(|error| ToolError::InvalidInput(format!("invalid URL `{raw}`: {error}")))?;

    if parsed.scheme() != "https" {
        return Err(ToolError::InvalidInput(format!(
            "only HTTPS URLs are supported, got `{}`",
            parsed.scheme()
        )));
    }

    if parsed.username() != "" || parsed.password().is_some() {
        return Err(ToolError::InvalidInput(
            "URLs with embedded credentials are not supported".into(),
        ));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| ToolError::InvalidInput("URL has no host".into()))?;

    if let Some(url::Host::Domain(_)) = parsed.host()
        && host.split('.').count() < 2
    {
        return Err(ToolError::InvalidInput(format!(
            "hostname `{host}` must have at least two parts (e.g. example.com)"
        )));
    }

    Ok(parsed)
}

/// Whether the URL targets a private, loopback, link-local, or cloud-metadata
/// address that must not be reached without an explicit allow-list entry. This
/// is the SSRF guard; it is applied in the fetch flow (not in [`validate_url`])
/// so an allow-listed internal host can still be reached deliberately.
pub(crate) fn is_ssrf_blocked_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ipv4_is_blocked(ip),
        Some(url::Host::Ipv6(ip)) => ipv6_is_blocked(ip),
        Some(url::Host::Domain(host)) => domain_is_blocked(host),
        None => false,
    }
}

/// Resolve `host` to concrete socket addresses and reject the fetch if ANY
/// resolved IP is a private/loopback/link-local/metadata address (unless the
/// host is explicitly allow-listed).
///
/// The literal-host guard ([`is_ssrf_blocked_host`]) only catches IP literals
/// and a few well-known names; an attacker-controlled *public* domain can still
/// resolve to `127.0.0.1`, an RFC1918 address, or `169.254.169.254`. Resolving
/// here and returning the validated addresses — which the caller pins onto the
/// HTTP client via `resolve_to_addrs` — closes that hole and the DNS-rebinding
/// window (reqwest connects to exactly the IPs we checked, not a re-resolved
/// set).
pub(crate) async fn resolve_and_validate_host(
    url: &Url,
    host: &str,
    context: &crate::ToolContext,
) -> Result<Vec<std::net::SocketAddr>, ToolError> {
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| {
            ToolError::ExecutionFailed(format!("DNS resolution failed for {host}: {error}"))
        })?
        .collect();
    if addrs.is_empty() {
        return Err(ToolError::ExecutionFailed(format!(
            "no addresses resolved for {host}"
        )));
    }
    if !web_cache::host_explicitly_allowlisted(host, Some(context)) {
        let blocked = addrs.iter().any(|addr| match addr.ip() {
            std::net::IpAddr::V4(ip) => ipv4_is_blocked(ip),
            std::net::IpAddr::V6(ip) => ipv6_is_blocked(ip),
        });
        if blocked {
            return Err(ssrf_rejection_error(host, url.as_str()));
        }
    }
    Ok(addrs)
}

/// Build a redirect-disabled HTTP client that resolves `host` to exactly
/// `addrs` (the validated addresses), so reqwest never re-resolves the host to
/// a different — possibly internal — IP.
fn build_pinned_web_client(
    host: &str,
    addrs: &[std::net::SocketAddr],
) -> Result<reqwest::Client, ToolError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent(WEB_FETCH_USER_AGENT)
        // Disable the automatic system proxy. `resolve_to_addrs` pins the target
        // IP only for a DIRECT connection; if `HTTP(S)_PROXY` were honored,
        // reqwest would connect to the proxy and the destination host would be
        // resolved proxy-side, re-opening the SSRF/DNS-rebinding hole the
        // resolve-and-validate step just closed. Only `no_proxy()` turns off
        // automatic system-proxy detection, so the vetted-IP guarantee holds.
        .no_proxy()
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|error| ToolError::ExecutionFailed(format!("HTTP client error: {error}")))
}

pub(crate) fn ssrf_rejection_error(host: &str, url: &str) -> ToolError {
    ToolError::ExecutionFailedWithMetadata {
        message: format!(
            "refusing to fetch {url}: `{host}` resolves to a private, loopback, link-local, or \
             cloud-metadata address; add it to the web domain allowlist to fetch it deliberately"
        ),
        metadata: json!({
            "webFetch": { "url": url, "blocked": true, "reason": "ssrf_internal_host" }
        }),
    }
}

/// Whether an IPv4 literal points at a non-public address that a fetch must not
/// reach (SSRF guard): loopback, RFC1918 private, link-local (incl. the
/// `169.254.169.254` cloud-metadata endpoint), CGNAT, unspecified, broadcast,
/// and documentation ranges.
fn ipv4_is_blocked(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || octets[0] == 0
        // Carrier-grade NAT 100.64.0.0/10.
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
}

/// IPv6 counterpart to [`ipv4_is_blocked`]: loopback, unspecified, unique-local
/// (`fc00::/7`), link-local (`fe80::/10`), and any IPv4-mapped address whose
/// embedded IPv4 is itself blocked.
fn ipv6_is_blocked(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return ipv4_is_blocked(mapped);
    }
    let first = ip.segments()[0];
    // fc00::/7 (unique local) or fe80::/10 (link local).
    (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

/// Whether a hostname targets a loopback alias or a cloud-metadata service.
/// DNS is resolved by the HTTP client at connect time, so this catches the
/// literal names; IP-literal hosts are handled separately above.
fn domain_is_blocked(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "metadata"
        || host == "metadata.google.internal"
        || host.ends_with(".internal")
}

pub(crate) fn is_same_host_redirect(original: &Url, redirect: &Url) -> bool {
    if original.host() != redirect.host() {
        let orig_host = original.host_str().unwrap_or_default();
        let redir_host = redirect.host_str().unwrap_or_default();
        let matches_www = (redir_host == format!("www.{orig_host}"))
            || (orig_host == format!("www.{redir_host}"));
        if !matches_www {
            return false;
        }
    }
    if original.scheme() != redirect.scheme() {
        return false;
    }
    if original.port() != redirect.port() {
        return false;
    }
    if redirect.username() != "" || redirect.password().is_some() {
        return false;
    }
    true
}

pub(crate) fn is_binary_content_type(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if ct.starts_with("text/") {
        return false;
    }
    if ct == "application/json"
        || ct.ends_with("+json")
        || ct.ends_with("+xml")
        || ct == "application/xml"
        || ct == "application/javascript"
        || ct == "application/x-www-form-urlencoded"
    {
        return false;
    }
    true
}

pub(crate) fn is_html_content_type(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    ct == "text/html" || ct == "application/xhtml+xml"
}

pub(crate) fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}

/// Build a WebFetch outcome from cached-or-freshly-fetched content. `cache_age_ms`
/// is `Some` only on a cache hit, driving the `cached`/`cacheAgeMs` metadata.
fn build_fetch_outcome(
    raw_url: &str,
    prompt: Option<&str>,
    cached: &CachedContent,
    duration_ms: u64,
    cache_age_ms: Option<u64>,
) -> ToolOutcome {
    let output = if let Some(prompt_text) = prompt {
        format!(
            "User prompt: {prompt_text}\n\n---\n\nFetched content from {raw_url}:\n\n{}",
            cached.content
        )
    } else {
        cached.content.clone()
    };

    ToolOutcome {
        name: "web-fetch".into(),
        summary: format!("Fetched {raw_url}."),
        output,
        metadata: Some(json!({
            "webFetch": {
                "url": raw_url,
                "finalUrl": cached.final_url,
                "statusCode": cached.status_code,
                "contentType": cached.content_type,
                "convertedToMarkdown": cached.converted_to_markdown,
                "redirected": cached.redirected,
                "redirectCount": cached.redirect_count,
                "durationMs": duration_ms,
                "truncated": cached.truncated,
                "responseBytes": cached.response_bytes,
                "cached": cache_age_ms.is_some(),
                "cacheAgeMs": cache_age_ms,
            }
        })),
        changed_paths: Vec::new(),
    }
}

impl ToolRegistry {
    pub(crate) async fn web_fetch(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        require_network(context)?;

        let payload = parse_payload(input)?;
        let raw_url = field_or_raw(&payload, "url", input)?;
        let prompt = string_field(&payload, "prompt");

        let start = Instant::now();
        let url = validate_url(&raw_url)?;

        // Domain preflight: reject blocked / non-allowlisted hosts before any
        // network request, cache lookup, or DNS resolution happens.
        let host = url.host_str().unwrap_or_default().to_string();
        if let Err(rejection) = web_cache::check_domain(&host, Some(context)) {
            return Err(ToolError::ExecutionFailedWithMetadata {
                message: rejection.message(&raw_url),
                metadata: json!({
                    "webFetch": {
                        "url": raw_url,
                        "blocked": true,
                        "blockReason": rejection.reason(),
                        "durationMs": start.elapsed().as_millis() as u64,
                    }
                }),
            });
        }

        // SSRF guard: refuse private/loopback/link-local/cloud-metadata targets
        // unless the host is explicitly on the web domain allowlist (letting a
        // user reach an internal service deliberately).
        if is_ssrf_blocked_host(&url)
            && !web_cache::host_explicitly_allowlisted(&host, Some(context))
        {
            return Err(ssrf_rejection_error(&host, &raw_url));
        }

        // Content cache: a hit within the TTL returns byte-identical content
        // without touching the network. The lookup is scoped to the active
        // domain policy (see `web_cache::lookup`), so a hit cached under an
        // allow-listed context is NOT served to a context with a different
        // policy — that becomes a miss and takes the full SSRF-validation path
        // below.
        let cache_key = url.as_str().to_string();
        if let Some(hit) = web_cache::lookup(&cache_key, Some(context)) {
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(build_fetch_outcome(
                &raw_url,
                prompt.as_deref(),
                &hit.content,
                duration_ms,
                Some(hit.age_ms),
            ));
        }

        // Resolve the host and reject if it points at an internal address —
        // catching attacker-controlled public domains that resolve to
        // loopback/RFC1918/metadata. Pin the validated addresses onto the client
        // so reqwest connects to exactly those IPs (no rebinding re-resolution).
        // `is_same_host_redirect` treats `example.com` and `www.example.com` as
        // the same host, so a redirect CAN change the host; the loop re-validates
        // and re-pins whenever it does, or the www-variant would be re-resolved
        // by reqwest and reopen the SSRF hole.
        let mut current_host = host.clone();
        let resolved_addrs = resolve_and_validate_host(&url, &current_host, context).await?;
        let mut client = build_pinned_web_client(&current_host, &resolved_addrs)?;

        let mut redirects = 0u32;
        let mut final_url = url.clone();

        let response = loop {
            #[cfg(test)]
            WEB_FETCH_NETWORK_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            let resp = client
                .get(final_url.as_str())
                .header(ACCEPT, "text/markdown, text/html, */*")
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        ToolError::ExecutionFailed(format!(
                            "Request timed out after {FETCH_TIMEOUT_SECS}s for {raw_url}"
                        ))
                    } else if error.is_connect() {
                        ToolError::ExecutionFailed(format!(
                            "Connection failed for {raw_url}: {error}"
                        ))
                    } else {
                        ToolError::ExecutionFailed(format!(
                            "HTTP request failed for {raw_url}: {error}"
                        ))
                    }
                })?;

            let status = resp.status();
            if status.is_redirection()
                && let Some(location) = resp.headers().get(LOCATION)
            {
                let location_str = location.to_str().unwrap_or_default();
                let redirect_url = final_url.join(location_str).map_err(|error| {
                    ToolError::ExecutionFailed(format!(
                        "invalid redirect URL `{location_str}`: {error}"
                    ))
                })?;

                if is_same_host_redirect(&url, &redirect_url) {
                    redirects += 1;
                    if redirects as usize > MAX_REDIRECTS {
                        return Err(ToolError::ExecutionFailed(format!(
                            "too many redirects (>{MAX_REDIRECTS}) for {raw_url}"
                        )));
                    }
                    // A same-host redirect may still change the host (the
                    // www/non-www variant). Re-validate the new host's DNS and
                    // re-pin the client, so a www redirect to a loopback/private/
                    // metadata IP is rejected instead of being re-resolved freely.
                    let redirect_host = redirect_url.host_str().unwrap_or_default().to_string();
                    if redirect_host != current_host {
                        let addrs =
                            resolve_and_validate_host(&redirect_url, &redirect_host, context)
                                .await?;
                        client = build_pinned_web_client(&redirect_host, &addrs)?;
                        current_host = redirect_host;
                    }
                    final_url = redirect_url;
                    continue;
                }

                let duration_ms = start.elapsed().as_millis() as u64;
                return Ok(ToolOutcome {
                    name: "web-fetch".into(),
                    summary: format!(
                        "Redirected to different host: {}",
                        redirect_url.host_str().unwrap_or("unknown")
                    ),
                    output: format!(
                        "The URL {raw_url} redirected to a different host: {redirect_url}\n\n\
                         To follow this redirect, make a new WebFetch request with the URL: {redirect_url}"
                    ),
                    metadata: Some(json!({
                        "webFetch": {
                            "url": raw_url,
                            "redirectUrl": redirect_url.to_string(),
                            "crossHostRedirect": true,
                            "durationMs": duration_ms,
                        }
                    })),
                    changed_paths: Vec::new(),
                });
            }

            break resp;
        };

        let status = response.status();
        if !status.is_success() {
            let duration_ms = start.elapsed().as_millis() as u64;
            let status_code = status.as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect::<String>();
            return Err(ToolError::ExecutionFailedWithMetadata {
                message: format!("HTTP {status_code} for {raw_url}: {body}"),
                metadata: json!({
                    "webFetch": {
                        "url": raw_url,
                        "statusCode": status_code,
                        "durationMs": duration_ms,
                    }
                }),
            });
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();

        if is_binary_content_type(&content_type) {
            let duration_ms = start.elapsed().as_millis() as u64;
            return Err(ToolError::ExecutionFailedWithMetadata {
                message: format!(
                    "Cannot process binary content type `{content_type}` from {raw_url}. \
                     WebFetch only supports text and HTML content."
                ),
                metadata: json!({
                    "webFetch": {
                        "url": raw_url,
                        "contentType": content_type,
                        "binary": true,
                        "durationMs": duration_ms,
                    }
                }),
            });
        }

        let content_length = response.content_length().unwrap_or(0) as usize;
        if content_length > MAX_HTTP_CONTENT_LENGTH {
            let duration_ms = start.elapsed().as_millis() as u64;
            return Err(ToolError::ExecutionFailedWithMetadata {
                message: format!(
                    "Response too large ({content_length} bytes, max {MAX_HTTP_CONTENT_LENGTH} bytes) for {raw_url}"
                ),
                metadata: json!({
                    "webFetch": {
                        "url": raw_url,
                        "contentLength": content_length,
                        "durationMs": duration_ms,
                    }
                }),
            });
        }

        let bytes = read_response_bytes(response, MAX_HTTP_CONTENT_LENGTH).await?;
        let raw_text = String::from_utf8_lossy(&bytes).to_string();
        let duration_ms = start.elapsed().as_millis() as u64;

        let is_html = is_html_content_type(&content_type);
        let content = if is_html {
            html_to_markdown(&raw_text)
        } else {
            raw_text
        };

        let truncated = content.chars().count() > MAX_MARKDOWN_LENGTH;
        let output = if truncated {
            truncate_tool_output(
                content,
                MAX_WEB_OUTPUT_CHARS,
                "Content truncated due to length.",
            )
        } else {
            content
        };

        let cached = CachedContent {
            content: output,
            final_url: final_url.to_string(),
            status_code: status.as_u16(),
            content_type,
            converted_to_markdown: is_html,
            redirected: redirects > 0,
            redirect_count: redirects,
            truncated,
            response_bytes: bytes.len(),
        };
        web_cache::store(&cache_key, cached.clone(), Some(context));

        Ok(build_fetch_outcome(
            &raw_url,
            prompt.as_deref(),
            &cached,
            duration_ms,
            None,
        ))
    }
}

async fn read_response_bytes(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ToolError> {
    let mut bytes =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(max_bytes as u64) as usize);

    let mut stream = response;
    while let Some(chunk) = stream.chunk().await.map_err(|error| {
        ToolError::ExecutionFailed(format!("failed to read response body: {error}"))
    })? {
        if bytes.len() + chunk.len() > max_bytes {
            return Err(ToolError::ExecutionFailed(format!(
                "response body exceeds {max_bytes} byte limit"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        is_same_host_redirect, is_ssrf_blocked_host, resolve_and_validate_host, validate_url,
    };
    use crate::ToolCancellationToken;
    use orbcode_protocol::SandboxMode;
    use std::path::Path;
    use url::Url;

    async fn minimal_context(cwd: &Path) -> crate::ToolContext {
        let home = cwd.join("home");
        std::fs::create_dir_all(&home).expect("create home");
        let mcp = orbcode_mcp::McpRegistry::load(&home, cwd)
            .await
            .expect("load mcp");
        crate::ToolContext {
            cwd: cwd.to_path_buf(),
            additional_directories: Vec::new(),
            home_dir: home,
            sandbox_mode: SandboxMode::DangerFullAccess,
            sandbox_allow_network: true,
            allow_network: true,
            allow_tools: true,
            mcp,
            progress: None,
            cancellation: ToolCancellationToken::default(),
            read_state: None,
            session_id: None,
            local_shell_tasks: None,
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: std::collections::BTreeMap::new(),
            skill_definitions: None,
        }
    }

    #[tokio::test]
    async fn redirect_host_resolving_to_loopback_is_rejected() {
        // The fetch loop re-resolves and re-validates the NEW host after a
        // (www-variant) same-host redirect via `resolve_and_validate_host`, so a
        // redirect that re-resolves to a private/loopback address is rejected
        // there rather than fetched. This guards that rejection primitive:
        // `localhost` resolves to loopback on every supported platform.
        let dir = tempfile::tempdir().expect("tempdir");
        let context = minimal_context(dir.path()).await;
        let url = Url::parse("https://localhost/after-redirect").expect("parse");
        let error = resolve_and_validate_host(&url, "localhost", &context)
            .await
            .expect_err("a host resolving to loopback must be rejected");
        assert!(
            error.to_string().contains("private, loopback"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn www_variant_redirect_is_same_host_so_revalidation_runs() {
        // A www/non-www redirect is classified same-host, so the loop FOLLOWS it
        // — and (per the test above) re-resolves + re-validates the new host,
        // closing the DNS-rebinding hole. A genuinely different host is not
        // followed at all.
        let original = Url::parse("https://example.com/a").expect("parse");
        let www = Url::parse("https://www.example.com/a").expect("parse");
        assert!(is_same_host_redirect(&original, &www));
        assert!(is_same_host_redirect(&www, &original));
        let other = Url::parse("https://evil.example/a").expect("parse");
        assert!(!is_same_host_redirect(&original, &other));
    }

    #[test]
    fn is_ssrf_blocked_host_flags_internal_targets() {
        for raw in [
            "https://127.0.0.1/",
            "https://192.168.1.1/",
            "https://10.0.0.5/",
            "https://172.16.0.1/",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::1]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
            "https://metadata.google.internal/",
            "https://foo.internal/",
            "https://0.0.0.0/",
            "https://100.64.0.1/",
        ] {
            let url = Url::parse(raw).expect("parse");
            assert!(
                is_ssrf_blocked_host(&url),
                "expected `{raw}` to be flagged as an SSRF target"
            );
        }
    }

    #[test]
    fn is_ssrf_blocked_host_allows_public_targets() {
        for raw in [
            "https://example.com/path",
            "https://8.8.8.8/",
            "https://93.184.216.34/",
        ] {
            let url = Url::parse(raw).expect("parse");
            assert!(!is_ssrf_blocked_host(&url), "`{raw}` should be allowed");
        }
    }

    #[test]
    fn validate_url_still_rejects_single_label_and_upgrades_scheme() {
        assert!(validate_url("https://localhost/path").is_err());
        assert!(validate_url("https://example.com/path").is_ok());
        assert!(validate_url("example.com").is_ok());
    }
}
