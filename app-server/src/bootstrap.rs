use orbcode_app_server_protocol::{BootstrapParams, BootstrapState, McpSlashSuggestionCatalog};
use orbcode_core::CoreError;
use orbcode_protocol::{SessionRecord, StreamEvent};

use super::AppServer;

impl AppServer {
    pub async fn bootstrap(
        &self,
        requested_session: Option<&str>,
    ) -> Result<BootstrapState, CoreError> {
        self.bootstrap_with_params(BootstrapParams {
            session_id: requested_session.map(str::to_string),
            ..BootstrapParams::default()
        })
        .await
    }

    pub async fn bootstrap_with_params(
        &self,
        params: BootstrapParams,
    ) -> Result<BootstrapState, CoreError> {
        let requested_session = params.session_id.clone();
        if params.read_only {
            let Some(session_id) = requested_session.as_deref() else {
                return Err(CoreError::Config(
                    "read-only bootstrap requires session_id".into(),
                ));
            };
            let (session, bootstrap_event) =
                self.sessions.load_session_for_view(session_id).await?;
            return self.bootstrap_state(session, bootstrap_event, false).await;
        }
        let can_load_child_session = requested_session.is_some()
            && params.cwd.is_none()
            && params.additional_directories.is_empty()
            && params.session_mcp_servers.is_empty();
        let result = self
            .sessions
            .start_or_resume_with_setup(
                params.session_id.as_deref(),
                params.cwd,
                params.additional_directories,
                params.session_mcp_servers,
            )
            .await;
        let (session, bootstrap_event) = match result {
            Ok(value) => value,
            Err(CoreError::SessionNotFound(_)) if can_load_child_session => {
                self.sessions
                    .load_child_session_record(requested_session.as_deref().expect("session id"))
                    .await?
            }
            Err(error) => return Err(error),
        };
        self.set_active_session_id(&session.session_id);
        self.bootstrap_state(session, bootstrap_event, true).await
    }

    pub async fn acp_load_setup(
        &self,
        params: BootstrapParams,
    ) -> Result<BootstrapState, CoreError> {
        let Some(session_id) = params.session_id else {
            return Err(CoreError::Config(
                "session/acp_load_setup requires session_id".into(),
            ));
        };
        let Some(cwd) = params.cwd else {
            return Err(CoreError::Config(
                "session/acp_load_setup requires cwd".into(),
            ));
        };
        let (session, bootstrap_event) = self
            .sessions
            .load_session_with_setup(
                &session_id,
                cwd,
                params.additional_directories,
                params.session_mcp_servers,
            )
            .await?;
        self.set_active_session_id(&session.session_id);
        self.bootstrap_state(session, bootstrap_event, false).await
    }

    pub async fn acp_resume_setup(
        &self,
        params: BootstrapParams,
    ) -> Result<BootstrapState, CoreError> {
        let Some(session_id) = params.session_id else {
            return Err(CoreError::Config(
                "session/acp_resume_setup requires session_id".into(),
            ));
        };
        let Some(cwd) = params.cwd else {
            return Err(CoreError::Config(
                "session/acp_resume_setup requires cwd".into(),
            ));
        };
        let (session, bootstrap_event) = self
            .sessions
            .load_session_with_setup(
                &session_id,
                cwd,
                params.additional_directories,
                params.session_mcp_servers,
            )
            .await?;
        self.set_active_session_id(&session.session_id);
        self.bootstrap_state(session, bootstrap_event, false).await
    }

    /// Build a `BootstrapState`. `enumerate_mcp` controls whether the (live,
    /// per-server) MCP tool/resource enumeration runs. It is required when
    /// starting or resuming a session, but can be skipped for an in-session
    /// rewind where the MCP servers, trust, and cwd are unchanged — sparing the
    /// caller a round-trip to every configured server.
    pub(crate) async fn bootstrap_state(
        &self,
        session: SessionRecord,
        bootstrap_event: StreamEvent,
        enumerate_mcp: bool,
    ) -> Result<BootstrapState, CoreError> {
        let config = self.sessions.effective_config();

        let prompt_history = self
            .sessions
            .prompt_history_for_session(&session.session_id, 100)
            .await?;

        let (
            available_tool_count,
            configured_mcp_server_count,
            enabled_mcp_capability_count,
            mcp_slash_suggestions,
        ) = if enumerate_mcp {
            let server_count = self
                .mcp
                .list_servers_for_session(&session.session_id)
                .await
                .len();
            let capability_count = self
                .mcp
                .capabilities()
                .await
                .into_iter()
                .filter(|capability| capability.enabled)
                .count();
            let tool_count = self
                .tools
                .provider_definitions_with_mcp_for_session(
                    true,
                    true,
                    &self.mcp,
                    &session.session_id,
                )
                .await
                .into_iter()
                .filter(|tool| self.sessions.permission_context().tool_visible(&tool.name))
                .count();
            let suggestions = self
                .mcp_slash_suggestions_for_session(&session.session_id)
                .await;
            (tool_count, server_count, capability_count, suggestions)
        } else {
            (0, 0, 0, McpSlashSuggestionCatalog::default())
        };

        Ok(BootstrapState {
            session,
            bootstrap_event,
            prompt_history,
            available_tool_count,
            configured_mcp_server_count,
            enabled_mcp_capability_count,
            home_dir: config.home_dir.clone(),
            cwd: config.cwd.clone(),
            model_display_name: self.sessions.model_display_name(),
            context_window_options: config.context_window_options(),
            max_output_token_options: config.max_output_token_options(),
            token_warning_options: config.token_warning_options(),
            theme: self.sessions.theme_setting(),
            editor_mode: self.sessions.editor_mode_setting(),
            default_provider: config.default_provider,
            fallback_provider: config.fallback_provider,
            max_retries: config.max_retries,
            permissions: self.sessions.permission_context(),
            mcp_slash_suggestions,
            statusline_command: config.settings.statusline_command.clone(),
            statusline_refresh_interval_secs: config
                .settings
                .statusline_refresh_interval_secs
                .unwrap_or(30),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_app_server_protocol::BootstrapParams;
    use orbcode_config::{AppConfigOverrides, sanitize_path};
    use orbcode_mcp::{McpAuth, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport};
    use serde_json::json;

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
    async fn bootstrap_resume_uses_transcript_cwd_for_runtime_context() {
        let home = test_path("resume-context-home");
        let launch_cwd = test_path("resume-context-launch");
        let resumed_cwd = test_path("resume-context-resumed");
        let extra = test_path("resume-context-extra");
        let relative_extra = format!(
            "../{}",
            extra
                .file_name()
                .and_then(|name| name.to_str())
                .expect("extra filename")
        );
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&launch_cwd)
            .await
            .expect("launch cwd");
        tokio::fs::create_dir_all(&resumed_cwd)
            .await
            .expect("resumed cwd");
        tokio::fs::create_dir_all(&extra).await.expect("extra");
        tokio::fs::write(resumed_cwd.join("CLAUDE.md"), "resumed project memory")
            .await
            .expect("resumed memory");

        let project_dir = home
            .join("projects")
            .join(sanitize_path(&launch_cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        let session_id = "resume-context-session";
        let payload = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-04-10T00:00:00.000Z",
            "message": { "role": "user", "content": "resume context" },
            "cwd": resumed_cwd.display().to_string(),
            "sessionId": session_id,
        }))
        .expect("serialize transcript");
        tokio::fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!("{payload}\n"),
        )
        .await
        .expect("write transcript");

        let app = AppServer::new(
            launch_cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let bootstrap = app.bootstrap(Some(session_id)).await.expect("bootstrap");
        assert_eq!(bootstrap.cwd, resumed_cwd);
        assert_eq!(
            app.status_overview(session_id).await.expect("status").cwd,
            resumed_cwd
        );
        let context = app.context_preview().await;
        assert_eq!(context.cwd, resumed_cwd.display().to_string());
        assert!(
            context
                .claude_md
                .as_deref()
                .is_some_and(|contents| contents.contains("resumed project memory"))
        );
        let candidate = app
            .validate_add_directory(&relative_extra)
            .await
            .expect("relative add-dir should resolve from resumed cwd");
        assert_eq!(
            candidate.path,
            std::fs::canonicalize(extra).expect("canonical extra")
        );
    }

    #[tokio::test]
    async fn bootstrap_can_open_workflow_child_session_view() {
        let home = test_path("workflow-child-home");
        let launch_cwd = test_path("workflow-child-launch");
        let session_cwd = test_path("workflow-child-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&launch_cwd)
            .await
            .expect("launch cwd");
        tokio::fs::create_dir_all(&session_cwd)
            .await
            .expect("session cwd");

        let parent_session_id = "parent-session";
        let run_id = "workflow-test";
        let step_key = "step.1.0";
        let child_session_id =
            format!("{parent_session_id}:{run_id}:agent-11111111111111111111111111111111");
        let source_tool_use_id = format!("workflow:{run_id}:{step_key}");
        let project_dir = home
            .join("projects")
            .join(sanitize_path(&session_cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");

        let user_line = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "parent-user-1",
            "timestamp": "2026-04-10T00:00:00.000Z",
            "message": { "role": "user", "content": "run workflow" },
            "cwd": session_cwd.display().to_string(),
            "sessionId": parent_session_id,
        }))
        .expect("serialize user line");
        let progress_line = serde_json::to_string(&json!({
            "type": "progress",
            "uuid": "child-progress-1",
            "timestamp": "2026-04-10T00:00:01.000Z",
            "parentToolUseID": source_tool_use_id,
            "data": {
                "type": "agent_progress",
                "agentId": "agent-11111111111111111111111111111111",
                "message": {
                    "type": "user",
                    "message": {
                        "role": "user",
                        "content": "task2 prompt"
                    }
                }
            },
            "cwd": session_cwd.display().to_string(),
            "sessionId": parent_session_id,
        }))
        .expect("serialize progress line");
        tokio::fs::write(
            project_dir.join(format!("{parent_session_id}.jsonl")),
            format!("{user_line}\n{progress_line}\n"),
        )
        .await
        .expect("write transcript");

        let child_metadata_dir = home.join("sessions").join("agents");
        tokio::fs::create_dir_all(&child_metadata_dir)
            .await
            .expect("child metadata dir");
        let child_metadata = serde_json::to_string_pretty(&json!({
            "childSessionId": child_session_id.clone(),
            "parentSessionId": parent_session_id,
            "agentId": "agent-11111111111111111111111111111111",
            "agentType": "general-purpose",
            "sourceToolUseId": source_tool_use_id,
            "cwd": session_cwd.display().to_string(),
            "model": "test-model",
            "permissionMode": null,
            "promptPreview": "task2 prompt",
            "status": "completed",
            "startedAt": 1775750400000i64,
            "endedAt": 1775750402000i64,
            "lastActivityAt": 1775750402000i64
        }))
        .expect("serialize child metadata");
        tokio::fs::write(
            child_metadata_dir
                .join("parent-session_workflow-test_agent-11111111111111111111111111111111.json"),
            child_metadata,
        )
        .await
        .expect("write child metadata");

        let journal_dir = home.join("workflow-runs").join(run_id);
        tokio::fs::create_dir_all(&journal_dir)
            .await
            .expect("journal dir");
        let journal_line = serde_json::to_string(&json!({
            "timestamp": "2026-04-10T00:00:02.000Z",
            "event": "step_completed",
            "step_key": step_key,
            "output": "task2 output"
        }))
        .expect("serialize journal line");
        tokio::fs::write(
            journal_dir.join("journal.jsonl"),
            format!("{journal_line}\n"),
        )
        .await
        .expect("write journal");

        let app = AppServer::new(
            launch_cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let bootstrap = app
            .bootstrap(Some(&child_session_id))
            .await
            .expect("bootstrap child session");
        assert_eq!(bootstrap.session.session_id, child_session_id);
        assert_eq!(
            bootstrap.session.cwd.as_deref(),
            Some(session_cwd.to_string_lossy().as_ref())
        );
        assert_eq!(bootstrap.session.messages.len(), 2);
        assert_eq!(bootstrap.session.messages[0].content, "task2 prompt");
        assert_eq!(bootstrap.session.messages[1].content, "task2 output");
    }

    #[tokio::test]
    async fn read_only_child_view_does_not_mutate_live_runtime_cwd() {
        let home = test_path("child-view-readonly-home");
        let launch_cwd = test_path("child-view-readonly-launch");
        let session_cwd = test_path("child-view-readonly-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&launch_cwd)
            .await
            .expect("launch cwd");
        tokio::fs::create_dir_all(&session_cwd)
            .await
            .expect("session cwd");

        let parent_session_id = "parent-session";
        let run_id = "workflow-readonly";
        let step_key = "step.1.0";
        let agent_id = "agent-33333333333333333333333333333333";
        let child_session_id = format!("{parent_session_id}:{run_id}:{agent_id}");
        let source_tool_use_id = format!("workflow:{run_id}:{step_key}");
        let project_dir = home
            .join("projects")
            .join(sanitize_path(&session_cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");

        let user_line = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "parent-user-1",
            "timestamp": "2026-04-10T00:00:00.000Z",
            "message": { "role": "user", "content": "run workflow" },
            "cwd": session_cwd.display().to_string(),
            "sessionId": parent_session_id,
        }))
        .expect("serialize user line");
        let progress_line = serde_json::to_string(&json!({
            "type": "progress",
            "uuid": "child-progress-1",
            "timestamp": "2026-04-10T00:00:01.000Z",
            "parentToolUseID": source_tool_use_id,
            "data": {
                "type": "agent_progress",
                "agentId": agent_id,
                "message": {
                    "type": "user",
                    "message": { "role": "user", "content": "task prompt" }
                }
            },
            "cwd": session_cwd.display().to_string(),
            "sessionId": parent_session_id,
        }))
        .expect("serialize progress line");
        tokio::fs::write(
            project_dir.join(format!("{parent_session_id}.jsonl")),
            format!("{user_line}\n{progress_line}\n"),
        )
        .await
        .expect("write transcript");

        let child_metadata_dir = home.join("sessions").join("agents");
        tokio::fs::create_dir_all(&child_metadata_dir)
            .await
            .expect("child metadata dir");
        let child_metadata = serde_json::to_string_pretty(&json!({
            "childSessionId": child_session_id.clone(),
            "parentSessionId": parent_session_id,
            "agentId": agent_id,
            "agentType": "general-purpose",
            "sourceToolUseId": source_tool_use_id,
            "cwd": session_cwd.display().to_string(),
            "model": "test-model",
            "permissionMode": null,
            "promptPreview": "task prompt",
            "status": "completed",
            "startedAt": 1775750400000i64,
            "endedAt": 1775750402000i64,
            "lastActivityAt": 1775750402000i64
        }))
        .expect("serialize child metadata");
        tokio::fs::write(
            child_metadata_dir.join(format!("{}.json", child_session_id.replace(':', "_"))),
            child_metadata,
        )
        .await
        .expect("write child metadata");

        let app = AppServer::new(
            launch_cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        // Start a fresh live session; runtime cwd tracks the launch cwd.
        app.bootstrap(None).await.expect("start live session");
        let live_cwd_before = app.context_preview().await.cwd;
        assert_ne!(
            live_cwd_before,
            session_cwd.display().to_string(),
            "precondition: live cwd differs from the child session cwd"
        );

        // Opening a child step's output read-only must NOT retarget the live
        // session's runtime cwd to the child session's cwd.
        let bootstrap = app
            .bootstrap_with_params(BootstrapParams {
                session_id: Some(child_session_id.clone()),
                read_only: true,
                ..BootstrapParams::default()
            })
            .await
            .expect("read-only child view");
        assert_eq!(bootstrap.session.session_id, child_session_id);
        assert_eq!(
            bootstrap.session.cwd.as_deref(),
            Some(session_cwd.to_string_lossy().as_ref())
        );
        assert_eq!(
            app.context_preview().await.cwd,
            live_cwd_before,
            "read-only child view must not mutate the live runtime cwd"
        );
    }

    #[tokio::test]
    async fn bootstrap_prefers_persisted_workflow_child_transcript() {
        let home = test_path("workflow-child-persisted-home");
        let launch_cwd = test_path("workflow-child-persisted-launch");
        let session_cwd = test_path("workflow-child-persisted-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&launch_cwd)
            .await
            .expect("launch cwd");
        tokio::fs::create_dir_all(&session_cwd)
            .await
            .expect("session cwd");

        let parent_session_id = "parent-session";
        let run_id = "workflow-persisted";
        let step_key = "step.0";
        let agent_id = "agent-22222222222222222222222222222222";
        let child_session_id = format!("{parent_session_id}:{run_id}:{agent_id}");
        let source_tool_use_id = format!("workflow:{run_id}:{step_key}");

        let child_metadata_dir = home.join("sessions").join("agents");
        tokio::fs::create_dir_all(&child_metadata_dir)
            .await
            .expect("child metadata dir");
        let child_metadata = serde_json::to_string_pretty(&json!({
            "childSessionId": child_session_id.clone(),
            "parentSessionId": parent_session_id,
            "agentId": agent_id,
            "agentType": "general-purpose",
            "sourceToolUseId": source_tool_use_id,
            "cwd": session_cwd.display().to_string(),
            "model": "test-model",
            "permissionMode": null,
            "promptPreview": "persisted child prompt",
            "status": "completed",
            "startedAt": 1775750400000i64,
            "endedAt": 1775750402000i64,
            "lastActivityAt": 1775750402000i64
        }))
        .expect("serialize child metadata");
        let sanitized_child_session_id = child_session_id.replace(':', "_");
        tokio::fs::write(
            child_metadata_dir.join(format!("{sanitized_child_session_id}.json")),
            child_metadata,
        )
        .await
        .expect("write child metadata");

        let child_transcript_path = child_metadata_dir
            .join("transcripts")
            .join(format!("{sanitized_child_session_id}.jsonl"));
        tokio::fs::create_dir_all(
            child_transcript_path
                .parent()
                .expect("child transcript parent"),
        )
        .await
        .expect("child transcript dir");
        let child_user_line = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "child-user-1",
            "timestamp": "2026-04-10T00:00:01.000Z",
            "message": { "role": "user", "content": "persisted child prompt" },
            "cwd": session_cwd.display().to_string(),
            "sessionId": child_session_id,
        }))
        .expect("serialize child user line");
        let child_assistant_line = serde_json::to_string(&json!({
            "type": "assistant",
            "uuid": "child-assistant-1",
            "parentUuid": "child-user-1",
            "timestamp": "2026-04-10T00:00:02.000Z",
            "message": { "role": "assistant", "content": "persisted child output" },
            "cwd": session_cwd.display().to_string(),
            "sessionId": child_session_id,
        }))
        .expect("serialize child assistant line");
        tokio::fs::write(
            &child_transcript_path,
            format!("{child_user_line}\n{child_assistant_line}\n"),
        )
        .await
        .expect("write child transcript");

        let project_dir = home
            .join("projects")
            .join(sanitize_path(&session_cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");
        let parent_progress_line = serde_json::to_string(&json!({
            "type": "progress",
            "uuid": "fallback-progress-1",
            "timestamp": "2026-04-10T00:00:03.000Z",
            "parentToolUseID": source_tool_use_id,
            "data": {
                "type": "agent_progress",
                "agentId": agent_id,
                "message": {
                    "type": "user",
                    "message": {
                        "role": "user",
                        "content": "fallback prompt should not appear"
                    }
                }
            },
            "cwd": session_cwd.display().to_string(),
            "sessionId": parent_session_id,
        }))
        .expect("serialize parent progress line");
        tokio::fs::write(
            project_dir.join(format!("{parent_session_id}.jsonl")),
            format!("{parent_progress_line}\n"),
        )
        .await
        .expect("write parent transcript");

        let journal_dir = home.join("workflow-runs").join(run_id);
        tokio::fs::create_dir_all(&journal_dir)
            .await
            .expect("journal dir");
        let journal_line = serde_json::to_string(&json!({
            "timestamp": "2026-04-10T00:00:04.000Z",
            "event": "step_completed",
            "step_key": step_key,
            "output": "fallback output should not appear"
        }))
        .expect("serialize journal line");
        tokio::fs::write(
            journal_dir.join("journal.jsonl"),
            format!("{journal_line}\n"),
        )
        .await
        .expect("write journal");

        let app = AppServer::new(
            launch_cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let bootstrap = app
            .bootstrap(Some(&child_session_id))
            .await
            .expect("bootstrap child session");
        assert_eq!(bootstrap.session.session_id, child_session_id);
        assert_eq!(
            bootstrap.session.cwd.as_deref(),
            Some(session_cwd.to_string_lossy().as_ref())
        );
        let contents = bootstrap
            .session
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            contents,
            vec!["persisted child prompt", "persisted child output"]
        );
        assert!(!contents.contains(&"fallback prompt should not appear"));
        assert!(!contents.contains(&"fallback output should not appear"));
    }

    #[tokio::test]
    async fn bootstrap_setup_overrides_cwd_add_dirs_and_session_mcp_without_persisting() {
        let home = test_path("setup-home");
        let launch_cwd = test_path("setup-launch");
        let session_cwd = test_path("setup-session");
        let extra = test_path("setup-extra");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&launch_cwd)
            .await
            .expect("launch cwd");
        tokio::fs::create_dir_all(&session_cwd)
            .await
            .expect("session cwd");
        tokio::fs::create_dir_all(&extra).await.expect("extra");

        let app = AppServer::new(
            launch_cwd,
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let bootstrap = app
            .bootstrap_with_params(BootstrapParams {
                cwd: Some(session_cwd.clone()),
                additional_directories: vec![extra.clone()],
                session_mcp_servers: vec![McpServerConfig {
                    id: "docs".to_string(),
                    transport: McpTransport::Stdio,
                    endpoint: "mock-mcp".to_string(),
                    args: vec!["--stdio".to_string()],
                    env: Default::default(),
                    cwd: None,
                    headers: Default::default(),
                    enabled: true,
                    status: McpServerStatus::Ready,
                    error: None,
                    summary: "Docs".to_string(),
                    auth: McpAuth::None,
                    trust: McpServerTrust::Unknown,
                    transport_type_hint: None,
                    source: None,
                }],
                ..BootstrapParams::default()
            })
            .await
            .expect("bootstrap");

        let session_cwd = std::fs::canonicalize(session_cwd).expect("canonical session cwd");
        let extra = std::fs::canonicalize(extra).expect("canonical extra");
        assert_eq!(bootstrap.cwd, session_cwd);
        assert_eq!(
            bootstrap.session.cwd.as_deref(),
            Some(session_cwd.to_string_lossy().as_ref())
        );
        assert_eq!(
            bootstrap.session.additional_directories,
            vec![extra.to_string_lossy().to_string()]
        );
        assert_eq!(
            app.context_preview().await.cwd,
            session_cwd.display().to_string()
        );
        assert!(app.permissions().additional_directories.contains(&extra));

        let global_servers = app.mcp.list_servers().await;
        assert!(
            global_servers.is_empty(),
            "session-scoped MCP servers must not be visible globally"
        );
        let servers = app
            .mcp
            .list_servers_for_session(&bootstrap.session.session_id)
            .await;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].id, "docs");
        assert_eq!(servers[0].trust, McpServerTrust::Unknown);
        assert!(
            !tokio::fs::try_exists(home.join("mcp").join("servers.json"))
                .await
                .expect("check MCP store"),
            "session-scoped MCP servers must not be persisted"
        );
    }
}
