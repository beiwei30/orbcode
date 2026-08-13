use std::path::{Path, PathBuf};

use orbcode_app_server_protocol::{
    MemoryFileOverview, MemoryOverview, PolicyConflictOverview, PolicyOverview,
    PolicySourceOverview, StatusOverview, WorkspaceDiff,
};
use orbcode_config::{
    AgentSource, AppConfig, ContributedHookSource, HookDiscovery, HookProvenance, ManagedOrigin,
    SettingsSource, StrictPluginOnly, load_agent_definitions_with_warnings, load_plugin_registry,
    plugin_contributed_hooks, sanitize_path,
};
use orbcode_core::CoreError;
use tokio::process::Command as TokioCommand;

use super::AppServer;
use super::doctor::run_doctor;
use super::settings::{auto_memory_enabled, capability_names};
use crate::protocol_conversion::status_auth_overview_to_wire;

const UNTRACKED_DIFF_MAX_BYTES: u64 = 256 * 1024;

impl AppServer {
    pub async fn status_overview(&self, session_id: &str) -> Result<StatusOverview, CoreError> {
        let permission_context = self.sessions.permission_context_for_session(session_id);
        let permission_overview = self.permission_overview_for_session(Some(session_id)).await;
        let session_count = self.sessions.list_sessions().await?.len();
        let background_job_count = self.background.list_jobs().await?.len();
        let mcp_server_count = self.mcp.list_servers().await.len();
        let enabled_mcp_capability_count = self
            .mcp
            .capabilities()
            .await
            .into_iter()
            .filter(|capability| capability.enabled)
            .count();
        let available_tool_count = self
            .tools
            .provider_definitions(true, true)
            .into_iter()
            .filter(|tool| permission_context.tool_visible(&tool.name))
            .count();
        let config = self.sessions.effective_config_for_session(session_id);
        let model_resolution = config.provider_model_resolution(config.default_provider);
        let small_fast_resolution = config.small_fast_model_resolution(config.default_provider);
        Ok(StatusOverview {
            session_id: session_id.to_string(),
            active_permission_preset: self.sessions.session_permission_preset(session_id)?,
            cwd: config.cwd.clone(),
            home_dir: config.home_dir.clone(),
            model_display_name: model_resolution.display_name.clone(),
            model_name: model_resolution.request_model.clone(),
            model_capabilities: capability_names(&model_resolution),
            small_fast_model_display_name: small_fast_resolution.display_name,
            effort_level: self.sessions.runtime_effort_override(),
            max_thinking_tokens: self.sessions.max_thinking_tokens(),
            default_provider: config.default_provider,
            fallback_provider: config.fallback_provider,
            max_retries: config.max_retries,
            sandbox_mode: config.sandbox_mode.as_str().to_string(),
            sandbox_allow_network: config.sandbox_allow_network,
            permissions: permission_overview,
            auth: status_auth_overview_to_wire(self.auth.overview().await?),
            persisted_session_count: session_count,
            background_job_count,
            available_tool_count,
            configured_mcp_server_count: mcp_server_count,
            enabled_mcp_capability_count,
            policy: policy_overview(&config),
        })
    }

    pub async fn workspace_diff(&self) -> Result<WorkspaceDiff, CoreError> {
        let cwd = self.sessions.effective_config().cwd;
        ensure_git_worktree(&cwd).await?;
        let status = git_output(&cwd, &["status", "--short"]).await?;
        let staged_diff = git_output(
            &cwd,
            &["diff", "--cached", "--no-ext-diff", "--color=never", "--"],
        )
        .await?;
        let mut unstaged_diff =
            git_output(&cwd, &["diff", "--no-ext-diff", "--color=never", "--"]).await?;
        let untracked_files = parse_untracked_files(&status);
        let untracked_diff = untracked_files_diff(&cwd, &untracked_files).await?;
        if !untracked_diff.is_empty() {
            if !unstaged_diff.is_empty() {
                unstaged_diff.push('\n');
            }
            unstaged_diff.push_str(&untracked_diff);
        }

        Ok(WorkspaceDiff {
            cwd,
            status,
            staged_diff,
            unstaged_diff,
            untracked_files,
        })
    }

    pub async fn memory_overview(&self) -> Result<MemoryOverview, CoreError> {
        let config = self.sessions.effective_config();
        let context = self.sessions.context_preview().await;
        let mut memories = context
            .memory_sources
            .iter()
            .filter_map(memory_source_overview)
            .collect::<Vec<_>>();
        if memories.is_empty() {
            memories.push(
                memory_file_overview("User memory", config.home_dir.join("CLAUDE.md")).await?,
            );
        }
        let user_memory = memories
            .iter()
            .find(|memory| memory.label == "User memory")
            .cloned()
            .unwrap_or_else(|| MemoryFileOverview {
                label: "User memory".to_string(),
                path: config.home_dir.join("CLAUDE.md"),
                exists: false,
                content: None,
                status: orbcode_protocol::MemorySourceStatus::Missing,
                writable: true,
                trust_boundary: Some("private user".to_string()),
                scope: None,
                skipped_reason: None,
            });
        let project_memories = memories
            .into_iter()
            .filter(|memory| memory.path != user_memory.path)
            .collect();
        Ok(MemoryOverview {
            user_memory,
            project_memories,
            auto_memory_enabled: auto_memory_enabled(),
            auto_memory_dir: config
                .home_dir
                .join("projects")
                .join(sanitize_path(&config.cwd.display().to_string()))
                .join("memory"),
        })
    }

    pub async fn ensure_memory_file(&self, path: PathBuf) -> Result<(), CoreError> {
        let overview = self.memory_overview().await?;
        let writable_target = std::iter::once(&overview.user_memory)
            .chain(overview.project_memories.iter())
            .any(|memory| memory.writable && memory.path == path);
        if !writable_target {
            return Err(CoreError::PermissionDenied(format!(
                "{} is not a writable memory source",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if tokio::fs::try_exists(&path).await? {
            return Ok(());
        }
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn doctor_report(&self) -> Result<super::DoctorReport, CoreError> {
        let context = self.sessions.context_preview().await;
        let session_count = self.sessions.list_sessions().await?.len();
        let background_job_count = self.background.list_jobs().await?.len();
        let mcp_server_count = self.mcp.list_servers().await.len();
        let auth = self.auth.overview().await?;
        let config = self.sessions.effective_config();
        let storage_health = self.sessions.session_storage_health().await;
        run_doctor(
            &config,
            context,
            session_count,
            background_job_count,
            mcp_server_count,
            Some(&self.mcp),
            &auth,
            storage_health,
        )
        .await
    }

    pub async fn cleanup_orphan_child_sessions(
        &self,
        dry_run: bool,
        stale_running_cutoff_ms: Option<i64>,
    ) -> Result<orbcode_core::ChildSessionOrphanCleanupResult, CoreError> {
        self.sessions
            .cleanup_orphan_child_sessions(dry_run, stale_running_cutoff_ms)
            .await
    }

    pub async fn hook_discovery(&self) -> HookDiscovery {
        let config = self.sessions.effective_config();
        let home = &config.home_dir;
        let cwd = &config.cwd;
        let mut contributed: Vec<ContributedHookSource> = Vec::new();
        if let Ok(outcome) = load_agent_definitions_with_warnings(home, cwd).await {
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
        if let Ok(registry) = load_plugin_registry(home, cwd).await {
            let (plugin_hooks, _warnings) =
                plugin_contributed_hooks(&registry, config.trusted_project);
            contributed.extend(plugin_hooks);
        }
        orbcode_config::discover_hooks(
            &config.settings_layers,
            &config.policy,
            config.trusted_project,
            &contributed,
        )
    }
}

fn policy_overview(config: &AppConfig) -> PolicyOverview {
    let policy = &config.policy;
    let managed_layer = config.settings_layers.get(SettingsSource::Managed);
    let managed_paths = managed_layer
        .map(|layer| layer.contributing_paths.clone())
        .unwrap_or_default();
    let managed_origin = policy.managed_origin.map(|origin| match origin {
        ManagedOrigin::File => "file".to_string(),
        ManagedOrigin::DropIn => "drop-in".to_string(),
        ManagedOrigin::FileAndDropIn => "file + drop-in".to_string(),
    });
    let strict = policy
        .strict_plugin_only_customization
        .as_ref()
        .map(|strict| match strict {
            StrictPluginOnly::All => "all".to_string(),
            StrictPluginOnly::Surfaces(surfaces) => surfaces.join(", "),
        });
    let settings_sources = config
        .settings_layers
        .layers
        .iter()
        .map(|layer| PolicySourceOverview {
            source: layer.source.short_label().to_string(),
            primary_path: layer.primary_path.clone(),
            present: layer.is_present(),
            read_only: layer.source.is_read_only(),
            error_count: layer.errors.len(),
        })
        .collect();
    let conflicts = config
        .policy_conflicts
        .iter()
        .map(|conflict| PolicyConflictOverview {
            source: conflict.source.short_label().to_string(),
            source_path: conflict.source_path.clone(),
            message: conflict.message.clone(),
        })
        .collect();

    PolicyOverview {
        managed_origin,
        managed_paths,
        available_models: policy.available_models.clone(),
        allowed_mcp_servers: policy.allowed_mcp_servers.as_ref().map(Vec::len),
        denied_mcp_servers: policy.denied_mcp_servers.len(),
        allow_managed_hooks_only: policy.allow_managed_hooks_only,
        allow_managed_permission_rules_only: policy.allow_managed_permission_rules_only,
        allow_managed_mcp_servers_only: policy.allow_managed_mcp_servers_only,
        disable_bypass_permissions_mode: policy.disable_bypass_permissions_mode,
        strict_plugin_only_customization: strict,
        force_login_method: policy.force_login_method.clone(),
        effective_model_source: policy
            .effective_model
            .as_ref()
            .map(|value| value.source.short_label().to_string()),
        conflicts,
        settings_sources,
    }
}

async fn memory_file_overview(
    label: impl Into<String>,
    path: PathBuf,
) -> Result<MemoryFileOverview, CoreError> {
    let exists = tokio::fs::try_exists(&path).await?;
    let content = if exists {
        let content = tokio::fs::read_to_string(&path).await?;
        let content = content.trim().to_string();
        if content.is_empty() {
            None
        } else {
            Some(content)
        }
    } else {
        None
    };
    let status = if !exists {
        orbcode_protocol::MemorySourceStatus::Missing
    } else if content.is_none() {
        orbcode_protocol::MemorySourceStatus::Empty
    } else {
        orbcode_protocol::MemorySourceStatus::Loaded
    };
    Ok(MemoryFileOverview {
        label: label.into(),
        path,
        exists,
        content,
        status,
        writable: true,
        trust_boundary: None,
        scope: None,
        skipped_reason: None,
    })
}

fn memory_source_overview(source: &orbcode_protocol::MemorySource) -> Option<MemoryFileOverview> {
    let path = source.path.as_ref().map(PathBuf::from)?;
    Some(MemoryFileOverview {
        label: source.label.clone(),
        exists: !matches!(source.status, orbcode_protocol::MemorySourceStatus::Missing),
        path,
        content: source.content.clone(),
        status: source.status,
        writable: source.writable,
        trust_boundary: source.trust_boundary.clone(),
        scope: source.scope.clone(),
        skipped_reason: source.skipped_reason.clone(),
    })
}

async fn ensure_git_worktree(cwd: &PathBuf) -> Result<(), CoreError> {
    let output = git_output(cwd, &["rev-parse", "--is-inside-work-tree"]).await?;
    if output.trim() == "true" {
        Ok(())
    } else {
        Err(CoreError::Config(format!(
            "{} is not inside a git worktree",
            cwd.display()
        )))
    }
}

async fn git_output(cwd: &PathBuf, args: &[&str]) -> Result<String, CoreError> {
    let output = TokioCommand::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() {
        format!("git {} exited with {}", args.join(" "), output.status)
    } else {
        format!("git {} failed: {stderr}", args.join(" "))
    };
    Err(CoreError::Config(detail))
}

fn parse_untracked_files(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| line.strip_prefix("?? ").map(str::to_string))
        .collect()
}

async fn untracked_files_diff(cwd: &Path, files: &[String]) -> Result<String, CoreError> {
    let mut sections = Vec::new();
    for file in files {
        let path = cwd.join(file);
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => continue,
        };
        if metadata.len() > UNTRACKED_DIFF_MAX_BYTES {
            sections.push(format!(
                "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@ -0,0 +1 @@\n+<untracked file omitted: larger than 256 KiB>"
            ));
            continue;
        }
        let bytes = tokio::fs::read(&path).await?;
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => {
                sections.push(format!(
                    "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@ -0,0 +1 @@\n+<binary untracked file omitted>"
                ));
                continue;
            }
        };
        let line_count = content.lines().count().max(1);
        let mut section = format!(
            "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@ -0,0 +1,{line_count} @@"
        );
        if content.is_empty() {
            section.push_str("\n+");
        } else {
            for line in content.lines() {
                section.push('\n');
                section.push('+');
                section.push_str(line);
            }
        }
        sections.push(section);
    }
    Ok(sections.join("\n"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_config::AppConfigOverrides;
    use orbcode_core::CoreError;

    use super::super::AppServer;

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-app-server-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn memory_overview_deduplicates_user_memory_from_project_ancestors() {
        let fake_user_home = test_path("memory-dedupe-user-home");
        let claude_home = fake_user_home.join(".claude");
        let cwd = fake_user_home.join("project");
        tokio::fs::create_dir_all(&claude_home)
            .await
            .expect("claude home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::write(claude_home.join("CLAUDE.md"), "User memory\n")
            .await
            .expect("user memory");
        tokio::fs::write(cwd.join("CLAUDE.md"), "Project memory\n")
            .await
            .expect("project memory");

        let app = AppServer::new(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(claude_home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let overview = app.memory_overview().await.expect("memory overview");
        let user_path = std::fs::canonicalize(claude_home.join("CLAUDE.md")).expect("user path");

        assert_eq!(overview.user_memory.path, claude_home.join("CLAUDE.md"));
        assert!(!overview
            .project_memories
            .iter()
            .any(|memory| std::fs::canonicalize(&memory.path).ok().as_ref() == Some(&user_path)));
        assert!(
            overview
                .project_memories
                .iter()
                .any(|memory| memory.path == cwd.join("CLAUDE.md"))
        );
    }

    #[tokio::test]
    async fn ensure_memory_file_rejects_non_writable_sources() {
        let home = test_path("memory-read-only-home");
        let cwd = test_path("memory-read-only-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let error = app
            .ensure_memory_file(orbcode_config::managed_memory_file())
            .await
            .expect_err("managed memory must be read-only");

        assert!(matches!(error, CoreError::PermissionDenied(_)));
    }
}
