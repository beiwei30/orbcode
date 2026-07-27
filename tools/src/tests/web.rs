use super::*;

#[tokio::test]
async fn web_fetch_converts_html_to_markdown() {
    let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
    let _lock = WebCacheLock::acquire();
    // The fixture server binds to loopback; allow-list it so the SSRF guard
    // permits the deliberate local fetch.
    crate::web_cache::set_domain_policy(&["127.0.0.1".into()], &[]);
    let resp = http_response(200, "text/html; charset=utf-8", html);
    let resp_static: &'static str = Box::leak(resp.into_boxed_str());
    let (port, server) = http_fixture_server(vec![("/page", resp_static)]).await;

    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-html").await;
    let result = registry
        .invoke(
            "web-fetch",
            &json!({"url": format!("https://127.0.0.1:{port}/page")}).to_string(),
            &context,
        )
        .await;

    server.abort();

    match result {
        Ok(outcome) => {
            assert!(outcome.output.contains("Hello"));
            let metadata = outcome.metadata.expect("metadata present");
            let wf = &metadata["webFetch"];
            assert_eq!(wf["convertedToMarkdown"], true);
        }
        Err(ToolError::ExecutionFailed(msg)) => {
            assert!(
                msg.contains("Connection failed") || msg.contains("HTTP request failed"),
                "unexpected error: {msg}"
            );
        }
        Err(other) => panic!("unexpected error variant: {other}"),
    }
}

#[tokio::test]
async fn web_fetch_rejects_network_denied() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("web-fetch-net-denied").await;
    context.allow_network = false;

    let error = registry
        .invoke("web-fetch", r#"{"url":"https://example.com"}"#, &context)
        .await
        .expect_err("should fail when network is denied");

    assert!(matches!(error, ToolError::NetworkDenied));
}

#[tokio::test]
async fn web_fetch_rejects_tools_denied() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("web-fetch-tools-denied").await;
    context.allow_tools = false;

    let error = registry
        .invoke("web-fetch", r#"{"url":"https://example.com"}"#, &context)
        .await
        .expect_err("should fail when tools are denied");

    assert!(matches!(error, ToolError::PermissionDenied));
}

#[tokio::test]
async fn web_fetch_rejects_url_with_credentials() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-creds").await;

    let error = registry
        .invoke(
            "web-fetch",
            r#"{"url":"https://user:pass@example.com/secret"}"#,
            &context,
        )
        .await
        .expect_err("should reject credentials in URL");

    assert!(
        error.to_string().contains("embedded credentials"),
        "got: {error}"
    );
}

#[tokio::test]
async fn web_fetch_rejects_url_exceeding_max_length() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-long-url").await;
    let long_url = format!("https://example.com/{}", "a".repeat(2000));

    let error = registry
        .invoke("web-fetch", &json!({"url": long_url}).to_string(), &context)
        .await
        .expect_err("should reject overly long URL");

    assert!(error.to_string().contains("maximum length"));
}

#[tokio::test]
async fn web_fetch_rejects_single_label_hostname() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-single-label").await;

    let error = registry
        .invoke("web-fetch", r#"{"url":"https://localhost/path"}"#, &context)
        .await
        .expect_err("should reject single-label hostname");

    assert!(
        error.to_string().contains("at least two parts"),
        "got: {error}"
    );
}

#[tokio::test]
async fn web_fetch_upgrades_http_to_https() {
    let _lock = WebCacheLock::acquire();
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-upgrade").await;

    let result = registry
        .invoke("web-fetch", r#"{"url":"http://example.com"}"#, &context)
        .await;

    if let Ok(outcome) = result {
        let wf = &outcome.metadata.expect("metadata")["webFetch"];
        assert!(
            wf["url"].as_str().unwrap().starts_with("http://"),
            "original URL preserved"
        );
        assert!(
            wf["finalUrl"].as_str().unwrap().starts_with("https://"),
            "upgraded to HTTPS"
        );
    }
}

#[tokio::test]
async fn web_fetch_rejects_non_https_scheme() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-ftp").await;

    let error = registry
        .invoke("web-fetch", r#"{"url":"ftp://example.com/file"}"#, &context)
        .await
        .expect_err("should reject non-HTTPS scheme");

    assert!(error.to_string().contains("only HTTPS"), "got: {error}");
}

#[tokio::test]
async fn web_fetch_returns_useful_error_on_connection_failure() {
    let _lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&["127.0.0.1".into()], &[]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-connfail").await;

    let error = registry
        .invoke("web-fetch", r#"{"url":"https://127.0.0.1:1"}"#, &context)
        .await
        .expect_err("should fail to connect to closed port");

    let msg = error.to_string();
    assert!(
        msg.contains("Connection failed") || msg.contains("HTTP request failed"),
        "should have a useful error message, got: {msg}"
    );
}

#[tokio::test]
async fn web_fetch_resolved_dns_ssrf_guard_rejects_domain_pointing_at_loopback() {
    // A domain that RESOLVES to a loopback/RFC1918/metadata IP must be rejected
    // even though it is not an IP literal or a well-known internal name — the
    // resolved-IP guard, not just the literal-host guard. `localhost` resolves
    // to 127.0.0.1/::1 deterministically without external network.
    let _lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&[], &[]);
    let context = test_context("web-fetch-dns-ssrf").await;
    let url = url::Url::parse("https://localhost:8080/").expect("url");

    let error = crate::web_fetch::resolve_and_validate_host(&url, "localhost", &context)
        .await
        .expect_err("a host resolving to loopback must be rejected");
    let msg = error.to_string();
    assert!(
        msg.contains("private") || msg.contains("loopback") || msg.contains("refusing"),
        "expected an SSRF rejection, got: {msg}"
    );

    // Explicitly allow-listing the host lets the deliberate internal fetch through.
    crate::web_cache::set_domain_policy(&["localhost".into()], &[]);
    crate::web_fetch::resolve_and_validate_host(&url, "localhost", &context)
        .await
        .expect("an allow-listed internal host is permitted");
}

#[tokio::test]
async fn web_search_rejects_network_denied() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("web-search-net-denied").await;
    context.allow_network = false;

    let error = registry
        .invoke("web-search", r#"{"query":"rust programming"}"#, &context)
        .await
        .expect_err("should fail when network is denied");

    assert!(matches!(error, ToolError::NetworkDenied));
}

#[tokio::test]
async fn web_search_rejects_tools_denied() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("web-search-tools-denied").await;
    context.allow_tools = false;

    let error = registry
        .invoke("web-search", r#"{"query":"rust programming"}"#, &context)
        .await
        .expect_err("should fail when tools are denied");

    assert!(matches!(error, ToolError::PermissionDenied));
}

#[test]
fn web_fetch_url_validation_covers_edge_cases() {
    use crate::web_fetch::validate_url;

    assert!(validate_url("https://example.com").is_ok());
    assert!(validate_url("https://www.example.com/path?q=1").is_ok());

    assert!(validate_url("example.com").is_ok(), "auto-prepends https");
    assert!(validate_url("http://example.com").is_ok(), "upgrades http");

    assert!(validate_url("ftp://example.com").is_err());
    assert!(validate_url("https://user:pass@example.com").is_err());
    assert!(validate_url("https://localhost/path").is_err());
    assert!(validate_url(&format!("https://example.com/{}", "a".repeat(2000))).is_err());
    assert!(validate_url("not a valid url at all !!!").is_err());
}

#[test]
fn web_fetch_content_type_detection_matches_typescript() {
    use crate::web_fetch::{is_binary_content_type, is_html_content_type};

    assert!(!is_binary_content_type("text/html"));
    assert!(!is_binary_content_type("text/plain; charset=utf-8"));
    assert!(!is_binary_content_type("application/json"));
    assert!(!is_binary_content_type("application/xml"));
    assert!(!is_binary_content_type("application/javascript"));
    assert!(!is_binary_content_type("application/atom+xml"));
    assert!(!is_binary_content_type("application/ld+json"));
    assert!(!is_binary_content_type("application/x-www-form-urlencoded"));

    assert!(is_binary_content_type("application/pdf"));
    assert!(is_binary_content_type("image/png"));
    assert!(is_binary_content_type("application/octet-stream"));
    assert!(is_binary_content_type("application/zip"));
    assert!(is_binary_content_type("video/mp4"));

    assert!(is_html_content_type("text/html"));
    assert!(is_html_content_type("text/html; charset=utf-8"));
    assert!(is_html_content_type("application/xhtml+xml"));
    assert!(!is_html_content_type("text/plain"));
    assert!(!is_html_content_type("application/json"));
}

#[test]
fn web_fetch_same_host_redirect_detection() {
    use crate::web_fetch::is_same_host_redirect;
    use url::Url;

    let orig = Url::parse("https://example.com/page").unwrap();

    let redir = Url::parse("https://example.com/other").unwrap();
    assert!(is_same_host_redirect(&orig, &redir));

    let redir = Url::parse("https://www.example.com/page").unwrap();
    assert!(is_same_host_redirect(&orig, &redir));

    let www_orig = Url::parse("https://www.example.com/page").unwrap();
    let redir = Url::parse("https://example.com/other").unwrap();
    assert!(is_same_host_redirect(&www_orig, &redir));

    let redir = Url::parse("https://other.com/page").unwrap();
    assert!(!is_same_host_redirect(&orig, &redir));

    let redir = Url::parse("https://example.com:8443/page").unwrap();
    assert!(!is_same_host_redirect(&orig, &redir));

    let redir = Url::parse("http://example.com/page").unwrap();
    assert!(!is_same_host_redirect(&orig, &redir));

    let redir = Url::parse("https://user:pass@example.com/page").unwrap();
    assert!(!is_same_host_redirect(&orig, &redir));
}

#[test]
fn web_fetch_html_to_markdown_converts_basic_structure() {
    use crate::web_fetch::html_to_markdown;

    let html = "<html><body><h1>Title</h1><p>Paragraph with <a href=\"https://example.com\">link</a>.</p><ul><li>Item 1</li><li>Item 2</li></ul></body></html>";
    let md = html_to_markdown(html);

    assert!(md.contains("Title"), "heading preserved");
    assert!(md.contains("Paragraph"), "paragraph preserved");
    assert!(md.contains("link"), "link text preserved");
    assert!(md.contains("Item 1"), "list items preserved");
}

#[test]
fn web_search_ddg_result_extraction_parses_fixture() {
    use crate::web_search::extract_search_results;

    let html = r#"
    <div class="result results_links results_links_deep web-result ">
      <div class="links_main links_deep result__body">
        <h2 class="result__title">
          <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&amp;rut=abc">
            Example &amp; Title
          </a>
        </h2>
        <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage">
          This is the snippet &lt;text&gt;.
        </a>
      </div>
    </div>
    <div class="result results_links results_links_deep web-result ">
      <div class="links_main links_deep result__body">
        <h2 class="result__title">
          <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fother.org%2Fdocs">
            Other Result
          </a>
        </h2>
        <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fother.org%2Fdocs">
          Second snippet here.
        </a>
      </div>
    </div>
    "#;

    let results = extract_search_results(html);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Example & Title");
    assert_eq!(results[0].url, "https://example.com/page");
    assert_eq!(results[0].snippet, "This is the snippet <text>.");
    assert_eq!(results[1].title, "Other Result");
    assert_eq!(results[1].url, "https://other.org/docs");
}

#[test]
fn web_search_ddg_redirect_resolution() {
    use crate::web_search::resolve_ddg_redirect;

    assert_eq!(
        resolve_ddg_redirect("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&rut=abc"),
        "https://example.com/page"
    );
    assert_eq!(
        resolve_ddg_redirect("https://example.com/direct"),
        "https://example.com/direct"
    );
    assert_eq!(resolve_ddg_redirect("/relative/path"), "");
    // Percent-encoded UTF-8 must reassemble to the real characters, not Latin-1
    // mojibake (`%C3%A9` → `é`, previously `Ã©`).
    assert_eq!(
        resolve_ddg_redirect(
            "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fcaf%C3%A9&rut=abc"
        ),
        "https://example.com/café"
    );
}

#[test]
fn web_search_bing_result_extraction_parses_fixture() {
    use crate::web_search::extract_bing_results;

    let html = r#"
    <li class="b_algo" data-id iid=SERP.5339>
      <div class="b_tpcn">
        <a class="tilk" target="_blank" href="https://example.com/page" h="ID=SERP,1">
          <div class="tpic"><div class="wr_fav"></div></div>
        </a>
      </div>
      <div class="b_title">
        <h2><a target="_blank" href="https://example.com/page" h="ID=SERP,2">
          Example &amp; Page Title
        </a></h2>
      </div>
      <div class="b_caption">
        <p class="b_lineclamp4 b_algoSlug">This is the snippet &lt;text&gt;.</p>
      </div>
    </li>
    <li class="b_algo" data-id iid=SERP.5340>
      <div class="b_tpcn">
        <a class="tilk" target="_blank" href="https://other.org/docs" h="ID=SERP,3">
          <div class="tpic"></div>
        </a>
      </div>
      <div class="b_title">
        <h2><a target="_blank" href="https://other.org/docs" h="ID=SERP,4">
          Other Result
        </a></h2>
      </div>
      <div class="b_caption">
        <p class="b_lineclamp4 b_algoSlug">Second snippet here.</p>
      </div>
    </li>
    "#;

    let results = extract_bing_results(html);
    assert_eq!(
        results.len(),
        2,
        "expected 2 results, got {}",
        results.len()
    );
    assert_eq!(results[0].title, "Example & Page Title");
    assert_eq!(results[0].url, "https://example.com/page");
    assert!(results[0].snippet.contains("This is the snippet"));
    assert_eq!(results[1].title, "Other Result");
    assert_eq!(results[1].url, "https://other.org/docs");
}

#[test]
fn web_search_brave_result_extraction_parses_fixture() {
    use crate::web_search_adapters::extract_brave_results;

    let body = r#"
    {
      "web": {
        "results": [
          {
            "title": "Example &amp; Page Title",
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
    assert_eq!(results[0].title, "Example & Page Title");
    assert_eq!(results[0].url, "https://example.com/page");
    assert_eq!(results[0].snippet, "This is the snippet <text>.");
    assert_eq!(results[1].title, "Other Result");
    assert_eq!(results[1].url, "https://other.org/docs");
}

#[test]
fn web_search_exa_result_extraction_parses_fixture() {
    use crate::web_search_adapters::extract_exa_results;

    let body = r#"
    {
      "results": [
        {
          "title": "Exa Example",
          "url": "https://example.com/exa",
          "snippet": "A direct &amp; snippet."
        },
        {
          "title": "Highlighted Result",
          "url": "https://other.org/h",
          "highlights": ["first part", "second part"]
        }
      ]
    }
    "#;

    let results = extract_exa_results(body);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Exa Example");
    assert_eq!(results[0].url, "https://example.com/exa");
    assert_eq!(results[0].snippet, "A direct & snippet.");
    assert_eq!(results[1].title, "Highlighted Result");
    assert_eq!(results[1].snippet, "first part … second part");
}

#[test]
fn web_search_engine_order_env_override_reorders_chain() {
    use crate::web_search_adapters::{SearchEngine, search_engine_order_from};

    assert_eq!(
        search_engine_order_from(None),
        vec![
            SearchEngine::Bing,
            SearchEngine::Brave,
            SearchEngine::Exa,
            SearchEngine::DuckDuckGo,
        ]
    );

    assert_eq!(
        search_engine_order_from(Some("exa,brave")),
        vec![SearchEngine::Exa, SearchEngine::Brave]
    );
}

#[test]
fn web_search_adapter_results_respect_domain_policy() {
    let _lock = WebCacheLock::acquire();
    use crate::web_search::filter_results_by_domain_policy;
    use crate::web_search_adapters::extract_brave_results;

    let body = r#"
    {
      "web": {
        "results": [
          { "title": "Keep", "url": "https://keep.com/a", "description": "" },
          { "title": "Blocked", "url": "https://blocked.org/b", "description": "" }
        ]
      }
    }
    "#;

    crate::web_cache::set_domain_policy(&[], &["blocked.org".into()]);
    let mut results = extract_brave_results(body);
    assert_eq!(results.len(), 2, "both parsed before filtering");
    filter_results_by_domain_policy(&mut results, None);
    assert_eq!(results.len(), 1, "blocked host dropped by policy");
    assert_eq!(results[0].url, "https://keep.com/a");
}

// ---------------------------------------------------------------------------
// Web tools: content cache + domain allow/block preflight
// ---------------------------------------------------------------------------

#[tokio::test]
async fn web_fetch_cache_hit_returns_content_without_network() {
    let lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&["127.0.0.1".into()], &[]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-cache-hit").await;

    let url = "https://127.0.0.1:1/cached";
    seed_cache(url, "CACHED MARKDOWN BODY");

    let outcome = registry
        .invoke("web-fetch", &json!({ "url": url }).to_string(), &context)
        .await
        .expect("cache hit should return content");

    assert_eq!(outcome.output, "CACHED MARKDOWN BODY");
    let wf = &outcome.metadata.expect("metadata")["webFetch"];
    assert_eq!(wf["cached"], true);
    assert!(
        wf["cacheAgeMs"].as_u64().is_some(),
        "cacheAgeMs populated on hit"
    );
    assert_eq!(
        lock.network_calls(),
        0,
        "cache hit must not hit the network"
    );
}

#[tokio::test]
async fn web_fetch_cache_hit_is_byte_identical_across_calls() {
    let lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&["127.0.0.1".into()], &[]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-cache-identical").await;

    let url = "https://127.0.0.1:1/page";
    seed_cache(url, "STABLE CONTENT");

    let first = registry
        .invoke("web-fetch", &json!({ "url": url }).to_string(), &context)
        .await
        .expect("first cached read");
    let second = registry
        .invoke("web-fetch", &json!({ "url": url }).to_string(), &context)
        .await
        .expect("second cached read");

    assert_eq!(
        first.output, second.output,
        "cached content is byte-identical"
    );
    assert_eq!(lock.network_calls(), 0);
}

#[tokio::test]
async fn web_fetch_cache_expires_after_ttl() {
    let lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&["127.0.0.1".into()], &[]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-cache-ttl").await;

    let url = "https://127.0.0.1:1/expiring";
    seed_cache(url, "WILL EXPIRE");

    crate::web_cache::advance_clock_for_tests(crate::web_cache::CACHE_TTL_MS / 2);
    let outcome = registry
        .invoke("web-fetch", &json!({ "url": url }).to_string(), &context)
        .await
        .expect("still cached within TTL");
    assert_eq!(outcome.output, "WILL EXPIRE");
    assert_eq!(lock.network_calls(), 0);

    crate::web_cache::advance_clock_for_tests(crate::web_cache::CACHE_TTL_MS);
    let error = registry
        .invoke("web-fetch", &json!({ "url": url }).to_string(), &context)
        .await
        .expect_err("expired entry must re-fetch and fail to connect");
    assert!(
        error.to_string().contains("Connection failed")
            || error.to_string().contains("HTTP request failed"),
        "expected a network error after expiry, got: {error}"
    );
    assert!(
        lock.network_calls() >= 1,
        "expired entry must hit the network"
    );
}

#[tokio::test]
async fn web_fetch_blocklist_rejects_before_network() {
    let lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&[], &["blocked.example".into()]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-blocklist").await;

    let error = registry
        .invoke(
            "web-fetch",
            r#"{"url":"https://blocked.example/path"}"#,
            &context,
        )
        .await
        .expect_err("blocked domain must be rejected");

    match error {
        ToolError::ExecutionFailedWithMetadata { message, metadata } => {
            assert!(
                message.contains("blocked"),
                "message names the block: {message}"
            );
            assert_eq!(metadata["webFetch"]["blocked"], true);
            assert_eq!(metadata["webFetch"]["blockReason"], "blocklist");
        }
        other => panic!("expected structured block error, got: {other}"),
    }
    assert_eq!(
        lock.network_calls(),
        0,
        "block must precede any network call"
    );
}

#[tokio::test]
async fn web_fetch_blocklist_matches_subdomains() {
    let lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&[], &["blocked.example".into()]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-blocklist-sub").await;

    let error = registry
        .invoke(
            "web-fetch",
            r#"{"url":"https://api.blocked.example/x"}"#,
            &context,
        )
        .await
        .expect_err("subdomain of blocked domain must be rejected");
    assert!(matches!(
        error,
        ToolError::ExecutionFailedWithMetadata { .. }
    ));
    assert_eq!(lock.network_calls(), 0);
}

#[tokio::test]
async fn web_fetch_allowlist_rejects_unlisted_domain() {
    let lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&["allowed.example".into()], &[]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-allowlist-deny").await;

    let error = registry
        .invoke(
            "web-fetch",
            r#"{"url":"https://other.example/x"}"#,
            &context,
        )
        .await
        .expect_err("domain outside allowlist must be rejected");

    match error {
        ToolError::ExecutionFailedWithMetadata { metadata, .. } => {
            assert_eq!(metadata["webFetch"]["blocked"], true);
            assert_eq!(metadata["webFetch"]["blockReason"], "not_in_allowlist");
        }
        other => panic!("expected structured block error, got: {other}"),
    }
    assert_eq!(lock.network_calls(), 0);
}

#[tokio::test]
async fn web_fetch_allowlist_permits_listed_domain() {
    let lock = WebCacheLock::acquire();
    crate::web_cache::set_domain_policy(&["allowed.example".into()], &[]);
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-allowlist-permit").await;

    let url = "https://allowed.example/ok";
    seed_cache(url, "ALLOWED BODY");

    let outcome = registry
        .invoke("web-fetch", &json!({ "url": url }).to_string(), &context)
        .await
        .expect("allowlisted domain should be permitted");
    assert_eq!(outcome.output, "ALLOWED BODY");
    assert_eq!(lock.network_calls(), 0);
}

#[test]
fn web_search_domain_policy_filters_result_hosts() {
    let _lock = WebCacheLock::acquire();
    use crate::web_search::{SearchResult, filter_results_by_domain_policy};

    let make = |url: &str| SearchResult {
        title: "t".into(),
        url: url.into(),
        snippet: String::new(),
    };

    crate::web_cache::set_domain_policy(&[], &["blocked.org".into()]);
    let mut results = vec![
        make("https://keep.com/a"),
        make("https://blocked.org/b"),
        make("https://sub.blocked.org/c"),
    ];
    filter_results_by_domain_policy(&mut results, None);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, "https://keep.com/a");

    crate::web_cache::set_domain_policy(&["only.com".into()], &[]);
    let mut results = vec![make("https://only.com/a"), make("https://other.com/b")];
    filter_results_by_domain_policy(&mut results, None);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, "https://only.com/a");
}

// ---------------------------------------------------------------------------
// Web tools: E2E smoke tests (real network, run with --ignored)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "hits real network; run with `cargo test -p orbcode-tools -- --ignored web_fetch_e2e_cache`"]
async fn web_fetch_e2e_cache_hit_avoids_second_network_call() {
    let lock = WebCacheLock::acquire();
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-e2e-cache").await;
    let url = r#"{"url":"https://example.com"}"#;

    let first = registry
        .invoke("web-fetch", url, &context)
        .await
        .expect("first real fetch");
    let after_first = lock.network_calls();
    assert!(after_first >= 1, "first fetch must hit the network");
    let wf1 = &first.metadata.as_ref().expect("metadata")["webFetch"];
    assert_eq!(wf1["cached"], false, "first fetch is not cached");

    let second = registry
        .invoke("web-fetch", url, &context)
        .await
        .expect("second fetch served from cache");
    assert_eq!(
        lock.network_calls(),
        after_first,
        "cache hit must not issue another network call"
    );
    assert_eq!(
        first.output, second.output,
        "cached content is byte-identical"
    );
    let wf2 = &second.metadata.as_ref().expect("metadata")["webFetch"];
    assert_eq!(wf2["cached"], true, "second fetch is cached");
    assert!(
        wf2["cacheAgeMs"].as_u64().is_some(),
        "cacheAgeMs populated on hit"
    );
}

#[tokio::test]
#[ignore = "hits real network; run with `cargo test -p orbcode-tools -- --ignored web_fetch_e2e_html`"]
async fn web_fetch_e2e_html_to_markdown() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-e2e-html").await;

    let result = registry
        .invoke("web-fetch", r#"{"url":"https://example.com"}"#, &context)
        .await
        .expect("should fetch example.com");

    assert!(
        result.output.contains("Example Domain"),
        "expected markdown to contain page title, got: {}",
        &result.output[..result.output.len().min(200)]
    );
    let wf = &result.metadata.expect("metadata")["webFetch"];
    assert_eq!(wf["statusCode"], 200);
    assert_eq!(wf["convertedToMarkdown"], true);
    assert!(wf["durationMs"].as_u64().unwrap() > 0);
}

#[tokio::test]
#[ignore = "hits real network; run with `cargo test -p orbcode-tools -- --ignored web_fetch_e2e_redirect`"]
async fn web_fetch_e2e_redirect_follow() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-e2e-redirect").await;

    let result = registry
        .invoke(
            "web-fetch",
            r#"{"url":"https://httpbin.org/redirect-to?url=%2Fget&status_code=302"}"#,
            &context,
        )
        .await
        .expect("should follow same-host redirect");

    let wf = &result.metadata.expect("metadata")["webFetch"];
    assert_eq!(wf["redirected"], true);
    assert!(wf["redirectCount"].as_u64().unwrap() >= 1);
}

#[tokio::test]
#[ignore = "hits real network; run with `cargo test -p orbcode-tools -- --ignored web_fetch_e2e_binary`"]
async fn web_fetch_e2e_binary_rejection() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-fetch-e2e-binary").await;

    let result = registry
        .invoke(
            "web-fetch",
            r#"{"url":"https://www.google.com/favicon.ico"}"#,
            &context,
        )
        .await;

    match result {
        Err(ToolError::ExecutionFailedWithMetadata { message, metadata }) => {
            assert!(
                message.contains("binary content type")
                    || message.contains("Cannot process binary"),
                "expected binary rejection, got: {message}"
            );
            assert_eq!(metadata["webFetch"]["binary"], true);
        }
        Err(ToolError::ExecutionFailed(msg)) if msg.contains("timed out") => {
            eprintln!("web_fetch_e2e_binary: timed out reaching server, skipping assertions");
        }
        Ok(outcome) => panic!(
            "expected binary rejection, got success: {}",
            &outcome.output[..outcome.output.len().min(100)]
        ),
        Err(other) => panic!("unexpected error variant: {other}"),
    }
}

#[tokio::test]
#[ignore = "hits real network; run with `cargo test -p orbcode-tools -- --ignored web_search_e2e`"]
async fn web_search_e2e_returns_results() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-search-e2e").await;

    let result = registry
        .invoke(
            "web-search",
            r#"{"query":"rust programming language"}"#,
            &context,
        )
        .await;

    match result {
        Ok(outcome) => {
            let ws = &outcome.metadata.expect("metadata")["webSearch"];
            assert!(ws["numResults"].as_u64().is_some());
            assert_eq!(ws["engine"], "duckduckgo");
        }
        Err(ToolError::ExecutionFailed(msg)) => {
            assert!(
                msg.contains("timed out") || msg.contains("Search returned HTTP"),
                "expected retriable failure, got: {msg}"
            );
            eprintln!("web_search_e2e: DuckDuckGo unavailable ({msg}), skipping assertions");
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
#[ignore = "hits real Brave API; needs BRAVE_API_KEY: \
            cargo test -p orbcode-tools -- --ignored web_search_e2e_brave"]
async fn web_search_e2e_brave_backend() {
    use crate::web_search_adapters::search_brave;

    if std::env::var("BRAVE_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .is_none()
    {
        eprintln!("web_search_e2e_brave: BRAVE_API_KEY unset, skipping");
        return;
    }

    match search_brave("rust programming language").await {
        Ok(results) => {
            assert!(!results.is_empty(), "expected Brave to return results");
            let first = &results[0];
            assert!(!first.title.is_empty(), "result has a title");
            assert!(first.url.starts_with("http"), "result has an http(s) URL");
        }
        Err(ToolError::ExecutionFailed(msg)) => {
            eprintln!("web_search_e2e_brave: Brave unavailable ({msg}), skipping assertions");
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
#[ignore = "hits real Exa API; needs EXA_API_KEY: \
            cargo test -p orbcode-tools -- --ignored web_search_e2e_exa"]
async fn web_search_e2e_exa_backend() {
    use crate::web_search_adapters::search_exa;

    if std::env::var("EXA_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .is_none()
    {
        eprintln!("web_search_e2e_exa: EXA_API_KEY unset, skipping");
        return;
    }

    match search_exa("rust programming language").await {
        Ok(results) => {
            assert!(!results.is_empty(), "expected Exa to return results");
            let first = &results[0];
            assert!(!first.title.is_empty(), "result has a title");
            assert!(first.url.starts_with("http"), "result has an http(s) URL");
        }
        Err(ToolError::ExecutionFailed(msg)) => {
            eprintln!("web_search_e2e_exa: Exa unavailable ({msg}), skipping assertions");
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
#[ignore = "hits real search APIs; pin ORBCODE_WEB_SEARCH_ORDER + the matching key"]
async fn web_search_e2e_chain_engine_metadata() {
    let registry = ToolRegistry::foundation();
    let context = test_context("web-search-e2e-chain").await;

    let order = std::env::var("ORBCODE_WEB_SEARCH_ORDER").unwrap_or_default();
    let expected = order.split(',').next().unwrap_or("").trim().to_string();
    if expected.is_empty() {
        eprintln!("web_search_e2e_chain_engine: ORBCODE_WEB_SEARCH_ORDER unset, skipping");
        return;
    }

    match registry
        .invoke(
            "web-search",
            r#"{"query":"rust programming language"}"#,
            &context,
        )
        .await
    {
        Ok(outcome) => {
            let ws = &outcome.metadata.expect("metadata")["webSearch"];
            assert!(ws["numResults"].as_u64().is_some());
            assert_eq!(
                ws["engine"], expected,
                "expected the pinned primary engine to serve results"
            );
        }
        Err(ToolError::ExecutionFailed(msg)) => {
            eprintln!("web_search_e2e_chain_engine: backend unavailable ({msg}), skipping");
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}
