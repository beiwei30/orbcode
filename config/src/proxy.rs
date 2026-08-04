//! Outbound proxy selection shared by provider and OAuth HTTP clients.
//!
//! Proxy selection is destination-aware and intentionally keeps proxy URLs
//! out of debug output. User settings use the conventional lowercase
//! `http_proxy` / `https_proxy` keys; process environment variables are the
//! next fallback, followed by the host operating system's proxy settings.

use std::collections::BTreeMap;
use std::env;
use std::fmt;

#[cfg(target_os = "macos")]
use std::net::IpAddr;

#[cfg(target_os = "macos")]
use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub struct OutboundProxyConfig {
    settings_http_proxy: Option<String>,
    settings_https_proxy: Option<String>,
    settings_no_proxy: Option<String>,
    process_http_proxy: Option<String>,
    process_https_proxy: Option<String>,
    process_all_proxy: Option<String>,
    process_no_proxy: Option<String>,
    process_legacy_proxy: Option<String>,
}

impl fmt::Debug for OutboundProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutboundProxyConfig")
            .field("settings_http_proxy", &self.settings_http_proxy.is_some())
            .field("settings_https_proxy", &self.settings_https_proxy.is_some())
            .field("settings_no_proxy", &self.settings_no_proxy.is_some())
            .field("process_http_proxy", &self.process_http_proxy.is_some())
            .field("process_https_proxy", &self.process_https_proxy.is_some())
            .field("process_all_proxy", &self.process_all_proxy.is_some())
            .field("process_no_proxy", &self.process_no_proxy.is_some())
            .field("process_legacy_proxy", &self.process_legacy_proxy.is_some())
            .finish()
    }
}

impl Default for OutboundProxyConfig {
    fn default() -> Self {
        Self::from_sources(&BTreeMap::new(), process_env_value)
    }
}

impl OutboundProxyConfig {
    pub(crate) fn from_sources(
        settings_env: &BTreeMap<String, String>,
        process_lookup: impl Fn(&str) -> Option<String>,
    ) -> Self {
        Self {
            // Only the conventional lowercase keys in settings.json are
            // treated as the explicit application-level proxy selection.
            settings_http_proxy: nonempty(settings_env.get("http_proxy").cloned()),
            settings_https_proxy: nonempty(settings_env.get("https_proxy").cloned()),
            settings_no_proxy: nonempty(settings_env.get("no_proxy").cloned()),
            process_http_proxy: first_nonempty([
                process_lookup("HTTP_PROXY"),
                process_lookup("http_proxy"),
            ]),
            process_https_proxy: first_nonempty([
                process_lookup("HTTPS_PROXY"),
                process_lookup("https_proxy"),
            ]),
            process_all_proxy: first_nonempty([
                process_lookup("ALL_PROXY"),
                process_lookup("all_proxy"),
            ]),
            process_no_proxy: first_nonempty([
                process_lookup("NO_PROXY"),
                process_lookup("no_proxy"),
            ]),
            // Keep the old Orb Code / TypeScript-compatible process variables
            // as a lower-priority fallback. They are intentionally not read
            // from settings.json: explicit settings use the conventional
            // lowercase protocol-specific keys above.
            process_legacy_proxy: first_nonempty([
                process_lookup("ORBCODE_PROXY"),
                process_lookup("CLAUDE_CODE_PROXY"),
                process_lookup("ANTHROPIC_PROXY_URL"),
            ]),
        }
    }

    pub fn resolve(&self, request_url: &str) -> OutboundProxyRoute {
        let Ok(url) = Url::parse(request_url) else {
            return OutboundProxyRoute::Direct;
        };
        let Some(host) = url.host_str() else {
            return OutboundProxyRoute::Direct;
        };
        if is_loopback_host(host) {
            return OutboundProxyRoute::Direct;
        }

        let is_secure = matches!(url.scheme(), "https" | "wss");
        let is_plain = matches!(url.scheme(), "http" | "ws");
        if !is_secure && !is_plain {
            return OutboundProxyRoute::Direct;
        }

        let settings_proxy = if is_secure {
            self.settings_https_proxy
                .as_ref()
                .or(self.settings_http_proxy.as_ref())
        } else {
            self.settings_http_proxy.as_ref()
        };
        if let Some(proxy_url) = settings_proxy {
            return OutboundProxyRoute::Proxy {
                url: proxy_url.clone(),
                no_proxy: with_loopback_bypass(
                    self.settings_no_proxy
                        .as_deref()
                        .or(self.process_no_proxy.as_deref()),
                ),
            };
        }

        let process_proxy = if is_secure {
            self.process_https_proxy
                .as_ref()
                .or(self.process_http_proxy.as_ref())
                .or(self.process_all_proxy.as_ref())
        } else {
            self.process_http_proxy
                .as_ref()
                .or(self.process_all_proxy.as_ref())
        };
        if let Some(proxy_url) = process_proxy {
            return OutboundProxyRoute::Proxy {
                url: proxy_url.clone(),
                no_proxy: with_loopback_bypass(self.process_no_proxy.as_deref()),
            };
        }
        if let Some(proxy_url) = &self.process_legacy_proxy {
            return OutboundProxyRoute::Proxy {
                url: proxy_url.clone(),
                no_proxy: with_loopback_bypass(self.process_no_proxy.as_deref()),
            };
        }

        resolve_system_proxy(&url).unwrap_or(OutboundProxyRoute::Direct)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum OutboundProxyRoute {
    Direct,
    Proxy {
        url: String,
        no_proxy: Option<String>,
    },
}

impl fmt::Debug for OutboundProxyRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct => f.write_str("Direct"),
            Self::Proxy { .. } => f
                .debug_struct("Proxy")
                .field("url", &"<redacted>")
                .field("no_proxy", &"<redacted>")
                .finish(),
        }
    }
}

fn process_env_value(key: &str) -> Option<String> {
    env::var(key).ok()
}

fn first_nonempty<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values.into_iter().find_map(nonempty)
}

fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host.starts_with("127.")
        || host == "::1"
}

fn with_loopback_bypass(no_proxy: Option<&str>) -> Option<String> {
    let loopback = "localhost,127.0.0.1,::1";
    Some(
        match no_proxy.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => format!("{value},{loopback}"),
            None => loopback.to_string(),
        },
    )
}

#[cfg(not(target_os = "macos"))]
fn resolve_system_proxy(_url: &Url) -> Option<OutboundProxyRoute> {
    None
}

#[cfg(target_os = "macos")]
fn resolve_system_proxy(url: &Url) -> Option<OutboundProxyRoute> {
    let settings = cached_macos_proxy_settings()?;
    let host = url.host_str()?;
    if settings.bypasses(host) {
        return Some(OutboundProxyRoute::Direct);
    }

    // Static HTTP(S) proxy settings are resolved here. PAC configurations are
    // deliberately not guessed: evaluating JavaScript PAC rules incorrectly
    // is worse than falling back to direct.
    if settings.pac_enabled {
        return Some(OutboundProxyRoute::Direct);
    }
    let proxy = match url.scheme() {
        "https" | "wss" => settings.https_proxy.or(settings.http_proxy),
        "http" | "ws" => settings.http_proxy,
        _ => None,
    }?;
    Some(OutboundProxyRoute::Proxy {
        url: proxy,
        no_proxy: None,
    })
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MacOsProxySettings {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    exceptions: Vec<String>,
    exclude_simple_hostnames: bool,
    pac_enabled: bool,
}

#[cfg(target_os = "macos")]
impl MacOsProxySettings {
    fn bypasses(&self, host: &str) -> bool {
        if is_loopback_host(host) {
            return true;
        }
        if self.exclude_simple_hostnames && !host.contains('.') {
            return true;
        }
        let host = host.to_ascii_lowercase();
        self.exceptions.iter().any(|entry| {
            let entry = entry.trim().to_ascii_lowercase();
            if entry == "<local>" {
                return !host.contains('.');
            }
            if let Some(suffix) = entry.strip_prefix("*.") {
                return host == suffix || host.ends_with(&format!(".{suffix}"));
            }
            if let Some(suffix) = entry.strip_prefix('.') {
                return host == suffix || host.ends_with(&format!(".{suffix}"));
            }
            if ip_cidr_matches(&host, &entry) {
                return true;
            }
            host == entry
        })
    }
}

#[cfg(target_os = "macos")]
fn ip_cidr_matches(host: &str, cidr: &str) -> bool {
    let Some((network, prefix)) = cidr.split_once('/') else {
        return false;
    };
    let Ok(host) = host.parse::<IpAddr>() else {
        return false;
    };
    let Ok(network) = network.parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u32>() else {
        return false;
    };
    match (host, network) {
        (IpAddr::V4(host), IpAddr::V4(network)) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            u32::from(host) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(host), IpAddr::V6(network)) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            u128::from(host) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct CachedMacOsProxySettings {
    value: Option<MacOsProxySettings>,
    expires_at: Instant,
}

#[cfg(target_os = "macos")]
static MACOS_PROXY_CACHE: OnceLock<Mutex<Option<CachedMacOsProxySettings>>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn cached_macos_proxy_settings() -> Option<MacOsProxySettings> {
    let cache = MACOS_PROXY_CACHE.get_or_init(|| Mutex::new(None));
    let mut cache = cache.lock().ok()?;
    let now = Instant::now();
    if let Some(cached) = cache.as_ref()
        && cached.expires_at > now
    {
        return cached.value.clone();
    }

    let value = load_macos_proxy_settings();
    *cache = Some(CachedMacOsProxySettings {
        value: value.clone(),
        expires_at: now + Duration::from_secs(if value.is_some() { 60 } else { 5 }),
    });
    value
}

#[cfg(target_os = "macos")]
fn load_macos_proxy_settings() -> Option<MacOsProxySettings> {
    let output = Command::new("/usr/sbin/scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_scutil_proxy_output(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "macos")]
fn parse_scutil_proxy_output(output: &str) -> Option<MacOsProxySettings> {
    let mut values = HashMap::<String, String>::new();
    let mut exceptions = Vec::new();
    let mut in_exceptions = false;
    for raw_line in output.lines() {
        let line = raw_line.trim();
        if line.starts_with("ExceptionsList : <array>") {
            in_exceptions = true;
            continue;
        }
        if in_exceptions {
            if line == "}" {
                in_exceptions = false;
                continue;
            }
            if let Some((index, value)) = line.split_once(" : ")
                && index.trim().parse::<usize>().is_ok()
            {
                exceptions.push(value.trim().to_string());
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(" : ") {
            values.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    let http_proxy = enabled_proxy(&values, "HTTPEnable", "HTTPProxy", "HTTPPort");
    let https_proxy = enabled_proxy(&values, "HTTPSEnable", "HTTPSProxy", "HTTPSPort");
    Some(MacOsProxySettings {
        http_proxy,
        https_proxy,
        exceptions,
        exclude_simple_hostnames: values
            .get("ExcludeSimpleHostnames")
            .is_some_and(|value| value == "1"),
        pac_enabled: values
            .get("ProxyAutoConfigEnable")
            .is_some_and(|value| value == "1"),
    })
}

#[cfg(target_os = "macos")]
fn enabled_proxy(
    values: &HashMap<String, String>,
    enabled_key: &str,
    host_key: &str,
    port_key: &str,
) -> Option<String> {
    if values.get(enabled_key).map(String::as_str) != Some("1") {
        return None;
    }
    let host = values.get(host_key)?.trim();
    if host.is_empty() {
        return None;
    }
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match values
        .get(port_key)
        .and_then(|value| value.parse::<u16>().ok())
    {
        Some(port) => Some(format!("http://{host}:{port}")),
        None => Some(format!("http://{host}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(settings: &[(&str, &str)], process: &[(&str, &str)]) -> OutboundProxyConfig {
        let settings = settings
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>();
        let process = process
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>();
        OutboundProxyConfig::from_sources(&settings, |key| process.get(key).cloned())
    }

    #[test]
    fn settings_lowercase_proxy_wins_over_process_environment() {
        let route = config(
            &[("https_proxy", "http://settings-proxy:9000")],
            &[("HTTPS_PROXY", "http://process-proxy:8000")],
        )
        .resolve("https://chatgpt.com/backend-api/codex");
        assert!(matches!(
            route,
            OutboundProxyRoute::Proxy { ref url, .. } if url == "http://settings-proxy:9000"
        ));
    }

    #[test]
    fn settings_http_proxy_is_the_https_fallback() {
        let route = config(
            &[("http_proxy", "http://settings-proxy:9000")],
            &[("HTTPS_PROXY", "http://process-proxy:8000")],
        )
        .resolve("https://chatgpt.com/backend-api/codex");
        assert!(matches!(
            route,
            OutboundProxyRoute::Proxy { ref url, .. } if url == "http://settings-proxy:9000"
        ));
    }

    #[test]
    fn process_https_then_http_then_all_proxy_precedence_is_stable() {
        let route = config(
            &[],
            &[
                ("HTTPS_PROXY", "http://https-proxy:8000"),
                ("HTTP_PROXY", "http://http-proxy:8000"),
                ("ALL_PROXY", "http://all-proxy:8000"),
            ],
        )
        .resolve("https://auth.openai.com/oauth/token");
        assert!(matches!(
            route,
            OutboundProxyRoute::Proxy { ref url, .. } if url == "http://https-proxy:8000"
        ));
    }

    #[test]
    fn uppercase_settings_proxy_is_not_an_explicit_proxy_setting() {
        let route = config(
            &[("HTTPS_PROXY", "http://settings-uppercase:9000")],
            &[("HTTPS_PROXY", "http://process-proxy:8000")],
        )
        .resolve("https://auth.openai.com/oauth/token");
        assert!(matches!(
            route,
            OutboundProxyRoute::Proxy { ref url, .. } if url == "http://process-proxy:8000"
        ));
    }

    #[test]
    fn legacy_process_proxy_is_supported_below_standard_process_proxy() {
        let route = config(
            &[],
            &[
                ("HTTPS_PROXY", "http://standard-proxy:8000"),
                ("ORBCODE_PROXY", "http://legacy-proxy:7000"),
            ],
        )
        .resolve("https://chatgpt.com/backend-api/codex");
        assert!(matches!(
            route,
            OutboundProxyRoute::Proxy { ref url, .. } if url == "http://standard-proxy:8000"
        ));

        let legacy_route = config(&[], &[("CLAUDE_CODE_PROXY", "http://legacy-proxy:7000")])
            .resolve("https://chatgpt.com/backend-api/codex");
        assert!(matches!(
            legacy_route,
            OutboundProxyRoute::Proxy { ref url, .. } if url == "http://legacy-proxy:7000"
        ));
    }

    #[test]
    fn loopback_destinations_always_bypass_proxy() {
        let route = config(&[("https_proxy", "http://settings-proxy:9000")], &[])
            .resolve("http://127.0.0.1:1455/auth/callback");
        assert_eq!(route, OutboundProxyRoute::Direct);
    }

    #[test]
    fn proxy_debug_output_redacts_urls() {
        let config = config(
            &[("https_proxy", "http://user:secret@private.proxy:9000")],
            &[],
        );
        let route = config.resolve("https://chatgpt.com");
        let debug = format!("{route:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("private.proxy"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_static_macos_http_proxy_and_exceptions() {
        let parsed = parse_scutil_proxy_output(
            r#"<dictionary> {
  ExceptionsList : <array> {
    0 : 127.0.0.1
    1 : *.local
    2 : <local>
    3 : 192.168.0.0/16
    4 : fd00::/8
  }
  HTTPEnable : 1
  HTTPPort : 7890
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7890
  HTTPSProxy : 127.0.0.1
}"#,
        )
        .expect("settings");
        assert_eq!(parsed.http_proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(parsed.https_proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert!(parsed.bypasses("service.local"));
        assert!(parsed.bypasses("localhost"));
        assert!(parsed.bypasses("192.168.20.3"));
        assert!(parsed.bypasses("fd12::1"));
        assert!(!parsed.bypasses("chatgpt.com"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detects_pac_without_attempting_to_guess_its_route() {
        let parsed = parse_scutil_proxy_output(
            r#"<dictionary> {
  ProxyAutoConfigEnable : 1
  ProxyAutoConfigURLString : http://localhost/proxy.pac
}"#,
        )
        .expect("settings");
        assert!(parsed.pac_enabled);
    }
}
