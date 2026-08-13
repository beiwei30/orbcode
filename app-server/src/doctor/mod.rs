mod environment;
mod mcp;
mod provider;
mod sandbox;

use std::path::Path;

use orbcode_app_server_protocol::{DoctorCheck, DoctorReport, DoctorStatus};
use orbcode_config::{AppConfig, AuthMethod, AuthOverview, load_chatgpt_oauth, model_capabilities};
use orbcode_core::{CoreError, SessionStorageHealth};
use orbcode_mcp::McpRegistry;
use orbcode_protocol::{ProviderId, TurnContext};
use tokio::process::Command;

pub async fn run_doctor(
    config: &AppConfig,
    context: TurnContext,
    session_count: usize,
    background_job_count: usize,
    mcp_server_count: usize,
    mcp: Option<&McpRegistry>,
    auth: &orbcode_config::AuthOverview,
    storage_health: SessionStorageHealth,
) -> Result<DoctorReport, CoreError> {
    let mut checks = Vec::new();
    checks.push(path_check("workspace", &config.cwd).await);
    checks.push(path_check("state_dir", &config.home_dir).await);
    checks.push(environment::home_dir_check(config));
    checks.push(git_context_check(&context));
    checks.push(provider_chain_check(
        config.default_provider,
        config.fallback_provider,
    ));
    checks.push(model_capabilities_check(config, auth));
    checks.push(environment::auth_check(
        config.default_provider,
        config.fallback_provider,
        auth,
    ));
    checks.push(environment::oauth_expiry_check(config));
    checks.push(chatgpt_subscription_check(config, auth).await);
    checks.push(provider::provider_probe_check(config).await);
    checks.push(permission_check(
        "network",
        config.allow_network,
        "network-backed tools",
    ));
    checks.push(permission_check(
        "tools",
        config.allow_tools,
        "tool execution and local file mutation",
    ));
    checks.push(sandbox::sandbox_check(
        config.sandbox_mode,
        config.sandbox_allow_network,
    ));
    checks.push(binary_check("git", &["--version"]).await);
    checks.push(binary_check("rg", &["--version"]).await);
    checks.push(binary_check("bun", &["--version"]).await);
    checks.push(binary_check("cargo", &["--version"]).await);
    checks.push(DoctorCheck {
        name: "sessions".to_string(),
        status: DoctorStatus::Pass,
        detail: format!("{session_count} persisted session(s)"),
    });
    checks.push(environment::session_storage_check(&storage_health));
    checks.push(DoctorCheck {
        name: "background_jobs".to_string(),
        status: DoctorStatus::Pass,
        detail: format!("{background_job_count} persisted background job(s)"),
    });
    checks.push(if mcp_server_count == 0 {
        DoctorCheck {
            name: "mcp".to_string(),
            status: DoctorStatus::Warn,
            detail: "no MCP servers configured; add one via settings.json, .mcp.json, or `orbcode mcp add`".to_string(),
        }
    } else {
        DoctorCheck {
            name: "mcp".to_string(),
            status: DoctorStatus::Pass,
            detail: format!("{mcp_server_count} configured MCP server(s)"),
        }
    });
    if let Some(registry) = mcp {
        checks.push(mcp::mcp_reachability_check(config, registry).await);
    }
    checks.push(environment::extension_load_check(config).await);

    Ok(DoctorReport { checks })
}

pub(super) fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

async fn path_check(name: &str, path: &Path) -> DoctorCheck {
    match tokio::fs::try_exists(path).await {
        Ok(true) => DoctorCheck {
            name: name.to_string(),
            status: DoctorStatus::Pass,
            detail: path.display().to_string(),
        },
        Ok(false) => DoctorCheck {
            name: name.to_string(),
            status: DoctorStatus::Fail,
            detail: format!("missing {}", path.display()),
        },
        Err(error) => DoctorCheck {
            name: name.to_string(),
            status: DoctorStatus::Fail,
            detail: error.to_string(),
        },
    }
}

fn git_context_check(context: &TurnContext) -> DoctorCheck {
    match &context.git_branch {
        Some(branch) => DoctorCheck {
            name: "git_repo".to_string(),
            status: DoctorStatus::Pass,
            detail: format!(
                "branch={} status={}",
                branch,
                context
                    .git_status
                    .clone()
                    .unwrap_or_else(|| "clean".to_string())
            ),
        },
        None => DoctorCheck {
            name: "git_repo".to_string(),
            status: DoctorStatus::Warn,
            detail: "git metadata unavailable for current cwd".to_string(),
        },
    }
}

fn provider_chain_check(
    default_provider: ProviderId,
    fallback_provider: Option<ProviderId>,
) -> DoctorCheck {
    DoctorCheck {
        name: "provider_chain".to_string(),
        status: DoctorStatus::Pass,
        detail: format!(
            "default={} fallback={}",
            default_provider,
            fallback_provider.map_or_else(|| "none".to_string(), |provider| provider.to_string())
        ),
    }
}

fn model_capabilities_check(config: &AppConfig, auth: &AuthOverview) -> DoctorCheck {
    let request_model = if chatgpt_is_active(auth) && !config.provider_model_is_explicit() {
        "gpt-5.6-sol".to_string()
    } else {
        config
            .provider_model_resolution(config.default_provider)
            .request_model
    };
    let caps = model_capabilities(&request_model, config.default_provider);
    let mut features = Vec::new();
    if caps.supports_thinking {
        features.push("thinking");
    }
    if caps.supports_vision {
        features.push("vision");
    }
    if caps.supports_streaming {
        features.push("streaming");
    }
    DoctorCheck {
        name: "model_capabilities".to_string(),
        status: DoctorStatus::Pass,
        detail: format!(
            "model={} context={}k max_output={}k features=[{}]",
            request_model,
            caps.context_window / 1_000,
            caps.max_output_tokens / 1_000,
            features.join(", "),
        ),
    }
}

fn chatgpt_is_active(auth: &AuthOverview) -> bool {
    auth.entries.iter().any(|entry| {
        entry.provider == ProviderId::OpenAi
            && entry.method == AuthMethod::ChatGpt
            && entry.usable
            && entry.active
    })
}

async fn chatgpt_subscription_check(config: &AppConfig, auth: &AuthOverview) -> DoctorCheck {
    let name = "chatgpt_subscription".to_string();
    let Some(credentials) = load_chatgpt_oauth(&config.home_dir).await else {
        return DoctorCheck {
            name,
            status: DoctorStatus::Pass,
            detail: "not configured".to_string(),
        };
    };
    if credentials.account_id.as_deref().is_none_or(str::is_empty) {
        return DoctorCheck {
            name,
            status: DoctorStatus::Fail,
            detail: "saved login has no ChatGPT account id; sign in again with `orbcode auth login --provider openai --method chatgpt`".to_string(),
        };
    }
    if !credentials.is_usable() {
        return DoctorCheck {
            name,
            status: DoctorStatus::Fail,
            detail: "saved ChatGPT login is incomplete; sign in again with `orbcode auth login --provider openai --method chatgpt`".to_string(),
        };
    }
    if !chatgpt_is_active(auth) {
        return DoctorCheck {
            name,
            status: DoctorStatus::Warn,
            detail: "ready but shadowed by a higher-precedence OpenAI API key".to_string(),
        };
    }

    let remaining_secs = (credentials.expires_at - chrono::Utc::now().timestamp_millis()) / 1000;
    let model = if config.provider_model_is_explicit() {
        config.provider_model_name(ProviderId::OpenAi)
    } else {
        "gpt-5.6-sol".to_string()
    };
    let plan = credentials.plan_type.as_deref().unwrap_or("unknown");
    if remaining_secs < 300 {
        return DoctorCheck {
            name,
            status: DoctorStatus::Warn,
            detail: format!(
                "active; token refresh required on next request; plan={plan} model={model} endpoint=fixed ChatGPT Codex Responses"
            ),
        };
    }
    DoctorCheck {
        name,
        status: DoctorStatus::Pass,
        detail: format!("active; plan={plan} model={model} endpoint=fixed ChatGPT Codex Responses"),
    }
}

fn permission_check(name: &str, enabled: bool, scope: &str) -> DoctorCheck {
    DoctorCheck {
        name: format!("{name}_permission"),
        status: if enabled {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Warn
        },
        detail: if enabled {
            format!("{scope} enabled")
        } else {
            format!("{scope} disabled")
        },
    }
}

async fn binary_check(binary: &str, args: &[&str]) -> DoctorCheck {
    match Command::new(binary).args(args).output().await {
        Ok(output) if output.status.success() => DoctorCheck {
            name: format!("{binary}_binary"),
            status: DoctorStatus::Pass,
            detail: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        },
        Ok(output) => DoctorCheck {
            name: format!("{binary}_binary"),
            status: DoctorStatus::Warn,
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        },
        Err(error) => DoctorCheck {
            name: format!("{binary}_binary"),
            status: DoctorStatus::Warn,
            detail: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_config::{
        AuthMethod, AuthOverview, AuthStatusEntry, ClaudeSettings, EffectivePolicy, SettingsLayers,
    };
    use orbcode_core::SessionStorageHealth;
    use std::path::Path;

    pub(super) fn config() -> AppConfig {
        let root = std::env::temp_dir().join(format!("orbcode-doctor-{}", std::process::id()));
        let mut env_overrides = std::collections::HashMap::new();
        env_overrides.insert("ORBCODE_DOCTOR_PROBE".to_string(), String::new());
        env_overrides.insert("ORBCODE_DOCTOR_MCP_PROBE".to_string(), String::new());
        AppConfig {
            cwd: root.clone(),
            home_dir: root.clone(),
            sessions_dir: root.join("sessions"),
            projects_dir: root.join("projects"),
            current_project_dir: root.join("projects").join("doctor"),
            history_path: root.join("history.jsonl"),
            settings_path: root.join("settings.json"),
            default_provider: ProviderId::Anthropic,
            fallback_provider: Some(ProviderId::OpenAi),
            max_retries: 2,
            sandbox_mode: orbcode_protocol::SandboxMode::DangerFullAccess,
            sandbox_allow_network: true,
            allow_network: true,
            provider_allow_network: true,
            allow_tools: false,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            ask_tools: Vec::new(),
            additional_directories: Vec::new(),
            mcp_config_inputs: Vec::new(),
            settings: ClaudeSettings::default(),
            settings_layers: SettingsLayers::default(),
            resolved_settings: Default::default(),
            settings_warnings: Vec::new(),
            policy: EffectivePolicy::default(),
            policy_conflicts: Vec::new(),
            runtime_model_override: orbcode_config::RuntimeModelOverride::Inherit,
            refreshed_persisted_model_setting: None,
            env_overrides,
            append_system_prompt: None,
            permission_mode: None,
            explicit_permission_overrides: Default::default(),
            trusted_project: true,
        }
    }

    #[tokio::test]
    async fn warns_when_default_provider_has_no_auth() {
        let report = run_doctor(
            &config(),
            TurnContext {
                cwd: "/tmp".to_string(),
                current_date: "2026-04-09".to_string(),
                ..Default::default()
            },
            0,
            0,
            1,
            None,
            &AuthOverview {
                store_path: Path::new("/tmp/auth.json").to_path_buf(),
                entries: Vec::new(),
            },
            SessionStorageHealth {
                writable: true,
                ..Default::default()
            },
        )
        .await
        .expect("run doctor");

        assert!(
            report
                .checks
                .iter()
                .any(|check| check.name == "auth" && check.status == DoctorStatus::Warn)
        );
    }

    #[tokio::test]
    async fn passes_when_auth_is_present() {
        let report = run_doctor(
            &config(),
            TurnContext {
                cwd: "/tmp".to_string(),
                current_date: "2026-04-09".to_string(),
                git_branch: Some("main".to_string()),
                git_status: Some("clean".to_string()),
                ..Default::default()
            },
            3,
            1,
            2,
            None,
            &AuthOverview {
                store_path: Path::new("/tmp/auth.json").to_path_buf(),
                entries: vec![
                    AuthStatusEntry {
                        provider: ProviderId::Anthropic,
                        method: AuthMethod::ApiKey,
                        source_summary: "env:ANTHROPIC_API_KEY".to_string(),
                        persisted: false,
                        usable: true,
                        active: true,
                        updated_at: None,
                    },
                    AuthStatusEntry {
                        provider: ProviderId::OpenAi,
                        method: AuthMethod::ApiKey,
                        source_summary: "env:OPENAI_API_KEY".to_string(),
                        persisted: false,
                        usable: true,
                        active: true,
                        updated_at: None,
                    },
                ],
            },
            SessionStorageHealth {
                writable: true,
                ..Default::default()
            },
        )
        .await
        .expect("run doctor");

        assert!(
            report
                .checks
                .iter()
                .any(|check| check.name == "auth" && check.status == DoctorStatus::Pass)
        );
    }

    #[tokio::test]
    async fn surfaces_recovery_hints_for_corrupt_and_partial_transcripts() {
        let report = run_doctor(
            &config(),
            TurnContext {
                cwd: "/tmp".to_string(),
                current_date: "2026-04-09".to_string(),
                git_branch: Some("main".to_string()),
                git_status: Some("clean".to_string()),
                ..Default::default()
            },
            5,
            0,
            0,
            None,
            &AuthOverview {
                store_path: Path::new("/tmp/auth.json").to_path_buf(),
                entries: Vec::new(),
            },
            SessionStorageHealth {
                project_dir: Path::new("/tmp/projects/doctor").to_path_buf(),
                total_transcripts: 5,
                corrupt_transcripts: 1,
                recoverable_transcripts: 1,
                trailing_partial_lines: 1,
                stray_tmp_files: 2,
                writable: true,
                write_probe_error: None,
                ..SessionStorageHealth::default()
            },
        )
        .await
        .expect("run doctor");

        let storage = report
            .checks
            .iter()
            .find(|check| check.name == "session_storage")
            .expect("session_storage check present");
        assert_eq!(storage.status, DoctorStatus::Warn);
        assert!(storage.detail.contains("1 corrupt"));
        assert!(storage.detail.contains("end mid-line"));
        assert!(storage.detail.contains("leftover"));
    }

    #[tokio::test]
    async fn surfaces_child_session_storage_health() {
        let mut storage_health = SessionStorageHealth {
            project_dir: Path::new("/tmp/projects/doctor").to_path_buf(),
            writable: true,
            ..SessionStorageHealth::default()
        };
        storage_health.child_sessions.metadata_records = 3;
        storage_health.child_sessions.transcript_records = 2;
        storage_health.child_sessions.corrupt_metadata_records = 1;
        storage_health.child_sessions.corrupt_transcripts = 1;
        storage_health.child_sessions.orphan_metadata_records = 1;
        storage_health.child_sessions.orphan_transcripts = 1;
        storage_health
            .child_sessions
            .workflow_children_without_transcripts = 1;

        let report = run_doctor(
            &config(),
            TurnContext {
                cwd: "/tmp".to_string(),
                current_date: "2026-04-09".to_string(),
                git_branch: Some("main".to_string()),
                git_status: Some("clean".to_string()),
                ..Default::default()
            },
            5,
            0,
            0,
            None,
            &AuthOverview {
                store_path: Path::new("/tmp/auth.json").to_path_buf(),
                entries: Vec::new(),
            },
            storage_health,
        )
        .await
        .expect("run doctor");

        let storage = report
            .checks
            .iter()
            .find(|check| check.name == "session_storage")
            .expect("session_storage check present");
        assert_eq!(storage.status, DoctorStatus::Warn);
        assert!(storage.detail.contains("child sessions: 3 metadata"));
        assert!(storage.detail.contains("2 transcript"));
        assert!(storage.detail.contains("1 journal-only workflow"));
        assert!(storage.detail.contains("corrupt child-session metadata"));
        assert!(storage.detail.contains("corrupt child transcript"));
        assert!(storage.detail.contains("missing parent"));
        assert!(storage.detail.contains("no metadata"));
    }

    #[tokio::test]
    async fn provider_probe_is_skipped_and_does_not_block_other_checks() {
        let report = run_doctor(
            &config(),
            TurnContext {
                cwd: "/tmp".to_string(),
                current_date: "2026-04-09".to_string(),
                ..Default::default()
            },
            0,
            0,
            1,
            None,
            &AuthOverview {
                store_path: Path::new("/tmp/auth.json").to_path_buf(),
                entries: Vec::new(),
            },
            SessionStorageHealth {
                writable: true,
                ..Default::default()
            },
        )
        .await
        .expect("run doctor");

        let probe = report
            .checks
            .iter()
            .find(|check| check.name == "provider_probe")
            .expect("provider_probe check present");
        assert_eq!(probe.status, DoctorStatus::Warn);
        assert!(probe.detail.contains("ORBCODE_DOCTOR_PROBE"));
        assert_ne!(probe.status, DoctorStatus::Fail);
        assert!(report.checks.iter().any(|check| check.name == "auth"));
    }

    #[tokio::test]
    async fn fails_when_transcript_dir_is_not_writable() {
        let report = run_doctor(
            &config(),
            TurnContext::default(),
            0,
            0,
            0,
            None,
            &AuthOverview {
                store_path: Path::new("/tmp/auth.json").to_path_buf(),
                entries: Vec::new(),
            },
            SessionStorageHealth {
                project_dir: Path::new("/var/empty").to_path_buf(),
                writable: false,
                write_probe_error: Some("permission denied".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("run doctor");

        let storage = report
            .checks
            .iter()
            .find(|check| check.name == "session_storage")
            .expect("session_storage check present");
        assert_eq!(storage.status, DoctorStatus::Fail);
        assert!(storage.detail.contains("not writable"));
        assert!(storage.detail.contains("permission denied"));
    }

    #[test]
    fn truthy_parsing_accepts_common_enable_values() {
        for value in ["1", "true", "TRUE", "yes", "On"] {
            assert!(is_truthy(value), "{value} should enable the probe");
        }
        for value in ["", "0", "false", "no", "off", "maybe"] {
            assert!(!is_truthy(value), "{value} should not enable the probe");
        }
    }

    #[tokio::test]
    async fn chatgpt_subscription_check_reports_fixed_responses_path_and_default_model() {
        let home = tempfile::tempdir().expect("home");
        std::fs::write(
            home.path().join("auth.json"),
            format!(
                r#"{{"entries":[{{"provider":"openai","method":"chatgpt","source":{{"kind":"chatgpt_oauth","credentials":{{"id_token":"id","access_token":"access","refresh_token":"refresh","expires_at":{},"account_id":"account-123","email":null,"plan_type":"plus"}}}},"updated_at":"2026-08-03T00:00:00Z"}}]}}"#,
                chrono::Utc::now().timestamp_millis() + 60 * 60 * 1000
            ),
        )
        .expect("auth store");
        let mut config = config();
        config.home_dir = home.path().to_path_buf();
        config.default_provider = ProviderId::OpenAi;
        let auth = AuthOverview {
            store_path: home.path().join("auth.json"),
            entries: vec![AuthStatusEntry {
                provider: ProviderId::OpenAi,
                method: AuthMethod::ChatGpt,
                source_summary: "chatgpt oauth (ready; subscription:plus)".to_string(),
                persisted: true,
                usable: true,
                active: true,
                updated_at: None,
            }],
        };

        let check = chatgpt_subscription_check(&config, &auth).await;
        assert_eq!(check.status, DoctorStatus::Pass);
        assert!(check.detail.contains("gpt-5.6-sol"));
        assert!(check.detail.contains("fixed ChatGPT Codex Responses"));
        let model = model_capabilities_check(&config, &auth);
        assert!(model.detail.contains("model=gpt-5.6-sol"));
    }
}
