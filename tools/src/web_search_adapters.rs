//! Secondary web-search backends (Brave, Exa) plus the configurable engine
//! fallback chain shared with [`crate::web_search`].
//!
//! Each adapter parses a saved provider response into the common
//! [`SearchResult`] shape so the parsing logic is exercised by fixtures without
//! touching the network. The live `search_*` helpers require an API key from the
//! environment and are skipped (returning an empty result set) when absent, so a
//! missing key degrades to the next engine in the chain rather than erroring.

use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::json;

use crate::{
    ToolError,
    encoding::percent_encode,
    web_search::{SEARCH_TIMEOUT_SECS, SearchResult, clean_html_text},
};

const ENV_SEARCH_ORDER: &str = "ORBCODE_WEB_SEARCH_ORDER";
const ENV_BRAVE_KEY: &str = "BRAVE_API_KEY";
const ENV_EXA_KEY: &str = "EXA_API_KEY";

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const EXA_ENDPOINT: &str = "https://api.exa.ai/search";

/// Maximum results pulled from any single adapter, matching the Bing/DDG cap.
const MAX_RESULTS: usize = 10;

/// A web-search backend. Ordered by the configurable fallback chain; the first
/// engine to return a non-empty result set wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchEngine {
    Bing,
    Brave,
    Exa,
    DuckDuckGo,
}

impl SearchEngine {
    /// Stable lowercase identifier used in metadata and order parsing.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SearchEngine::Bing => "bing",
            SearchEngine::Brave => "brave",
            SearchEngine::Exa => "exa",
            SearchEngine::DuckDuckGo => "duckduckgo",
        }
    }

    fn parse(token: &str) -> Option<SearchEngine> {
        match token.trim().to_ascii_lowercase().as_str() {
            "bing" => Some(SearchEngine::Bing),
            "brave" => Some(SearchEngine::Brave),
            "exa" => Some(SearchEngine::Exa),
            "duckduckgo" | "ddg" => Some(SearchEngine::DuckDuckGo),
            _ => None,
        }
    }
}

/// Default fallback chain: Bing primary, Brave/Exa secondary, DuckDuckGo last.
pub(crate) const DEFAULT_SEARCH_ORDER: [SearchEngine; 4] = [
    SearchEngine::Bing,
    SearchEngine::Brave,
    SearchEngine::Exa,
    SearchEngine::DuckDuckGo,
];

/// Resolve the active engine order, honoring the `ORBCODE_WEB_SEARCH_ORDER`
/// override (comma-separated engine labels) and falling back to the default.
pub(crate) fn search_engine_order() -> Vec<SearchEngine> {
    search_engine_order_from(std::env::var(ENV_SEARCH_ORDER).ok().as_deref())
}

/// Pure resolver behind [`search_engine_order`]: parses an explicit override
/// string. Unknown tokens are ignored and duplicates collapse to first mention;
/// an absent or fully-unparseable value yields [`DEFAULT_SEARCH_ORDER`].
pub(crate) fn search_engine_order_from(raw: Option<&str>) -> Vec<SearchEngine> {
    let Some(raw) = raw else {
        return DEFAULT_SEARCH_ORDER.to_vec();
    };

    let mut order: Vec<SearchEngine> = Vec::new();
    for token in raw.split(',') {
        if let Some(engine) = SearchEngine::parse(token)
            && !order.contains(&engine)
        {
            order.push(engine);
        }
    }

    if order.is_empty() {
        DEFAULT_SEARCH_ORDER.to_vec()
    } else {
        order
    }
}

#[derive(Deserialize)]
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWebResults>,
}

#[derive(Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveWebItem>,
}

#[derive(Deserialize)]
struct BraveWebItem {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResultItem>,
}

#[derive(Deserialize)]
struct ExaResultItem {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    highlights: Option<Vec<String>>,
    #[serde(default)]
    text: Option<String>,
}

/// Parse a Brave Search API JSON response (`web.results[]`) into results.
/// Snippet text (`description`) may contain inline `<strong>` highlight markup,
/// which is stripped and entity-decoded via [`clean_html_text`].
pub(crate) fn extract_brave_results(body: &str) -> Vec<SearchResult> {
    let Ok(parsed) = serde_json::from_str::<BraveResponse>(body) else {
        return Vec::new();
    };

    let Some(web) = parsed.web else {
        return Vec::new();
    };

    let mut results = Vec::new();
    for item in web.results.iter().take(MAX_RESULTS) {
        let title = clean_html_text(&item.title);
        let snippet = clean_html_text(&item.description);

        if !item.url.is_empty() && !title.is_empty() {
            results.push(SearchResult {
                title,
                url: item.url.clone(),
                snippet,
            });
        }
    }

    results
}

/// Parse an Exa `/search` JSON response (`results[]`) into results. The snippet
/// prefers an explicit `snippet`, then joined `highlights`, then a trimmed
/// `text` excerpt.
pub(crate) fn extract_exa_results(body: &str) -> Vec<SearchResult> {
    let Ok(parsed) = serde_json::from_str::<ExaResponse>(body) else {
        return Vec::new();
    };

    let mut results = Vec::new();
    for item in parsed.results.iter().take(MAX_RESULTS) {
        let title = clean_html_text(&item.title);
        let snippet = exa_snippet(item);

        if !item.url.is_empty() && !title.is_empty() {
            results.push(SearchResult {
                title,
                url: item.url.clone(),
                snippet,
            });
        }
    }

    results
}

fn exa_snippet(item: &ExaResultItem) -> String {
    if let Some(snippet) = item.snippet.as_deref()
        && !snippet.trim().is_empty()
    {
        return clean_html_text(snippet);
    }

    if let Some(highlights) = &item.highlights {
        let joined = highlights
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" … ");
        if !joined.trim().is_empty() {
            return clean_html_text(&joined);
        }
    }

    let text = item.text.as_deref().unwrap_or_default();
    let trimmed: String = text.trim().chars().take(300).collect();
    clean_html_text(&trimmed)
}

/// Query Brave's API. Returns an empty set (skipping the engine) when no
/// `BRAVE_API_KEY` is configured.
pub(crate) async fn search_brave(query: &str) -> Result<Vec<SearchResult>, ToolError> {
    let Some(key) = api_key(ENV_BRAVE_KEY) else {
        return Ok(Vec::new());
    };

    let client = search_client()?;
    let url = format!("{BRAVE_ENDPOINT}?q={}", percent_encode(query));
    let resp = client
        .get(&url)
        .header(ACCEPT, "application/json")
        .header("X-Subscription-Token", key)
        .send()
        .await
        .map_err(|error| engine_error("Brave", query, &error))?;

    if !resp.status().is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "Brave search returned HTTP {} for `{query}`",
            resp.status()
        )));
    }

    let body = resp.text().await.map_err(|error| {
        ToolError::ExecutionFailed(format!("Failed to read Brave response: {error}"))
    })?;

    Ok(extract_brave_results(&body))
}

/// Query Exa's API. Returns an empty set (skipping the engine) when no
/// `EXA_API_KEY` is configured.
pub(crate) async fn search_exa(query: &str) -> Result<Vec<SearchResult>, ToolError> {
    let Some(key) = api_key(ENV_EXA_KEY) else {
        return Ok(Vec::new());
    };

    let client = search_client()?;
    let resp = client
        .post(EXA_ENDPOINT)
        .header(CONTENT_TYPE, "application/json")
        .header("x-api-key", key)
        .body(
            json!({
                "query": query,
                "numResults": MAX_RESULTS,
            })
            .to_string(),
        )
        .send()
        .await
        .map_err(|error| engine_error("Exa", query, &error))?;

    if !resp.status().is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "Exa search returned HTTP {} for `{query}`",
            resp.status()
        )));
    }

    let body = resp.text().await.map_err(|error| {
        ToolError::ExecutionFailed(format!("Failed to read Exa response: {error}"))
    })?;

    Ok(extract_exa_results(&body))
}

fn api_key(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn search_client() -> Result<reqwest::Client, ToolError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS))
        .build()
        .map_err(|error| ToolError::ExecutionFailed(format!("HTTP client error: {error}")))
}

fn engine_error(engine: &str, query: &str, error: &reqwest::Error) -> ToolError {
    if error.is_timeout() {
        ToolError::ExecutionFailed(format!("{engine} search timed out for `{query}`"))
    } else {
        ToolError::ExecutionFailed(format!(
            "{engine} search request failed for `{query}`: {error}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_order_defaults_when_unset() {
        assert_eq!(
            search_engine_order_from(None),
            DEFAULT_SEARCH_ORDER.to_vec()
        );
    }

    #[test]
    fn engine_order_honors_override_and_dedupes() {
        let order = search_engine_order_from(Some("exa, brave , bing,exa"));
        assert_eq!(
            order,
            vec![SearchEngine::Exa, SearchEngine::Brave, SearchEngine::Bing,]
        );
    }

    #[test]
    fn engine_order_ignores_unknown_tokens() {
        let order = search_engine_order_from(Some("nope,ddg,garbage"));
        assert_eq!(order, vec![SearchEngine::DuckDuckGo]);
    }

    #[test]
    fn engine_order_falls_back_when_all_tokens_invalid() {
        assert_eq!(
            search_engine_order_from(Some("foo,bar")),
            DEFAULT_SEARCH_ORDER.to_vec()
        );
    }

    #[test]
    fn brave_results_parse_from_fixture() {
        let body = r#"
        {
          "web": {
            "results": [
              {
                "title": "Example &amp; Page",
                "url": "https://example.com/page",
                "description": "This is the <strong>snippet</strong> &lt;text&gt;."
              },
              {
                "title": "Other Result",
                "url": "https://other.org/docs",
                "description": "Second snippet here."
              }
            ]
          }
        }
        "#;

        let results = extract_brave_results(body);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example & Page");
        assert_eq!(results[0].url, "https://example.com/page");
        assert_eq!(results[0].snippet, "This is the snippet <text>.");
        assert_eq!(results[1].title, "Other Result");
        assert_eq!(results[1].url, "https://other.org/docs");
    }

    #[test]
    fn brave_results_empty_on_malformed_json() {
        assert!(extract_brave_results("not json").is_empty());
        assert!(extract_brave_results("{}").is_empty());
    }

    #[test]
    fn exa_results_parse_snippet_field() {
        let body = r#"
        {
          "results": [
            {
              "title": "Exa Example",
              "url": "https://example.com/exa",
              "snippet": "A direct &amp; snippet."
            }
          ]
        }
        "#;

        let results = extract_exa_results(body);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Exa Example");
        assert_eq!(results[0].url, "https://example.com/exa");
        assert_eq!(results[0].snippet, "A direct & snippet.");
    }

    #[test]
    fn exa_results_fall_back_to_highlights_then_text() {
        let highlights = r#"
        {
          "results": [
            {
              "title": "Highlighted",
              "url": "https://example.com/h",
              "highlights": ["first part", "second part"]
            }
          ]
        }
        "#;
        let results = extract_exa_results(highlights);
        assert_eq!(results[0].snippet, "first part … second part");

        let text_only = r#"
        {
          "results": [
            {
              "title": "Text Only",
              "url": "https://example.com/t",
              "text": "Body text used as the snippet fallback."
            }
          ]
        }
        "#;
        let results = extract_exa_results(text_only);
        assert_eq!(
            results[0].snippet,
            "Body text used as the snippet fallback."
        );
    }

    #[test]
    fn exa_results_empty_on_malformed_json() {
        assert!(extract_exa_results("nope").is_empty());
        assert!(extract_exa_results(r#"{"results":"wrong"}"#).is_empty());
    }
}
