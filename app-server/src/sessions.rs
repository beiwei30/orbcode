use orbcode_app_server_protocol::{
    AcpDeleteSessionParams, AcpLoadReplayPreflight, BootstrapState, ContextOverview,
    PermissionMode, SessionControlState, SessionModelOption,
};
use orbcode_core::{
    CompactDecision, CompactSessionResult, CoreError, CostOverview, PermissionDecision,
    ProviderRequestDebugSnapshot, StatsOverview, TurnInteractionContext, UsageOverview,
};
use orbcode_protocol::{
    EffortLevel, SessionRecord, SessionSummary, StreamEvent, TranscriptMessage, TurnContext,
};
use tokio::sync::mpsc;

use super::AppServer;
use crate::protocol_conversion::{permission_mode_from_wire, permission_mode_to_wire};

impl AppServer {
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, CoreError> {
        self.sessions.list_sessions().await
    }

    pub async fn session_id_for_exact_custom_title(
        &self,
        title: &str,
    ) -> Result<Option<String>, CoreError> {
        self.sessions.session_id_for_exact_custom_title(title).await
    }

    pub async fn rename_session(&self, session_id: &str, new_title: &str) -> Result<(), CoreError> {
        self.sessions.rename_session(session_id, new_title).await
    }

    pub async fn prompt_history(&self, limit: usize) -> Result<Vec<String>, CoreError> {
        self.sessions.prompt_history(limit).await
    }

    pub async fn prompt_history_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<String>, CoreError> {
        self.sessions
            .prompt_history_for_session(session_id, limit)
            .await
    }

    pub fn remove_last_prompt_history_entry(&self) {
        self.sessions.remove_last_prompt_history_entry();
    }

    pub async fn context_preview(&self) -> TurnContext {
        self.sessions.context_preview().await
    }

    pub async fn pre_user_instructions_preview(&self, session_id: &str) -> String {
        self.sessions
            .pre_user_instructions_preview(session_id)
            .await
    }

    pub async fn last_provider_request_snapshot(&self) -> Option<ProviderRequestDebugSnapshot> {
        self.sessions.last_provider_request_snapshot().await
    }

    pub async fn context_overview(&self, session_id: &str) -> Result<ContextOverview, CoreError> {
        let context = self.context_preview().await;
        let (usage, report) = self
            .sessions
            .context_usage_and_diagnostics(session_id, context.clone())
            .await?;
        Ok(ContextOverview {
            context,
            usage,
            report,
            max_thinking_tokens: self.sessions.max_thinking_tokens(),
        })
    }

    pub async fn usage_overview(&self, session_id: &str) -> Result<UsageOverview, CoreError> {
        self.sessions.usage_overview(session_id).await
    }

    pub async fn cost_overview(&self, session_id: &str) -> Result<CostOverview, CoreError> {
        self.sessions.cost_overview(session_id).await
    }

    pub async fn stats_overview(&self) -> Result<StatsOverview, CoreError> {
        self.sessions.stats_overview().await
    }

    pub async fn record_system_message(
        &self,
        session_id: &str,
        content: impl Into<String>,
    ) -> Result<TranscriptMessage, CoreError> {
        self.sessions
            .append_system_message(session_id, content)
            .await
    }

    pub async fn fork_session(
        &self,
        session_id: &str,
        title: Option<String>,
        note: Option<String>,
    ) -> Result<SessionRecord, CoreError> {
        self.sessions.fork_session(session_id, title, note).await
    }

    pub async fn acp_load_replay_preflight(
        &self,
        session_id: &str,
    ) -> Result<AcpLoadReplayPreflight, CoreError> {
        let (session, blockers) = self.sessions.acp_load_replay_preflight(session_id).await?;
        Ok(AcpLoadReplayPreflight {
            session,
            replay_allowed: blockers.is_empty(),
            blockers,
        })
    }

    pub async fn acp_delete_session(
        &self,
        params: AcpDeleteSessionParams,
    ) -> Result<(), CoreError> {
        self.sessions
            .delete_acp_visible_session(&params.session_id, params.cwd)
            .await?;
        self.sessions.remove_session_controls(&params.session_id);
        self.mcp.remove_session_servers(&params.session_id).await;
        Ok(())
    }

    pub fn session_control_state(
        &self,
        session_id: &str,
    ) -> Result<SessionControlState, CoreError> {
        Ok(SessionControlState {
            session_id: session_id.to_string(),
            permission_mode: permission_mode_to_wire(
                self.sessions.session_permission_mode(session_id)?,
            ),
            model_options: self
                .sessions
                .session_model_options(session_id)?
                .into_iter()
                .map(|option| SessionModelOption {
                    value: option.value,
                    label: option.label,
                    description: option.description,
                    current: option.current,
                })
                .collect(),
            effort_level: self.sessions.session_effort_level(session_id)?,
            effort_options: self.sessions.session_effort_options(session_id)?,
        })
    }

    pub async fn set_session_permission_mode(
        &self,
        session_id: &str,
        mode: PermissionMode,
    ) -> Result<SessionControlState, CoreError> {
        self.ensure_setting_mutable("permissions")?;
        self.sessions
            .set_session_permission_mode(session_id, permission_mode_from_wire(mode))
            .await?;
        self.session_control_state(session_id)
    }

    pub async fn set_session_model(
        &self,
        session_id: &str,
        model: Option<String>,
    ) -> Result<SessionControlState, CoreError> {
        self.ensure_setting_mutable("model")?;
        self.sessions.set_session_model(session_id, model).await?;
        self.session_control_state(session_id)
    }

    pub async fn set_session_effort(
        &self,
        session_id: &str,
        effort: Option<EffortLevel>,
    ) -> Result<SessionControlState, CoreError> {
        self.ensure_setting_mutable("effortLevel")?;
        self.sessions.set_session_effort(session_id, effort).await?;
        self.session_control_state(session_id)
    }

    pub async fn cleanup_session(&self, session_id: &str) {
        self.sessions.remove_session_controls(session_id);
        self.remove_session_mcp_servers(session_id).await;
    }

    pub async fn clear_session(
        &self,
        previous_session_id: &str,
    ) -> Result<BootstrapState, CoreError> {
        let session = self.sessions.clear_session(previous_session_id).await?;
        self.set_active_session_id(&session.session_id);
        let event = StreamEvent::SessionStarted {
            summary: session.summary(),
        };
        self.bootstrap_state(session, event, true).await
    }

    pub async fn evaluate_manual_compact_decision(
        &self,
        session_id: &str,
    ) -> Result<CompactDecision, CoreError> {
        self.sessions
            .evaluate_manual_compact_decision(session_id)
            .await
    }

    pub async fn compact_session(
        &self,
        session_id: &str,
    ) -> Result<CompactSessionResult, CoreError> {
        self.sessions.compact_session(session_id).await
    }

    /// Rewind a session to a previous point by truncating the persisted
    /// transcript so it keeps only the first `keep_messages` messages, then
    /// reloading the (now-shortened) session. Returns the fresh
    /// `BootstrapState` so the caller can re-initialise its view exactly as it
    /// would after a resume.
    ///
    /// Unlike a resume, a rewind never changes the cwd, config, or MCP servers,
    /// so the (potentially slow) live MCP enumeration is skipped: the returned
    /// `BootstrapState` carries no MCP slash suggestions, and the caller is
    /// expected to keep the ones it already holds.
    pub async fn rewind_session(
        &self,
        session_id: &str,
        keep_messages: usize,
    ) -> Result<BootstrapState, CoreError> {
        self.sessions
            .rewind_session(session_id, keep_messages)
            .await?;
        let (session, bootstrap_event) = self.sessions.start_or_resume(Some(session_id)).await?;
        self.set_active_session_id(&session.session_id);
        self.bootstrap_state(session, bootstrap_event, false).await
    }

    pub async fn submit_turn(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, CoreError> {
        self.sessions.submit_turn(session_id, prompt).await
    }

    pub async fn submit_turn_with_interaction(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
        interaction: TurnInteractionContext,
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, CoreError> {
        self.sessions
            .submit_turn_with_interaction(session_id, prompt, interaction)
            .await
    }

    pub async fn steer_turn(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
    ) -> Result<(), CoreError> {
        self.sessions.steer_turn(session_id, prompt).await
    }

    pub async fn cancel_turn(&self, session_id: &str) -> bool {
        self.sessions.cancel_turn(session_id).await
    }

    pub async fn remove_session_mcp_servers(&self, session_id: &str) -> Vec<String> {
        self.mcp.remove_session_servers(session_id).await
    }

    pub async fn interrupt_turn(&self, session_id: &str) -> bool {
        self.sessions.interrupt_turn(session_id).await
    }

    pub async fn respond_to_permission_request(
        &self,
        request_id: &str,
        decision: PermissionDecision,
    ) -> bool {
        self.sessions
            .respond_to_permission_request(request_id, decision)
            .await
    }

    pub fn resolve_ask_user_question(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: orbcode_protocol::AskUserResponseOutcome,
    ) -> Result<(), orbcode_core::InteractionResolveError> {
        self.sessions
            .resolve_ask_user_question(session_id, request_id, outcome)
    }

    pub fn cancel_pending_ask_user(
        &self,
        request_ids: &[String],
        reason: orbcode_protocol::AskUserCancellationReason,
    ) {
        self.sessions.cancel_pending_ask_user(request_ids, reason);
    }

    pub fn cancel_pending_ask_user_for_owner(
        &self,
        owner_id: &str,
        reason: orbcode_protocol::AskUserCancellationReason,
    ) {
        self.sessions
            .cancel_pending_ask_user_for_owner(owner_id, reason);
    }

    pub async fn disconnect_interaction_owner(&self, owner_id: &str) -> Vec<String> {
        self.sessions.disconnect_interaction_owner(owner_id).await
    }

    #[doc(hidden)]
    pub fn register_pending_ask_user_for_test(
        &self,
        session_id: &str,
        request_id: &str,
        questions: Vec<orbcode_protocol::AskUserQuestionSpec>,
    ) -> tokio::sync::oneshot::Receiver<orbcode_protocol::AskUserResponseOutcome> {
        self.sessions
            .register_pending_ask_user_for_test(session_id, request_id, questions)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_config::{AppConfigOverrides, sanitize_path};
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
    async fn rewind_session_truncates_persisted_transcript_on_disk() {
        let home = test_path("rewind-e2e-home");
        let cwd = test_path("rewind-e2e-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let project_dir = home
            .join("projects")
            .join(sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");

        let session_id = "rewind-e2e-session";
        let session_cwd = cwd.display().to_string();
        let user_line = |index: usize, parent: Option<usize>, text: &str| {
            serde_json::to_string(&json!({
                "parentUuid": parent.map(|p| format!("uuid-{p}")),
                "type": "user",
                "message": { "role": "user", "content": text },
                "cwd": session_cwd,
                "sessionId": session_id,
                "uuid": format!("uuid-{index}"),
                "timestamp": format!("2026-05-20T10:00:{index:02}.000Z"),
            }))
            .expect("serialize user line")
        };
        let assistant_line = |index: usize, parent: usize, text: &str| {
            serde_json::to_string(&json!({
                "parentUuid": format!("uuid-{parent}"),
                "type": "assistant",
                "message": {
                    "id": format!("msg-{index}"),
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-20250514",
                    "content": [{ "type": "text", "text": text }],
                    "stop_reason": "end_turn",
                    "stop_sequence": null,
                    "usage": { "input_tokens": 5, "output_tokens": 5 },
                },
                "requestId": format!("req-{index}"),
                "uuid": format!("uuid-{index}"),
                "timestamp": format!("2026-05-20T10:00:{index:02}.000Z"),
            }))
            .expect("serialize assistant line")
        };

        let lines = [
            user_line(1, None, "first prompt"),
            assistant_line(2, 1, "first reply"),
            user_line(3, Some(2), "second prompt"),
            assistant_line(4, 3, "second reply"),
            user_line(5, Some(4), "third prompt"),
        ];
        let transcript_path = project_dir.join(format!("{session_id}.jsonl"));
        tokio::fs::write(&transcript_path, format!("{}\n", lines.join("\n")))
            .await
            .expect("write transcript");

        let app = AppServer::new(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let before = app.bootstrap(Some(session_id)).await.expect("bootstrap");
        assert_eq!(before.session.messages.len(), 5);
        let lines_before = tokio::fs::read_to_string(&transcript_path)
            .await
            .expect("read transcript before")
            .lines()
            .count();

        let rewound = app
            .rewind_session(session_id, 3)
            .await
            .expect("rewind session");
        assert_eq!(rewound.session.messages.len(), 3);
        assert_eq!(rewound.session.messages[2].content, "second prompt");

        let lines_after = tokio::fs::read_to_string(&transcript_path)
            .await
            .expect("read transcript after")
            .lines()
            .count();
        assert!(
            lines_after < lines_before,
            "transcript file should shrink: {lines_before} -> {lines_after}"
        );
        assert_eq!(lines_after, 3);

        let resumed = app
            .bootstrap(Some(session_id))
            .await
            .expect("bootstrap after rewind");
        assert_eq!(resumed.session.messages.len(), 3);
        assert_eq!(resumed.session.messages[2].content, "second prompt");
    }

    #[tokio::test]
    async fn rewind_session_clamps_out_of_range_keep_count() {
        let home = test_path("rewind-clamp-home");
        let cwd = test_path("rewind-clamp-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let project_dir = home
            .join("projects")
            .join(sanitize_path(&cwd.display().to_string()));
        tokio::fs::create_dir_all(&project_dir)
            .await
            .expect("project dir");

        let session_id = "rewind-clamp-session";
        let payload = serde_json::to_string(&json!({
            "parentUuid": null,
            "type": "user",
            "message": { "role": "user", "content": "only prompt" },
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
            "uuid": "uuid-1",
            "timestamp": "2026-05-20T10:00:01.000Z",
        }))
        .expect("serialize transcript");
        tokio::fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!("{payload}\n"),
        )
        .await
        .expect("write transcript");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let rewound = app
            .rewind_session(session_id, 99)
            .await
            .expect("rewind with oversized keep count");
        assert_eq!(rewound.session.messages.len(), 1);
        assert_eq!(rewound.session.messages[0].content, "only prompt");
    }
}
