use orbcode_config::{
    AgentSource, AppConfig, AuthOverview, ContributedHookSource, HookProvenance, discover_hooks,
    load_agent_definitions_with_warnings, load_claude_ai_oauth,
    load_output_style_definitions_with_warnings, load_plugin_registry, plugin_contributed_hooks,
    shadowed_home,
};
use orbcode_core::SessionStorageHealth;
use orbcode_protocol::ProviderId;

use super::{DoctorCheck, DoctorStatus};

pub(super) fn auth_check(
    default_provider: ProviderId,
    fallback_provider: Option<ProviderId>,
    auth: &AuthOverview,
) -> DoctorCheck {
    let default_ready = auth.has_provider(default_provider);
    let fallback_ready = fallback_provider.is_none_or(|provider| auth.has_provider(provider));

    match (default_ready, fallback_ready) {
        (true, true) => DoctorCheck {
            name: "auth".to_string(),
            status: DoctorStatus::Pass,
            detail: format!("credentials modeled for {default_provider}"),
        },
        (false, _) => DoctorCheck {
            name: "auth".to_string(),
            status: DoctorStatus::Warn,
            detail: format!(
                "no auth modeled for default provider {default_provider}; use `orbcode auth login` or environment variables"
            ),
        },
        (true, false) => DoctorCheck {
            name: "auth".to_string(),
            status: DoctorStatus::Warn,
            detail: format!(
                "fallback provider {} has no modeled auth",
                fallback_provider
                    .map_or_else(|| "none".to_string(), |provider| provider.to_string())
            ),
        },
    }
}

const OAUTH_EXPIRY_CHECK: &str = "oauth_token_expiry";

/// Warn when the stored OAuth token will expire within 5 minutes (300 s).
/// Reading the credentials file is local I/O, so this never needs an opt-in
/// gate like the live probe.
pub(super) fn oauth_expiry_check(config: &AppConfig) -> DoctorCheck {
    let Some(oauth) = load_claude_ai_oauth(&config.home_dir) else {
        return DoctorCheck {
            name: OAUTH_EXPIRY_CHECK.to_string(),
            status: DoctorStatus::Pass,
            detail: "no stored OAuth token".to_string(),
        };
    };
    let Some(expires_at_ms) = oauth.expires_at else {
        return DoctorCheck {
            name: OAUTH_EXPIRY_CHECK.to_string(),
            status: DoctorStatus::Pass,
            detail: "OAuth token has no expiry timestamp".to_string(),
        };
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    let remaining_secs = (expires_at_ms - now_ms) / 1000;

    if remaining_secs <= 0 {
        return DoctorCheck {
            name: OAUTH_EXPIRY_CHECK.to_string(),
            status: DoctorStatus::Fail,
            detail: "OAuth token has expired, run `orbcode auth login` to refresh".to_string(),
        };
    }

    if remaining_secs < 300 {
        return DoctorCheck {
            name: OAUTH_EXPIRY_CHECK.to_string(),
            status: DoctorStatus::Warn,
            detail: format!(
                "OAuth token expires in {remaining_secs} seconds, run `orbcode auth login` to refresh"
            ),
        };
    }

    DoctorCheck {
        name: OAUTH_EXPIRY_CHECK.to_string(),
        status: DoctorStatus::Pass,
        detail: "OAuth token is valid".to_string(),
    }
}

/// Warn when the `~/.orbcode` opt-in has left the user staring at a blank slate.
///
/// Creating `~/.orbcode` makes it win home resolution, but nothing is copied into
/// it. If it is still empty while `~/.claude` next door holds settings,
/// credentials or transcripts, then sessions and logins have apparently vanished
/// — the single most confusing outcome of that opt-in, and the one worth naming
/// explicitly rather than letting someone conclude their data was lost.
pub(super) fn home_dir_check(config: &AppConfig) -> DoctorCheck {
    match shadowed_home(&config.home_dir) {
        Some(shadow) => DoctorCheck {
            name: "home_dir".to_string(),
            status: DoctorStatus::Warn,
            detail: format!(
                "{} has no settings, credentials or transcripts, but it takes precedence over {}, \
                 which still has yours. Creating the directory opted in to a separate home; nothing \
                 was copied into it. Remove {} to go back to the shared home, or copy across what you \
                 need — no data was lost.",
                shadow.active.display(),
                shadow.shadowed.display(),
                shadow.active.display(),
            ),
        },
        None => DoctorCheck {
            name: "home_dir".to_string(),
            status: DoctorStatus::Pass,
            detail: config.home_dir.display().to_string(),
        },
    }
}

pub(super) fn session_storage_check(health: &SessionStorageHealth) -> DoctorCheck {
    let project = health.project_dir.display();

    if !health.writable {
        let reason = health
            .write_probe_error
            .clone()
            .unwrap_or_else(|| "write probe failed".to_string());
        return DoctorCheck {
            name: "session_storage".to_string(),
            status: DoctorStatus::Fail,
            detail: format!(
                "{project}: transcript dir not writable ({reason}); fix permissions or pick a different home dir with ORBCODE_HOME",
            ),
        };
    }

    let mut hints = Vec::new();
    if health.corrupt_transcripts > 0 {
        hints.push(format!(
            "{} corrupt transcript(s) — list with `orbcode sessions` and delete the bad files",
            health.corrupt_transcripts
        ));
    }
    if health.trailing_partial_lines > 0 {
        hints.push(format!(
            "{} transcript(s) end mid-line — next append will heal automatically; resume the session to trigger it",
            health.trailing_partial_lines
        ));
    }
    if health.stray_tmp_files > 0 {
        hints.push(format!(
            "{} leftover *.jsonl.*.tmp file(s) — safe to delete from {project}",
            health.stray_tmp_files
        ));
    }
    if let Some(error) = health.child_session_scan_error.as_deref() {
        hints.push(format!("could not scan child-session storage: {error}"));
    }
    if health.child_sessions.corrupt_metadata_records > 0 {
        hints.push(format!(
            "{} corrupt child-session metadata record(s)",
            health.child_sessions.corrupt_metadata_records
        ));
    }
    if health.child_sessions.corrupt_transcripts > 0 {
        hints.push(format!(
            "{} corrupt child transcript(s)",
            health.child_sessions.corrupt_transcripts
        ));
    }
    if health.child_sessions.orphan_metadata_records > 0 {
        hints.push(format!(
            "{} child-session metadata record(s) reference missing parent sessions",
            health.child_sessions.orphan_metadata_records
        ));
    }
    if health.child_sessions.orphan_transcripts > 0 {
        hints.push(format!(
            "{} child transcript file(s) have no metadata record",
            health.child_sessions.orphan_transcripts
        ));
    }

    let detail_base = format!(
        "{project}: {} transcript(s), {} recoverable; child sessions: {} metadata, {} transcript(s), {} journal-only workflow child(ren)",
        health.total_transcripts,
        health.recoverable_transcripts,
        health.child_sessions.metadata_records,
        health.child_sessions.transcript_records,
        health.child_sessions.workflow_children_without_transcripts,
    );

    if hints.is_empty() {
        DoctorCheck {
            name: "session_storage".to_string(),
            status: DoctorStatus::Pass,
            detail: detail_base,
        }
    } else {
        let status = DoctorStatus::Warn;
        DoctorCheck {
            name: "session_storage".to_string(),
            status,
            detail: format!("{detail_base}; {}", hints.join("; ")),
        }
    }
}

const EXTENSION_LOAD_CHECK: &str = "extension_load";

/// Aggregates the non-fatal load warnings from the three extension loaders
/// (agents, plugins, output styles) into a single diagnostic, and surfaces the
/// cross-layer hook discovery (per-layer settings hooks plus agent-contributed
/// hooks) with each hook's source, trust, and load-time validation status. Each
/// loader already keeps going past a bad file and reports a structured warning;
/// this check is the first product surface that consumes those warnings.
///
/// Warnings only ever originate from user/project/plugin layers (the loaders
/// never read managed/enterprise locations), and hook provenance labels render
/// only the layer name (`user`/`project`/`local`/`managed`) rather than a path,
/// so the rendered detail does not leak managed policy paths.
pub async fn extension_load_check(config: &AppConfig) -> DoctorCheck {
    let home = &config.home_dir;
    let cwd = &config.cwd;
    let mut lines: Vec<String> = Vec::new();
    let mut contributed: Vec<ContributedHookSource> = Vec::new();

    match load_agent_definitions_with_warnings(home, cwd).await {
        Ok(outcome) => {
            for warning in outcome.warnings {
                lines.push(format!(
                    "agent [{}]: {}",
                    warning.source.as_str(),
                    warning.message
                ));
            }
            for def in outcome.definitions {
                if def.hooks.is_empty() {
                    continue;
                }
                let trusted =
                    !matches!(def.source, AgentSource::ProjectSettings) || config.trusted_project;
                contributed.push(ContributedHookSource {
                    provenance: HookProvenance::Agent {
                        name: def.agent_type.clone(),
                    },
                    trusted,
                    hooks: def.hooks.clone(),
                });
            }
        }
        Err(error) => {
            return extension_scan_error("agent definitions", &error.to_string());
        }
    }

    match load_plugin_registry(home, cwd).await {
        Ok(registry) => {
            for warning in &registry.warnings {
                lines.push(format!(
                    "plugin [{}]: {}",
                    warning.plugin_id, warning.message
                ));
            }
            let (plugin_hooks, hook_warnings) =
                plugin_contributed_hooks(&registry, config.trusted_project);
            for warning in hook_warnings {
                lines.push(format!(
                    "plugin [{}]: {}",
                    warning.plugin_id, warning.message
                ));
            }
            contributed.extend(plugin_hooks);
        }
        Err(error) => {
            return extension_scan_error("plugins", &error.to_string());
        }
    }

    match load_output_style_definitions_with_warnings(home, cwd).await {
        Ok(outcome) => {
            for warning in outcome.warnings {
                lines.push(format!(
                    "output-style [{}]: {}",
                    warning.source.as_str(),
                    warning.message
                ));
            }
        }
        Err(error) => {
            return extension_scan_error("output styles", &error.to_string());
        }
    }

    let discovery = discover_hooks(
        &config.settings_layers,
        &config.policy,
        config.trusted_project,
        &contributed,
    );
    for warning in &discovery.warnings {
        lines.push(format!("hook {}", warning.summary_line()));
    }
    let hook_lines: Vec<String> = discovery
        .hooks
        .iter()
        .map(orbcode_config::DiscoveredHook::summary_line)
        .collect();

    let warning_count = lines.len();
    let status = if warning_count == 0 {
        DoctorStatus::Pass
    } else {
        DoctorStatus::Warn
    };

    let mut segments: Vec<String> = Vec::new();
    if warning_count > 0 {
        segments.push(format!(
            "{warning_count} extension load warning(s): {}",
            lines.join("; ")
        ));
    }
    if !hook_lines.is_empty() {
        segments.push(format!(
            "{} hook(s) discovered: {}",
            hook_lines.len(),
            hook_lines.join("; ")
        ));
    }

    let detail = if segments.is_empty() {
        "agents, plugins, and output styles loaded without warnings".to_string()
    } else {
        segments.join(" | ")
    };

    DoctorCheck {
        name: EXTENSION_LOAD_CHECK.to_string(),
        status,
        detail,
    }
}

fn extension_scan_error(scope: &str, reason: &str) -> DoctorCheck {
    DoctorCheck {
        name: EXTENSION_LOAD_CHECK.to_string(),
        status: DoctorStatus::Warn,
        detail: format!("could not scan {scope}: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::doctor::tests::config;
    use orbcode_config::{SettingsLayer, SettingsLayers, SettingsSource};

    fn config_at(home: std::path::PathBuf, cwd: std::path::PathBuf) -> AppConfig {
        let mut config = config();
        config.home_dir = home;
        config.cwd = cwd;
        config
    }

    async fn write_file(path: &Path, contents: &str) {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, contents).await.unwrap();
    }

    fn settings_layer(source: SettingsSource, hooks_json: &str) -> SettingsLayer {
        let value: serde_json::Value = serde_json::from_str(hooks_json).expect("valid json");
        let raw = value.as_object().cloned();
        SettingsLayer {
            source,
            primary_path: std::path::PathBuf::from(format!("/{}.json", source.short_label())),
            contributing_paths: Vec::new(),
            raw,
            errors: Vec::new(),
        }
    }

    #[tokio::test]
    async fn extension_load_passes_when_no_warnings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        write_file(
            &home.join("agents").join("worker.md"),
            "---\nname: worker\ndescription: do work\n---\nbody\n",
        )
        .await;
        write_file(
            &home.join("output-styles").join("Fancy.md"),
            "---\nname: Fancy\ndescription: a style\n---\nbe fancy\n",
        )
        .await;

        let check = extension_load_check(&config_at(home, cwd)).await;
        assert_eq!(check.name, "extension_load");
        assert_eq!(check.status, DoctorStatus::Pass);
        assert!(check.detail.contains("without warnings"));
    }

    #[tokio::test]
    async fn extension_load_warns_and_lists_each_source() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");

        write_file(
            &home.join("agents").join("broken.md"),
            "---\nname: broken\n---\nbody\n",
        )
        .await;

        write_file(
            &home.join("output-styles").join("Broken.md"),
            "---\nname: Broken\ndescription: d\nno closing delimiter\n",
        )
        .await;

        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_file(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            "{ this is not valid json",
        )
        .await;
        write_file(
            &home.join("plugins").join("installed_plugins.json"),
            &format!(
                r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
                plugin_root.display()
            ),
        )
        .await;
        write_file(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let check = extension_load_check(&config_at(home, cwd)).await;
        assert_eq!(check.status, DoctorStatus::Warn);
        assert!(
            check.detail.starts_with("3 extension load warning(s):"),
            "detail was: {}",
            check.detail
        );
        assert!(
            check.detail.contains("agent ["),
            "missing agent line: {}",
            check.detail
        );
        assert!(
            check.detail.contains("plugin ["),
            "missing plugin line: {}",
            check.detail
        );
        assert!(
            check.detail.contains("output-style ["),
            "missing output-style line: {}",
            check.detail
        );
        assert!(check.detail.contains("description"));
        assert!(check.detail.contains("not valid JSON"));
    }

    #[tokio::test]
    async fn extension_load_lists_discovered_hooks_with_status() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let mut config = config_at(home, cwd);
        config.settings_layers = SettingsLayers {
            layers: vec![
                settings_layer(
                    SettingsSource::User,
                    r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo user"}]}]}}"#,
                ),
                settings_layer(
                    SettingsSource::Local,
                    r#"{"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"  "}]}]}}"#,
                ),
            ],
        };

        let check = extension_load_check(&config).await;
        assert_eq!(check.name, "extension_load");
        assert_eq!(
            check.status,
            DoctorStatus::Warn,
            "an invalid hook command must drive Warn: {}",
            check.detail
        );
        assert!(
            check.detail.contains("[user]"),
            "should list the user-layer hook: {}",
            check.detail
        );
        assert!(
            check.detail.contains("[local]"),
            "should list the local-layer hook: {}",
            check.detail
        );
        assert!(
            check.detail.contains("invalid"),
            "should flag the invalid command: {}",
            check.detail
        );
        assert!(
            check.detail.contains("hook(s) discovered"),
            "should render the hook discovery segment: {}",
            check.detail
        );
    }

    #[test]
    fn oauth_expiry_warns_when_token_expires_soon() {
        let mut config = config();
        let home = tempfile::tempdir().unwrap();
        config.home_dir = home.path().to_path_buf();
        let expires_at_ms = chrono::Utc::now().timestamp_millis() + 120_000;
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "test-token",
                "expiresAt": expires_at_ms,
                "scopes": ["user:inference"]
            }
        });
        std::fs::write(home.path().join(".credentials.json"), creds.to_string()).unwrap();

        let check = oauth_expiry_check(&config);
        assert_eq!(check.name, OAUTH_EXPIRY_CHECK);
        assert_eq!(check.status, DoctorStatus::Warn);
        assert!(
            check.detail.contains("expires in") && check.detail.contains("orbcode auth login"),
            "detail should warn about near-expiry: {}",
            check.detail
        );
    }

    #[test]
    fn oauth_expiry_fails_when_token_is_expired() {
        let mut config = config();
        let home = tempfile::tempdir().unwrap();
        config.home_dir = home.path().to_path_buf();
        let expires_at_ms = chrono::Utc::now().timestamp_millis() - 60_000;
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "test-token",
                "expiresAt": expires_at_ms,
                "scopes": ["user:inference"]
            }
        });
        std::fs::write(home.path().join(".credentials.json"), creds.to_string()).unwrap();

        let check = oauth_expiry_check(&config);
        assert_eq!(check.status, DoctorStatus::Fail);
        assert!(check.detail.contains("expired"));
    }

    #[test]
    fn oauth_expiry_passes_when_token_is_fresh() {
        let mut config = config();
        let home = tempfile::tempdir().unwrap();
        config.home_dir = home.path().to_path_buf();
        let expires_at_ms = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "test-token",
                "expiresAt": expires_at_ms,
                "scopes": ["user:inference"]
            }
        });
        std::fs::write(home.path().join(".credentials.json"), creds.to_string()).unwrap();

        let check = oauth_expiry_check(&config);
        assert_eq!(check.status, DoctorStatus::Pass);
        assert!(check.detail.contains("valid"));
    }

    #[test]
    fn oauth_expiry_passes_when_no_credentials_file() {
        let mut config = config();
        let home = tempfile::tempdir().unwrap();
        config.home_dir = home.path().to_path_buf();

        let check = oauth_expiry_check(&config);
        assert_eq!(check.status, DoctorStatus::Pass);
        assert!(check.detail.contains("no stored OAuth token"));
    }
}
