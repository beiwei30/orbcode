//! Parser for the feature-gated, process-only ChatGPT OAuth test inputs.
//!
//! This module is compiled into `orbcode-config` unit tests so the validation
//! contract is testable without mutating the test runner's environment. It is
//! compiled into runtime code only when `oauth-test-support` is explicitly
//! enabled. None of these names participate in settings or env compatibility.

use std::time::Duration;

use url::{Host, Url};

use crate::openai_oauth::OpenAiOAuthOptions;

pub(crate) const ISSUER_ENV: &str = "ORBCODE_TEST_OPENAI_ISSUER";
pub(crate) const CODEX_BASE_URL_ENV: &str = "ORBCODE_TEST_OPENAI_CODEX_BASE_URL";
pub(crate) const CALLBACK_PORTS_ENV: &str = "ORBCODE_TEST_OPENAI_CALLBACK_PORTS";
pub(crate) const BROWSER_TIMEOUT_MS_ENV: &str = "ORBCODE_TEST_OPENAI_BROWSER_TIMEOUT_MS";
pub(crate) const DEVICE_TIMEOUT_MS_ENV: &str = "ORBCODE_TEST_OPENAI_DEVICE_TIMEOUT_MS";
pub(crate) const ORIGINATOR_ENV: &str = "ORBCODE_TEST_OPENAI_ORIGINATOR";

const REQUIRED_ENV: [&str; 5] = [
    ISSUER_ENV,
    CODEX_BASE_URL_ENV,
    CALLBACK_PORTS_ENV,
    BROWSER_TIMEOUT_MS_ENV,
    DEVICE_TIMEOUT_MS_ENV,
];
const ALL_ENV: [&str; 6] = [
    ISSUER_ENV,
    CODEX_BASE_URL_ENV,
    CALLBACK_PORTS_ENV,
    BROWSER_TIMEOUT_MS_ENV,
    DEVICE_TIMEOUT_MS_ENV,
    ORIGINATOR_ENV,
];
const MAX_CALLBACK_PORTS: usize = 16;
const MIN_TIMEOUT_MS: u64 = 50;
const MAX_TIMEOUT_MS: u64 = 60_000;
const MAX_ORIGINATOR_LEN: usize = 64;

/// Load the exact test-only variables from the process. Error text names the
/// invalid input but deliberately never includes its value.
#[cfg(feature = "oauth-test-support")]
pub(crate) fn from_process_env() -> Result<Option<OpenAiOAuthOptions>, String> {
    from_values(|key| std::env::var(key).ok())
}

fn from_values(
    mut value: impl FnMut(&str) -> Option<String>,
) -> Result<Option<OpenAiOAuthOptions>, String> {
    let values = ALL_ENV.map(|key| (key, value(key).filter(|value| !value.trim().is_empty())));
    if values.iter().all(|(_, value)| value.is_none()) {
        return Ok(None);
    }

    for key in REQUIRED_ENV {
        if values
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .and_then(|(_, value)| value.as_ref())
            .is_none()
        {
            return Err(format!(
                "incomplete ChatGPT OAuth test configuration: {key} is required"
            ));
        }
    }

    let get = |key| {
        values
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .and_then(|(_, value)| value.as_deref())
            .expect("required test input was checked above")
    };
    let defaults = OpenAiOAuthOptions::default();
    let originator = if let Some(originator) = values
        .iter()
        .find(|(key, _)| *key == ORIGINATOR_ENV)
        .and_then(|(_, value)| value.as_deref())
    {
        parse_originator(originator)?
    } else {
        defaults.originator.clone()
    };
    Ok(Some(OpenAiOAuthOptions {
        issuer: parse_loopback_endpoint(ISSUER_ENV, get(ISSUER_ENV))?,
        codex_base_url: parse_loopback_endpoint(CODEX_BASE_URL_ENV, get(CODEX_BASE_URL_ENV))?,
        originator,
        callback_ports: parse_callback_ports(get(CALLBACK_PORTS_ENV))?,
        browser_timeout: parse_timeout(BROWSER_TIMEOUT_MS_ENV, get(BROWSER_TIMEOUT_MS_ENV))?,
        device_timeout: parse_timeout(DEVICE_TIMEOUT_MS_ENV, get(DEVICE_TIMEOUT_MS_ENV))?,
        ..defaults
    }))
}

fn parse_loopback_endpoint(name: &str, value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| invalid(name, "must be a valid URL"))?;
    if url.scheme() != "http" {
        return Err(invalid(
            name,
            "must use plain HTTP for a local test service",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(name, "must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid(name, "must not contain a query string or fragment"));
    }
    if url.port().is_none() {
        return Err(invalid(name, "must include an explicit port"));
    }
    let loopback = matches!(url.host(), Some(Host::Ipv4(address)) if address.is_loopback() && address.octets() == [127, 0, 0, 1])
        || matches!(url.host(), Some(Host::Domain(host)) if host.eq_ignore_ascii_case("localhost"));
    if !loopback {
        return Err(invalid(name, "must use 127.0.0.1 or localhost"));
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn parse_callback_ports(value: &str) -> Result<Vec<u16>, String> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.is_empty()
        || parts.len() > MAX_CALLBACK_PORTS
        || parts.iter().any(|part| part.is_empty())
    {
        return Err(invalid(
            CALLBACK_PORTS_ENV,
            "must contain 1 to 16 comma-separated ports",
        ));
    }
    let mut ports = Vec::with_capacity(parts.len());
    for part in parts {
        let port = part
            .parse::<u16>()
            .map_err(|_| invalid(CALLBACK_PORTS_ENV, "contains an invalid port"))?;
        if ports.contains(&port) {
            return Err(invalid(CALLBACK_PORTS_ENV, "contains a duplicate port"));
        }
        ports.push(port);
    }
    Ok(ports)
}

fn parse_timeout(name: &str, value: &str) -> Result<Duration, String> {
    let milliseconds = value
        .parse::<u64>()
        .map_err(|_| invalid(name, "must be an integer number of milliseconds"))?;
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&milliseconds) {
        return Err(invalid(name, "must be between 50 and 60000 milliseconds"));
    }
    Ok(Duration::from_millis(milliseconds))
}

fn parse_originator(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.len() > MAX_ORIGINATOR_LEN
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid(
            ORIGINATOR_ENV,
            "must be 1 to 64 ASCII letters, digits, dots, dashes, or underscores",
        ));
    }
    Ok(value.to_string())
}

fn invalid(name: &str, reason: &str) -> String {
    format!("invalid ChatGPT OAuth test configuration in {name}: {reason}")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn valid_values() -> HashMap<&'static str, String> {
        HashMap::from([
            (ISSUER_ENV, "http://127.0.0.1:4010/oauth".to_string()),
            (
                CODEX_BASE_URL_ENV,
                "http://localhost:4011/backend-api/codex".to_string(),
            ),
            (CALLBACK_PORTS_ENV, "0,1455".to_string()),
            (BROWSER_TIMEOUT_MS_ENV, "2500".to_string()),
            (DEVICE_TIMEOUT_MS_ENV, "3500".to_string()),
            (ORIGINATOR_ENV, "orbcode-test.v1".to_string()),
        ])
    }

    fn parse(values: &HashMap<&str, String>) -> Result<Option<OpenAiOAuthOptions>, String> {
        from_values(|key| values.get(key).cloned())
    }

    #[test]
    fn oauth_test_support_accepts_only_bounded_loopback_inputs() {
        let options = parse(&valid_values())
            .expect("valid options")
            .expect("present");
        assert_eq!(options.issuer, "http://127.0.0.1:4010/oauth");
        assert_eq!(
            options.codex_base_url,
            "http://localhost:4011/backend-api/codex"
        );
        assert_eq!(options.callback_ports, vec![0, 1455]);
        assert_eq!(options.browser_timeout, Duration::from_millis(2500));
        assert_eq!(options.device_timeout, Duration::from_millis(3500));
        assert_eq!(options.originator, "orbcode-test.v1");
    }

    #[test]
    fn oauth_test_support_leaves_production_defaults_when_absent() {
        let values = HashMap::new();
        assert!(parse(&values).expect("empty inputs").is_none());
    }

    #[test]
    fn oauth_test_support_rejects_non_loopback_and_url_smuggling_without_echoing_values() {
        for invalid_issuer in [
            "https://127.0.0.1:4010",
            "http://example.com:4010",
            "http://127.0.0.1.evil:4010",
            "http://user:password@127.0.0.1:4010",
            "http://127.0.0.1:4010?secret=canary",
            "http://127.0.0.1:4010/#canary",
            "http://127.0.0.1",
        ] {
            let mut values = valid_values();
            values.insert(ISSUER_ENV, invalid_issuer.to_string());
            let error = parse(&values).expect_err("issuer must be rejected");
            assert!(error.contains(ISSUER_ENV), "{error}");
            assert!(
                !error.contains(invalid_issuer),
                "must not echo input: {error}"
            );
            assert!(
                !error.contains("canary"),
                "must not leak query data: {error}"
            );
        }

        let invalid_codex = "https://localhost:4011/codex?secret=canary";
        let mut values = valid_values();
        values.insert(CODEX_BASE_URL_ENV, invalid_codex.to_string());
        let error = parse(&values).expect_err("Codex base URL must be rejected");
        assert!(error.contains(CODEX_BASE_URL_ENV), "{error}");
        assert!(
            !error.contains(invalid_codex),
            "must not echo input: {error}"
        );
        assert!(
            !error.contains("canary"),
            "must not leak query data: {error}"
        );
    }

    #[test]
    fn oauth_test_support_rejects_incomplete_ports_timeouts_and_originator() {
        let cases = [
            (CALLBACK_PORTS_ENV, "0,0"),
            (CALLBACK_PORTS_ENV, "65536"),
            (BROWSER_TIMEOUT_MS_ENV, "49"),
            (DEVICE_TIMEOUT_MS_ENV, "60001"),
            (ORIGINATOR_ENV, "has spaces"),
        ];
        for (name, bad_value) in cases {
            let mut values = valid_values();
            values.insert(name, bad_value.to_string());
            let error = parse(&values).expect_err("value must be rejected");
            assert!(error.contains(name), "{error}");
            assert!(!error.contains(bad_value), "must not echo input: {error}");
        }

        let mut incomplete = valid_values();
        incomplete.remove(CODEX_BASE_URL_ENV);
        let error = parse(&incomplete).expect_err("missing required input");
        assert!(error.contains(CODEX_BASE_URL_ENV), "{error}");
    }
}
