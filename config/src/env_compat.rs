/// Environment variable alias table for TypeScript CLI compatibility.
///
/// Maps canonical `ORBCODE_*` keys to their legacy (TypeScript-era) aliases.
/// When the resolver is asked for any key in an alias group, it checks all
/// keys in canonical-first order so both old and new names work, with the
/// canonical name taking priority.
///
/// Resolution precedence (per layer):
///   canonical process env > legacy process env > canonical settings env > legacy settings env
struct AliasGroup {
    canonical: &'static str,
    legacy: &'static [&'static str],
}

const ALIASES: &[AliasGroup] = &[
    // ── Provider / base URL ────────────────────────────────────────────
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_BASE_URL",
        legacy: &["ANTHROPIC_BASE_URL"],
    },
    AliasGroup {
        canonical: "ORBCODE_OPENAI_BASE_URL",
        legacy: &["OPENAI_BASE_URL"],
    },
    // ── Auth keys ──────────────────────────────────────────────────────
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_API_KEY",
        legacy: &["ANTHROPIC_API_KEY"],
    },
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_AUTH_TOKEN",
        legacy: &["ANTHROPIC_AUTH_TOKEN"],
    },
    AliasGroup {
        canonical: "ORBCODE_OAUTH_TOKEN",
        legacy: &["CLAUDE_CODE_OAUTH_TOKEN"],
    },
    AliasGroup {
        canonical: "ORBCODE_OPENAI_API_KEY",
        legacy: &["OPENAI_API_KEY"],
    },
    AliasGroup {
        canonical: "ORBCODE_GEMINI_API_KEY",
        legacy: &["GEMINI_API_KEY"],
    },
    AliasGroup {
        canonical: "ORBCODE_XAI_API_KEY",
        legacy: &["XAI_API_KEY"],
    },
    AliasGroup {
        canonical: "ORBCODE_GROK_API_KEY",
        legacy: &["GROK_API_KEY"],
    },
    // ── Model selection ────────────────────────────────────────────────
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_MODEL",
        legacy: &["ANTHROPIC_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_OPENAI_MODEL",
        legacy: &["OPENAI_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_SMALL_FAST_MODEL",
        legacy: &["ANTHROPIC_SMALL_FAST_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_OPENAI_SMALL_FAST_MODEL",
        legacy: &["OPENAI_SMALL_FAST_MODEL"],
    },
    // ── Model family defaults ──────────────────────────────────────────
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_DEFAULT_OPUS_MODEL",
        legacy: &["ANTHROPIC_DEFAULT_OPUS_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_DEFAULT_SONNET_MODEL",
        legacy: &["ANTHROPIC_DEFAULT_SONNET_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_DEFAULT_HAIKU_MODEL",
        legacy: &["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_OPENAI_DEFAULT_OPUS_MODEL",
        legacy: &["OPENAI_DEFAULT_OPUS_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_OPENAI_DEFAULT_SONNET_MODEL",
        legacy: &["OPENAI_DEFAULT_SONNET_MODEL"],
    },
    AliasGroup {
        canonical: "ORBCODE_OPENAI_DEFAULT_HAIKU_MODEL",
        legacy: &["OPENAI_DEFAULT_HAIKU_MODEL"],
    },
    // ── Custom model option ────────────────────────────────────────────
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_CUSTOM_MODEL_OPTION",
        legacy: &["ANTHROPIC_CUSTOM_MODEL_OPTION"],
    },
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        legacy: &["ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"],
    },
    AliasGroup {
        canonical: "ORBCODE_ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        legacy: &["ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"],
    },
    // ── Request / context / budget options ─────────────────────────────
    AliasGroup {
        canonical: "ORBCODE_MAX_CONTEXT_TOKENS",
        legacy: &["CLAUDE_CODE_MAX_CONTEXT_TOKENS"],
    },
    AliasGroup {
        canonical: "ORBCODE_DISABLE_1M_CONTEXT",
        legacy: &["CLAUDE_CODE_DISABLE_1M_CONTEXT"],
    },
    AliasGroup {
        canonical: "ORBCODE_AUTO_COMPACT_WINDOW",
        legacy: &["CLAUDE_CODE_AUTO_COMPACT_WINDOW"],
    },
    AliasGroup {
        canonical: "ORBCODE_MAX_OUTPUT_TOKENS",
        legacy: &["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
    },
    AliasGroup {
        canonical: "ORBCODE_MAX_BUDGET_USD",
        legacy: &["CLAUDE_CODE_MAX_BUDGET_USD"],
    },
    AliasGroup {
        canonical: "ORBCODE_EXTRA_BODY",
        legacy: &["CLAUDE_CODE_EXTRA_BODY"],
    },
    AliasGroup {
        canonical: "ORBCODE_EXTRA_METADATA",
        legacy: &["CLAUDE_CODE_EXTRA_METADATA"],
    },
    AliasGroup {
        canonical: "ORBCODE_BETAS",
        legacy: &["CLAUDE_CODE_BETAS"],
    },
    AliasGroup {
        canonical: "ORBCODE_USER_AGENT",
        legacy: &["CLAUDE_CODE_USER_AGENT", "USER_AGENT"],
    },
    AliasGroup {
        canonical: "ORBCODE_CUSTOM_HEADERS",
        legacy: &["ANTHROPIC_CUSTOM_HEADERS"],
    },
    AliasGroup {
        canonical: "ORBCODE_API_TIMEOUT_MS",
        legacy: &["API_TIMEOUT_MS"],
    },
    AliasGroup {
        canonical: "ORBCODE_API_MAX_RETRIES",
        legacy: &["API_MAX_RETRIES"],
    },
    AliasGroup {
        canonical: "ORBCODE_PROXY",
        legacy: &["CLAUDE_CODE_PROXY", "ANTHROPIC_PROXY_URL"],
    },
    AliasGroup {
        canonical: "ORBCODE_RETRY_BASE_DELAY_MS",
        legacy: &["CLAUDE_CODE_RETRY_BASE_DELAY_MS"],
    },
    AliasGroup {
        canonical: "ORBCODE_RETRY_MAX_DELAY_MS",
        legacy: &["CLAUDE_CODE_RETRY_MAX_DELAY_MS"],
    },
    AliasGroup {
        canonical: "ORBCODE_BLOCKING_LIMIT_OVERRIDE",
        legacy: &["CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE"],
    },
    // ── Tools env ─────────────────────────────────────────────────────
    AliasGroup {
        canonical: "ORBCODE_GLOB_TIMEOUT_SECONDS",
        legacy: &["CLAUDE_CODE_GLOB_TIMEOUT_SECONDS"],
    },
    AliasGroup {
        canonical: "ORBCODE_WEB_ALLOWED_DOMAINS",
        legacy: &["CLAUDE_CODE_WEB_ALLOWED_DOMAINS"],
    },
    AliasGroup {
        canonical: "ORBCODE_WEB_BLOCKED_DOMAINS",
        legacy: &["CLAUDE_CODE_WEB_BLOCKED_DOMAINS"],
    },
    AliasGroup {
        canonical: "ORBCODE_TASK_LIST_ID",
        legacy: &["CLAUDE_CODE_TASK_LIST_ID"],
    },
];

/// Resolve an env var from the process environment with alias expansion.
///
/// Checks canonical `ORBCODE_*` key first, then legacy aliases. Empty
/// strings are treated as unset. This is the narrow public API for
/// crates that need alias-aware env reads without access to
/// `AppConfig` or `ClaudeSettings` (e.g. the tools crate).
pub fn resolve_process_env(key: &str) -> Option<String> {
    for k in resolve_keys(key) {
        if let Some(value) = std::env::var(k).ok().filter(|v| !v.trim().is_empty()) {
            return Some(value);
        }
    }
    None
}

/// Return the ordered list of env var keys to probe for `key`.
///
/// If `key` belongs to an alias group (as either canonical or legacy), all
/// keys in the group are returned in canonical-first order. For an
/// unrecognized key the single-element list `[key]` is returned.
pub(crate) fn resolve_keys(key: &str) -> Vec<&str> {
    for group in ALIASES {
        if group.canonical == key || group.legacy.contains(&key) {
            let mut keys = Vec::with_capacity(1 + group.legacy.len());
            keys.push(group.canonical);
            keys.extend_from_slice(group.legacy);
            return keys;
        }
    }
    vec![key]
}

/// Return all canonical keys from the alias table. Used by
/// `sealed_provider_env_overrides` so tests seal both canonical and legacy
/// names.
pub(crate) fn canonical_keys() -> impl Iterator<Item = &'static str> {
    ALIASES.iter().map(|group| group.canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_keys_returns_canonical_first_for_legacy_key() {
        let keys = resolve_keys("ANTHROPIC_API_KEY");
        assert_eq!(keys, vec!["ORBCODE_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]);
    }

    #[test]
    fn resolve_keys_returns_canonical_first_for_canonical_key() {
        let keys = resolve_keys("ORBCODE_ANTHROPIC_API_KEY");
        assert_eq!(keys, vec!["ORBCODE_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]);
    }

    #[test]
    fn resolve_keys_returns_single_for_unknown_key() {
        let keys = resolve_keys("UNRELATED_VAR");
        assert_eq!(keys, vec!["UNRELATED_VAR"]);
    }

    #[test]
    fn resolve_keys_multi_legacy_alias() {
        let keys = resolve_keys("CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(keys, vec!["ORBCODE_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"]);
    }

    #[test]
    fn canonical_keys_are_all_orbcode_prefixed() {
        for key in canonical_keys() {
            assert!(
                key.starts_with("ORBCODE_"),
                "canonical key {key} should start with ORBCODE_"
            );
        }
    }

    #[test]
    fn no_duplicate_keys_across_groups() {
        let mut seen = std::collections::HashSet::new();
        for group in ALIASES {
            assert!(
                seen.insert(group.canonical),
                "duplicate canonical key: {}",
                group.canonical
            );
            for legacy in group.legacy {
                assert!(
                    seen.insert(legacy),
                    "duplicate legacy key: {legacy} (in group {})",
                    group.canonical
                );
            }
        }
    }

    use std::collections::BTreeMap;

    use crate::claude_home::resolve_env_value_with;

    fn make_process_env(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn lookup(env: &BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        |key| env.get(key).cloned()
    }

    #[test]
    fn resolve_env_value_prefers_canonical_process_env() {
        let process = make_process_env(&[
            ("ORBCODE_ANTHROPIC_MODEL", "canonical-model"),
            ("ANTHROPIC_MODEL", "legacy-model"),
        ]);
        let settings = BTreeMap::new();
        let result = resolve_env_value_with("ANTHROPIC_MODEL", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("canonical-model"));
    }

    #[test]
    fn resolve_env_value_falls_back_to_legacy_process_env() {
        let process = make_process_env(&[("ANTHROPIC_MODEL", "legacy-model")]);
        let settings = BTreeMap::new();
        let result = resolve_env_value_with("ANTHROPIC_MODEL", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("legacy-model"));
    }

    #[test]
    fn resolve_env_value_canonical_settings_over_legacy_settings() {
        let process = BTreeMap::new();
        let settings = make_process_env(&[
            ("ORBCODE_ANTHROPIC_MODEL", "canonical"),
            ("ANTHROPIC_MODEL", "legacy"),
        ]);
        let result = resolve_env_value_with("ANTHROPIC_MODEL", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("canonical"));
    }

    #[test]
    fn resolve_env_value_empty_string_process_env_is_unset() {
        let process = make_process_env(&[
            ("ORBCODE_ANTHROPIC_MODEL", ""),
            ("ANTHROPIC_MODEL", "fallback"),
        ]);
        let settings = BTreeMap::new();
        let result = resolve_env_value_with("ANTHROPIC_MODEL", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("fallback"));
    }

    #[test]
    fn resolve_env_value_empty_string_settings_env_is_unset() {
        let process = BTreeMap::new();
        let settings = make_process_env(&[
            ("ORBCODE_ANTHROPIC_MODEL", ""),
            ("ANTHROPIC_MODEL", "fallback"),
        ]);
        let result = resolve_env_value_with("ANTHROPIC_MODEL", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("fallback"));
    }

    #[test]
    fn resolve_env_value_process_env_wins_over_settings_env() {
        let process = make_process_env(&[("ANTHROPIC_MODEL", "from-shell")]);
        let settings = make_process_env(&[("ANTHROPIC_MODEL", "from-settings")]);
        let result = resolve_env_value_with("ANTHROPIC_MODEL", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("from-shell"));
    }

    #[test]
    fn resolve_env_value_unaliased_key_passes_through() {
        let process = make_process_env(&[("CUSTOM_FLAG", "yes")]);
        let settings = BTreeMap::new();
        let result = resolve_env_value_with("CUSTOM_FLAG", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("yes"));
    }

    // ── Request-option alias equivalence ──────────────────────────────

    #[test]
    fn request_option_aliases_resolve_via_canonical_and_legacy() {
        let cases = [
            (
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
                "ORBCODE_MAX_CONTEXT_TOKENS",
            ),
            ("CLAUDE_CODE_MAX_BUDGET_USD", "ORBCODE_MAX_BUDGET_USD"),
            ("CLAUDE_CODE_EXTRA_BODY", "ORBCODE_EXTRA_BODY"),
            ("CLAUDE_CODE_PROXY", "ORBCODE_PROXY"),
            (
                "CLAUDE_CODE_RETRY_BASE_DELAY_MS",
                "ORBCODE_RETRY_BASE_DELAY_MS",
            ),
            ("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "ORBCODE_MAX_OUTPUT_TOKENS"),
        ];
        for (legacy, canonical) in cases {
            let process = make_process_env(&[(canonical, "42")]);
            let settings = BTreeMap::new();
            let result = resolve_env_value_with(legacy, &settings, lookup(&process));
            assert_eq!(
                result.as_deref(),
                Some("42"),
                "lookup via legacy {legacy} should find canonical {canonical}"
            );
        }
    }

    #[test]
    fn proxy_multi_legacy_alias_resolves() {
        let keys = resolve_keys("ANTHROPIC_PROXY_URL");
        assert_eq!(
            keys,
            vec!["ORBCODE_PROXY", "CLAUDE_CODE_PROXY", "ANTHROPIC_PROXY_URL"]
        );
        let process = make_process_env(&[("ORBCODE_PROXY", "http://canonical")]);
        let settings = BTreeMap::new();
        let result = resolve_env_value_with("ANTHROPIC_PROXY_URL", &settings, lookup(&process));
        assert_eq!(result.as_deref(), Some("http://canonical"));
    }

    // ── Inventory guard ───────────────────────────────────────────────

    /// Guard test: every CLAUDE_CODE_* and ANTHROPIC_*/OPENAI_*/GEMINI_*
    /// env var that config reads through resolve_env must be in the alias
    /// table. If you add a new env var, add it here AND to ALIASES.
    #[test]
    fn inventory_all_provider_env_keys_are_aliased() {
        let known_aliased = [
            // Auth
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "XAI_API_KEY",
            "GROK_API_KEY",
            // Base URL
            "ANTHROPIC_BASE_URL",
            "OPENAI_BASE_URL",
            // Model selection
            "ANTHROPIC_MODEL",
            "OPENAI_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "OPENAI_SMALL_FAST_MODEL",
            // Model family
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "OPENAI_DEFAULT_OPUS_MODEL",
            "OPENAI_DEFAULT_SONNET_MODEL",
            "OPENAI_DEFAULT_HAIKU_MODEL",
            // Custom model option
            "ANTHROPIC_CUSTOM_MODEL_OPTION",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
            // Request options
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            "CLAUDE_CODE_DISABLE_1M_CONTEXT",
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "CLAUDE_CODE_MAX_BUDGET_USD",
            "CLAUDE_CODE_EXTRA_BODY",
            "CLAUDE_CODE_EXTRA_METADATA",
            "CLAUDE_CODE_BETAS",
            "CLAUDE_CODE_USER_AGENT",
            "USER_AGENT",
            "CLAUDE_CODE_PROXY",
            "ANTHROPIC_PROXY_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "API_TIMEOUT_MS",
            "API_MAX_RETRIES",
            "CLAUDE_CODE_RETRY_BASE_DELAY_MS",
            "CLAUDE_CODE_RETRY_MAX_DELAY_MS",
            "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE",
            // Tools env
            "CLAUDE_CODE_GLOB_TIMEOUT_SECONDS",
            "CLAUDE_CODE_WEB_ALLOWED_DOMAINS",
            "CLAUDE_CODE_WEB_BLOCKED_DOMAINS",
            "CLAUDE_CODE_TASK_LIST_ID",
        ];
        for key in known_aliased {
            let keys = resolve_keys(key);
            assert!(
                keys.len() > 1,
                "env var {key} is in the inventory but NOT in the alias table — \
                 add it to ALIASES in env_compat.rs"
            );
        }
    }

    /// Guard: keys that are orbcode-only (no legacy alias needed) must NOT
    /// accidentally appear in the alias table.
    #[test]
    fn orbcode_only_keys_are_not_aliased() {
        let orbcode_only = [
            "ORBCODE_PROVIDER",
            "ORBCODE_FALLBACK_PROVIDER",
            "ORBCODE_MAX_RETRIES",
            "ORBCODE_SANDBOX_MODE",
            "ORBCODE_ALLOW_NETWORK",
            "ORBCODE_ALLOW_TOOLS",
            "ORBCODE_TRUSTED_PROJECT",
            "ORBCODE_HOME",
            "ORBCODE_BUNDLED_SKILLS_DIR",
            "ORBCODE_WEB_SEARCH_ORDER",
            "ORBCODE_FORCE_RG_FALLBACK",
        ];
        for key in orbcode_only {
            let keys = resolve_keys(key);
            assert_eq!(
                keys,
                vec![key],
                "orbcode-only key {key} should NOT be in the alias table"
            );
        }
    }

    /// Guard: process-only behavioral flags that intentionally bypass the
    /// alias resolver and settings.env. These are TS-era flags where adding
    /// settings.env fallback would change behavior or where Orb Code has no
    /// matching feature. Listed here so they aren't accidentally routed
    /// through the resolver.
    #[test]
    fn process_only_exceptions_are_documented() {
        let process_only = [
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
            "CLAUDE_CODE_SIMPLE",
            "CLAUDE_CODE_REMOTE",
            "CLAUDE_CODE_REMOTE_MEMORY_DIR",
            "CLAUDE_CODE_MANAGED_SETTINGS_PATH",
            "CLAUDE_CONFIG_DIR",
        ];
        for key in process_only {
            let keys = resolve_keys(key);
            assert_eq!(
                keys,
                vec![key],
                "process-only exception {key} must NOT be in the alias table; \
                 if it needs alias support, move it out of this list first"
            );
        }
    }

    /// Source-scan guard: known legacy compatibility keys must not appear
    /// in direct `std::env::var("LEGACY_KEY")` calls outside the resolver
    /// and the explicitly allowlisted locations. If this test fails, the
    /// offending call site should use `resolve_process_env`,
    /// `ToolContext::resolve_env`, or `AppConfig::resolve_env` instead.
    #[test]
    fn no_direct_reads_of_aliased_legacy_keys() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let legacy_keys: Vec<&str> = ALIASES
            .iter()
            .flat_map(|group| group.legacy.iter().copied())
            .collect();

        let allowlisted_files = [
            "config/src/env_compat.rs",
            "config/src/claude_home.rs",
            "config/src/auth.rs",
            "config/src/config.rs",
            "config/src/settings_resolution.rs",
            "config/src/memory.rs",
            "config/src/policy.rs",
        ];

        let mut violations = Vec::new();
        for key in &legacy_keys {
            let pattern = format!("\"{key}\"");
            let output = std::process::Command::new("rg")
                .args([
                    "--no-heading",
                    "-n",
                    "--glob",
                    "*.rs",
                    "--glob",
                    "!**/tests/**",
                    "--glob",
                    "!**/test*",
                    &pattern,
                ])
                .current_dir(workspace)
                .output()
                .expect("rg should be available");
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.contains("env::var") && !line.contains("env_var") {
                    continue;
                }
                let is_allowlisted = allowlisted_files
                    .iter()
                    .any(|allowed| line.starts_with(allowed));
                if !is_allowlisted {
                    violations.push(format!("{key}: {line}"));
                }
            }
        }
        assert!(
            violations.is_empty(),
            "found direct std::env::var reads of aliased legacy keys \
             outside the resolver — use resolve_process_env or \
             ToolContext::resolve_env instead:\n{}",
            violations.join("\n")
        );
    }
}
