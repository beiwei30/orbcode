use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;
use reqwest::header::ACCEPT;
use serde_json::json;
use url::Url;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    encoding::percent_encode,
    output::{MAX_WEB_OUTPUT_CHARS, truncate_tool_output},
    payload::{field_or_raw, parse_payload, string_field_any},
    permissions::{require_network, require_tools},
    web_cache,
    web_search_adapters::{SearchEngine, search_brave, search_engine_order, search_exa},
};

const BING_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0";
const WEB_FETCH_USER_AGENT: &str = "Claude-User (claude-code/0.1; +https://support.anthropic.com/)";
pub(crate) const SEARCH_TIMEOUT_SECS: u64 = 15;

static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>").expect("compile tag strip regex"));
static ENTITY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&(?:#(\d+)|#x([0-9a-fA-F]+)|(\w+));").expect("compile entity regex")
});
static BING_TITLE_LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a[^>]*target="_blank"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
        .expect("compile bing title regex")
});
static BING_SNIPPET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<p class="b_lineclamp[^"]*"[^>]*>(.*?)</p>"#)
        .expect("compile bing snippet regex")
});
static BING_SNIPPET_FALLBACK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<div class="b_caption"[^>]*><p>(.*?)</p>"#)
        .expect("compile bing snippet fallback regex")
});
static DDG_LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a[^>]+class="[^"]*result__a[^"]*"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
        .expect("compile result link regex")
});
static DDG_SNIPPET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>"#)
        .expect("compile snippet regex")
});

impl ToolRegistry {
    pub(crate) async fn web_search(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        require_network(context)?;

        let payload = parse_payload(input)?;
        let query = field_or_raw(&payload, "query", input)?;
        let allowed_domains = string_field_any(&payload, &["allowed_domains"]);
        let blocked_domains = string_field_any(&payload, &["blocked_domains"]);

        let start = Instant::now();

        // Try each configured backend in priority order, stopping at the first
        // engine that yields a non-empty result set. If every engine errors we
        // surface the first error; if every engine simply returns empty we fall
        // through with no results (engine = first in the chain).
        let order = search_engine_order();
        let mut results = Vec::new();
        let mut engine = order.first().map_or("bing", |e| e.label());
        let mut first_error: Option<ToolError> = None;

        for candidate in &order {
            match run_search_engine(*candidate, &query).await {
                Ok(found) if !found.is_empty() => {
                    results = found;
                    engine = candidate.label();
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        if results.is_empty()
            && let Some(error) = first_error
        {
            return Err(error);
        }

        // Match on the URL host, not a raw substring: `url.contains("github.com")`
        // would also accept `attacker.com/?to=github.com`. A result with no
        // parseable host is dropped by an allow-list (can't confirm it matches)
        // and kept by a block-list (can't confirm it's blocked).
        if let Some(ref allowed) = allowed_domains {
            let domains: Vec<String> = allowed
                .split(',')
                .map(|d| d.trim().to_ascii_lowercase())
                .filter(|d| !d.is_empty())
                .collect();
            results.retain(|r| {
                result_host(&r.url)
                    .is_some_and(|host| domains.iter().any(|d| web_cache::host_matches(&host, d)))
            });
        }
        if let Some(ref blocked) = blocked_domains {
            let domains: Vec<String> = blocked
                .split(',')
                .map(|d| d.trim().to_ascii_lowercase())
                .filter(|d| !d.is_empty())
                .collect();
            results.retain(|r| {
                result_host(&r.url)
                    .is_none_or(|host| !domains.iter().any(|d| web_cache::host_matches(&host, d)))
            });
        }

        // Configured (settings/env) domain policy filters result hosts in
        // addition to the per-call allowed/blocked lists above.
        filter_results_by_domain_policy(&mut results, Some(context));

        let duration_ms = start.elapsed().as_millis() as u64;
        let num_results = results.len();

        let mut output = format!("Web search results for query: \"{query}\"\nLinks:\n");
        for result in &results {
            let snippet = if result.snippet.is_empty() {
                String::new()
            } else {
                format!(": {}", result.snippet)
            };
            writeln!(output, "  - [{}]({}){}", result.title, result.url, snippet)
                .expect("writing to String cannot fail");
        }

        if results.is_empty() {
            output.push_str("  (no results found)\n");
        }

        output.push_str(
            "\nREMINDER: You MUST include the sources above in a \"Sources:\" section at the end of your response.",
        );

        let output = truncate_tool_output(
            output,
            MAX_WEB_OUTPUT_CHARS,
            "Search output truncated. Narrow the query for more focused results.",
        );

        Ok(ToolOutcome {
            name: "web-search".into(),
            summary: format!("Found {num_results} results for `{query}`."),
            output,
            metadata: Some(json!({
                "webSearch": {
                    "query": query,
                    "numResults": num_results,
                    "durationMs": duration_ms,
                    "engine": engine,
                }
            })),
            changed_paths: Vec::new(),
        })
    }
}

/// Extract the lowercased host of a result URL, if it parses and has one.
fn result_host(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
}

/// Drop search results whose host is disallowed by the configured domain policy.
/// Results with an unparseable URL are kept (the model still sees the link text).
pub(crate) fn filter_results_by_domain_policy(
    results: &mut Vec<SearchResult>,
    context: Option<&crate::ToolContext>,
) {
    results.retain(|result| match Url::parse(&result.url) {
        Ok(parsed) => parsed
            .host_str()
            .is_none_or(|host| web_cache::host_allowed(host, context)),
        Err(_) => true,
    });
}

/// Dispatch a single query to one search backend.
async fn run_search_engine(
    engine: SearchEngine,
    query: &str,
) -> Result<Vec<SearchResult>, ToolError> {
    match engine {
        SearchEngine::Bing => search_bing(query).await,
        SearchEngine::Brave => search_brave(query).await,
        SearchEngine::Exa => search_exa(query).await,
        SearchEngine::DuckDuckGo => search_ddg(query).await,
    }
}

async fn search_bing(query: &str) -> Result<Vec<SearchResult>, ToolError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS))
        .build()
        .map_err(|error| ToolError::ExecutionFailed(format!("HTTP client error: {error}")))?;

    let search_url = format!(
        "https://www.bing.com/search?q={}&setmkt=en-US",
        percent_encode(query)
    );

    let resp = client
        .get(&search_url)
        .header(ACCEPT, "text/html")
        .header(reqwest::header::USER_AGENT, BING_USER_AGENT)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                ToolError::ExecutionFailed(format!("Bing search timed out for `{query}`"))
            } else {
                ToolError::ExecutionFailed(format!(
                    "Bing search request failed for `{query}`: {error}"
                ))
            }
        })?;

    if !resp.status().is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "Bing search returned HTTP {} for `{query}`",
            resp.status()
        )));
    }

    let body = resp.text().await.map_err(|error| {
        ToolError::ExecutionFailed(format!("Failed to read Bing response: {error}"))
    })?;

    Ok(extract_bing_results(&body))
}

async fn search_ddg(query: &str) -> Result<Vec<SearchResult>, ToolError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS))
        .user_agent(WEB_FETCH_USER_AGENT)
        .build()
        .map_err(|error| ToolError::ExecutionFailed(format!("HTTP client error: {error}")))?;

    let search_url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        percent_encode(query)
    );

    let resp = client
        .get(&search_url)
        .header(ACCEPT, "text/html")
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                ToolError::ExecutionFailed(format!("DuckDuckGo search timed out for `{query}`"))
            } else {
                ToolError::ExecutionFailed(format!(
                    "DuckDuckGo search request failed for `{query}`: {error}"
                ))
            }
        })?;

    if !resp.status().is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "DuckDuckGo search returned HTTP {} for `{query}`",
            resp.status()
        )));
    }

    let body = resp.text().await.map_err(|error| {
        ToolError::ExecutionFailed(format!("Failed to read DuckDuckGo response: {error}"))
    })?;

    Ok(extract_ddg_results(&body))
}

pub(crate) fn extract_bing_results(html: &str) -> Vec<SearchResult> {
    let blocks: Vec<&str> = html.split("<li class=\"b_algo\"").skip(1).collect();

    let mut results = Vec::new();
    for block in blocks.iter().take(10) {
        let mut url = None;
        let mut title = None;

        for caps in BING_TITLE_LINK_RE.captures_iter(block) {
            let href = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let inner = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            let clean = TAG_RE.replace_all(inner, "");
            let clean = clean.trim();
            if clean.is_empty() {
                continue;
            }
            if url.is_none() {
                url = Some(href.to_string());
            }
            if !clean.contains("://") && title.is_none() {
                title = Some(decode_html_entities(clean, &ENTITY_RE));
            }
        }

        let raw_snippet = BING_SNIPPET_RE
            .captures(block)
            .or_else(|| BING_SNIPPET_FALLBACK_RE.captures(block))
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let snippet = decode_html_entities(&TAG_RE.replace_all(&raw_snippet, ""), &ENTITY_RE)
            .trim()
            .to_string();

        if let (Some(url), Some(title)) = (url, title)
            && !url.is_empty()
            && !title.is_empty()
        {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }

    results
}

pub(crate) struct SearchResult {
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) snippet: String,
}

#[cfg(test)]
pub(crate) fn extract_search_results(html: &str) -> Vec<SearchResult> {
    extract_ddg_results(html)
}

pub(crate) fn extract_ddg_results(html: &str) -> Vec<SearchResult> {
    let blocks: Vec<&str> = html.split("<div class=\"result ").skip(1).collect();

    let mut results = Vec::new();
    for block in blocks.iter().take(10) {
        let raw_url = match DDG_LINK_RE.captures(block) {
            Some(caps) => caps.get(1).map(|m| m.as_str().to_string()),
            None => continue,
        };
        let raw_title = DDG_LINK_RE
            .captures(block)
            .and_then(|caps| caps.get(2))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let raw_snippet = DDG_SNIPPET_RE
            .captures(block)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let title = decode_html_entities(&TAG_RE.replace_all(&raw_title, ""), &ENTITY_RE);
        let snippet = decode_html_entities(&TAG_RE.replace_all(&raw_snippet, ""), &ENTITY_RE);

        if let Some(href) = raw_url {
            let url = resolve_ddg_redirect(&href);
            if !url.is_empty() && !title.is_empty() {
                results.push(SearchResult {
                    title: title.trim().to_string(),
                    url,
                    snippet: snippet.trim().to_string(),
                });
            }
        }
    }

    results
}

pub(crate) fn resolve_ddg_redirect(href: &str) -> String {
    if let Some(pos) = href.find("uddg=") {
        let encoded = &href[pos + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        url_decode(&encoded[..end])
    } else if href.starts_with("http") {
        href.to_string()
    } else {
        String::new()
    }
}

fn url_decode(input: &str) -> String {
    // Accumulate the decoded *bytes* and interpret them as UTF-8 at the end.
    // Pushing each `%XX` byte directly `as char` treats it as Latin-1, so a
    // percent-encoded UTF-8 sequence like `%C3%A9` (é) became `Ã©`.
    let mut bytes: Vec<u8> = Vec::with_capacity(input.len());
    let mut iter = input.bytes();
    while let Some(b) = iter.next() {
        match b {
            b'%' => {
                let hi = iter.next();
                let lo = iter.next();
                match (hi, lo) {
                    (Some(hi), Some(lo))
                        if (hi as char).is_ascii_hexdigit() && (lo as char).is_ascii_hexdigit() =>
                    {
                        let value = ((hi as char).to_digit(16).unwrap() * 16
                            + (lo as char).to_digit(16).unwrap())
                            as u8;
                        bytes.push(value);
                    }
                    // Malformed escape: preserve the literal bytes read.
                    (hi, lo) => {
                        bytes.push(b'%');
                        if let Some(hi) = hi {
                            bytes.push(hi);
                        }
                        if let Some(lo) = lo {
                            bytes.push(lo);
                        }
                    }
                }
            }
            b'+' => bytes.push(b' '),
            other => bytes.push(other),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Strip HTML tags and decode entities from `raw`, returning trimmed plain text.
/// Shared by the search adapters that receive snippet/title fragments containing
/// inline markup (e.g. Brave's `<strong>` highlights).
pub(crate) fn clean_html_text(raw: &str) -> String {
    decode_html_entities(&TAG_RE.replace_all(raw, ""), &ENTITY_RE)
        .trim()
        .to_string()
}

fn decode_html_entities(input: &str, entity_re: &Regex) -> String {
    entity_re
        .replace_all(input, |caps: &regex::Captures| {
            if let Some(decimal) = caps.get(1) {
                decimal
                    .as_str()
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32)
                    .map_or_else(|| caps[0].to_string(), |c| c.to_string())
            } else if let Some(hex) = caps.get(2) {
                u32::from_str_radix(hex.as_str(), 16)
                    .ok()
                    .and_then(char::from_u32)
                    .map_or_else(|| caps[0].to_string(), |c| c.to_string())
            } else if let Some(name) = caps.get(3) {
                match name.as_str() {
                    "amp" => "&".into(),
                    "lt" => "<".into(),
                    "gt" => ">".into(),
                    "quot" => "\"".into(),
                    "apos" => "'".into(),
                    "nbsp" => " ".into(),
                    _ => caps[0].to_string(),
                }
            } else {
                caps[0].to_string()
            }
        })
        .to_string()
}
