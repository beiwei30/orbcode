use orbcode_config::{HookMatcher, HookSource};
use orbcode_protocol::{MessageRole, StreamEvent, TranscriptMessage};
use orbcode_tools::{ToolOutcome, post_tool_response};
use serde_json::{Value, json};
use tokio::{sync::mpsc, time::Duration};

use super::{POST_TOOL_FAILURE_RETRY_MESSAGE, QueuedModelVisibleContext, SessionManager};
use crate::{
    CoreError,
    hook_runner::{
        HookCommandContext, HookCommandProgress, run_permission_denied_hook_commands,
        run_post_tool_failure_hook_commands, run_post_tool_hook_commands,
        run_pre_tool_hook_commands, run_stop_failure_hook_commands, run_stop_hook_commands,
        run_subagent_start_hook_commands, run_subagent_stop_hook_commands,
        run_user_prompt_submit_hook_commands,
    },
    hooks::{
        PreToolHookOutcome, StopHookOutcome, UserPromptSubmitHookOutcome, hook_additional_context,
        hook_progress_record, model_visible_context_message, stop_hook_feedback,
    },
};

impl SessionManager {
    pub(super) async fn append_post_tool_hook_contexts(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        outcome: &ToolOutcome,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let response = post_tool_response(outcome);
        let additional_contexts = self
            .run_post_tool_hooks(
                session_id,
                tool_use_id,
                tool_name,
                tool_input,
                &response,
                tx,
            )
            .await;
        self.append_hook_additional_contexts(session_id, "PostToolUse", additional_contexts, tx)
            .await
    }

    pub(super) async fn append_post_tool_failure_hook_contexts(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        error_message: &str,
        is_interrupt: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let (additional_contexts, retry) = self
            .run_post_tool_failure_hooks(
                session_id,
                tool_use_id,
                tool_name,
                tool_input,
                error_message,
                is_interrupt,
                tx,
            )
            .await;
        self.append_hook_additional_contexts(
            session_id,
            "PostToolUseFailure",
            additional_contexts,
            tx,
        )
        .await?;
        if retry {
            self.append_model_visible_context_message(
                session_id,
                "hook_retry_context_to_llm",
                "PostToolUseFailure retry guidance",
                POST_TOOL_FAILURE_RETRY_MESSAGE.to_string(),
                tx,
            )
            .await?;
        }
        Ok(())
    }

    async fn run_post_tool_hooks(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tool_response: &Value,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Vec<String> {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) = self.trusted_hook_matchers("PostToolUse");
        let run = run_post_tool_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            tool_use_id,
            tool_name,
            tool_input,
            tool_response,
        )
        .await;
        self.append_tool_hook_progress_events(
            session_id,
            tool_use_id,
            tool_name,
            &run.progress,
            tx,
        )
        .await;
        run.additional_contexts
    }

    async fn run_post_tool_failure_hooks(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        error: &str,
        is_interrupt: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> (Vec<String>, bool) {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) = self.trusted_hook_matchers("PostToolUseFailure");
        let run = run_post_tool_failure_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            tool_use_id,
            tool_name,
            tool_input,
            error,
            is_interrupt,
        )
        .await;
        self.append_tool_hook_progress_events(
            session_id,
            tool_use_id,
            tool_name,
            &run.progress,
            tx,
        )
        .await;
        (run.additional_contexts, run.retry)
    }

    pub(super) async fn run_user_prompt_submit_hooks(
        &self,
        session_id: &str,
        prompt: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> UserPromptSubmitHookOutcome {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) = self.trusted_hook_matchers("UserPromptSubmit");
        let run = run_user_prompt_submit_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            prompt,
        )
        .await;
        self.append_lifecycle_hook_progress_events(session_id, &run.progress, tx)
            .await;
        run.outcome
    }

    pub(super) async fn run_permission_denied_hooks(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        reason: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> bool {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) = self.trusted_hook_matchers("PermissionDenied");
        let run = run_permission_denied_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            tool_use_id,
            tool_name,
            tool_input,
            reason,
        )
        .await;
        self.append_tool_hook_progress_events(
            session_id,
            tool_use_id,
            tool_name,
            &run.progress,
            tx,
        )
        .await;
        run.retry
    }

    pub(super) async fn run_stop_hooks(
        &self,
        session_id: &str,
        last_assistant_message: &str,
        stop_hook_active: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> StopHookOutcome {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) = self.trusted_hook_matchers("Stop");
        let run = run_stop_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            last_assistant_message,
            stop_hook_active,
        )
        .await;
        self.append_lifecycle_hook_progress_events(session_id, &run.progress, tx)
            .await;
        run.outcome
    }

    pub(super) async fn run_stop_failure_hooks(
        &self,
        session_id: &str,
        error: &str,
        error_details: &str,
        last_assistant_message: Option<&str>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) = self.trusted_hook_matchers("StopFailure");
        let run = run_stop_failure_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            error,
            error_details,
            last_assistant_message,
        )
        .await;
        self.append_lifecycle_hook_progress_events(session_id, &run.progress, tx)
            .await;
    }

    pub(super) async fn run_subagent_start_hooks(
        &self,
        session_id: &str,
        agent_id: &str,
        agent_type: &str,
        agent_definition: Option<&orbcode_config::AgentDefinition>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Vec<String> {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) =
            self.trusted_hook_matchers_with_agent_overlay("SubagentStart", agent_definition);
        let run = run_subagent_start_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            agent_id,
            agent_type,
        )
        .await;
        self.append_lifecycle_hook_progress_events(session_id, &run.progress, tx)
            .await;
        run.additional_contexts
    }

    pub(super) async fn run_subagent_stop_hooks(
        &self,
        session_id: &str,
        agent_id: &str,
        child_session_id: &str,
        agent_type: &str,
        last_assistant_message: &str,
        stop_hook_active: bool,
        agent_definition: Option<&orbcode_config::AgentDefinition>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> StopHookOutcome {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) =
            self.trusted_hook_matchers_with_agent_overlay("SubagentStop", agent_definition);
        let run = run_subagent_stop_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            agent_id,
            child_session_id,
            agent_type,
            last_assistant_message,
            stop_hook_active,
        )
        .await;
        self.append_lifecycle_hook_progress_events(session_id, &run.progress, tx)
            .await;
        run.outcome
    }

    pub(super) async fn append_stop_hook_feedback(
        &self,
        session_id: &str,
        blocking_errors: Vec<String>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        for blocking_error in blocking_errors {
            self.append_model_visible_context_message(
                session_id,
                "hook_feedback_to_llm",
                "Stop hook feedback",
                stop_hook_feedback(&blocking_error),
                tx,
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn emit_hook_notice(
        &self,
        session_id: &str,
        hook_event_name: &str,
        message: &str,
        is_error: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        self.provider_debug_trace
            .append_activity(json!({
                "type": "hook_notice_to_orbcode",
                "label": format!("{hook_event_name} hook"),
                "hook_event_name": hook_event_name,
                "message": message,
                "is_error": is_error,
            }))
            .await;
        let _ = tx.send(StreamEvent::HookNotice {
            session_id: session_id.to_string(),
            hook_event_name: hook_event_name.to_string(),
            message: message.to_string(),
            is_error,
        });
    }

    pub(super) async fn append_hook_additional_contexts(
        &self,
        session_id: &str,
        hook_event: &str,
        additional_contexts: Vec<String>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        for additional_context in additional_contexts {
            let label = format!("{hook_event} hook context");
            self.append_model_visible_context_message(
                session_id,
                "hook_context_to_llm",
                &label,
                hook_additional_context(hook_event, &additional_context),
                tx,
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn append_model_visible_context_message(
        &self,
        session_id: &str,
        activity_type: &str,
        label: &str,
        content: String,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let message = model_visible_context_message(content);
        {
            let mut queued = self.queued_model_visible_contexts.lock().await;
            if let Some(state) = queued.get_mut(session_id)
                && state.hold_depth > 0
            {
                state.contexts.push(QueuedModelVisibleContext {
                    activity_type: activity_type.to_string(),
                    label: label.to_string(),
                    message,
                });
                return Ok(());
            }
        }

        self.append_model_visible_context_entry(session_id, activity_type, label, message, tx)
            .await?;
        Ok(())
    }

    pub(super) async fn begin_tool_result_context_queue(&self, session_id: &str) {
        let mut queued = self.queued_model_visible_contexts.lock().await;
        let state = queued.entry(session_id.to_string()).or_default();
        state.hold_depth += 1;
    }

    pub(super) async fn flush_tool_result_context_queue(
        &self,
        session_id: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let contexts = {
            let mut queued = self.queued_model_visible_contexts.lock().await;
            let Some(state) = queued.get_mut(session_id) else {
                return Ok(());
            };
            state.hold_depth = state.hold_depth.saturating_sub(1);
            if state.hold_depth > 0 {
                Vec::new()
            } else {
                let contexts = std::mem::take(&mut state.contexts);
                queued.remove(session_id);
                contexts
            }
        };

        for context in contexts {
            self.append_model_visible_context_entry(
                session_id,
                &context.activity_type,
                &context.label,
                context.message,
                tx,
            )
            .await?;
        }

        Ok(())
    }

    pub(super) async fn discard_tool_result_context_queue(&self, session_id: &str) {
        let mut queued = self.queued_model_visible_contexts.lock().await;
        queued.remove(session_id);
    }

    /// Drain queued user commands and persist them as real user messages so the
    /// next provider request sees them in the transcript.
    pub(super) async fn flush_queued_user_commands(
        &self,
        session_id: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let commands = self.drain_queued_user_commands(session_id).await;
        for command in commands {
            let message = TranscriptMessage::new(MessageRole::User, command.content);
            self.append_message(session_id, message.clone()).await?;
            let _ = tx.send(StreamEvent::UserMessage { message });
        }
        Ok(())
    }

    async fn append_model_visible_context_entry(
        &self,
        session_id: &str,
        activity_type: &str,
        label: &str,
        message: TranscriptMessage,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        self.append_message(session_id, message.clone()).await?;
        self.provider_debug_trace
            .append_message_activity(self.config.default_provider, activity_type, label, &message)
            .await;
        let _ = tx.send(StreamEvent::UserMessage { message });
        Ok(())
    }

    pub(super) async fn run_pre_tool_hooks(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<PreToolHookOutcome, CoreError> {
        let hook_context = self.hook_command_context(session_id);
        let (matchers, sources) = self.trusted_hook_matchers("PreToolUse");
        let run = run_pre_tool_hook_commands(
            &hook_context,
            Some(matchers.as_slice()),
            Some(sources.as_slice()),
            tool_use_id,
            tool_name,
            tool_input,
        )
        .await;
        self.append_tool_hook_progress_events(
            session_id,
            tool_use_id,
            tool_name,
            &run.progress,
            tx,
        )
        .await;
        run.outcome
    }

    async fn append_hook_progress_event(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        hook_event_name: &str,
        command: &str,
        status: &'static str,
        exit_code: Option<i32>,
        error: Option<&str>,
        duration: Duration,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        let _ = self
            .append_tool_progress_event(
                session_id,
                tool_use_id,
                tool_name,
                hook_progress_record(hook_event_name, command, status, exit_code, error, duration),
                tx,
            )
            .await;
    }

    async fn append_lifecycle_hook_progress_event(
        &self,
        session_id: &str,
        hook_event_name: &str,
        command: &str,
        status: &'static str,
        exit_code: Option<i32>,
        error: Option<&str>,
        duration: Duration,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        let progress =
            hook_progress_record(hook_event_name, command, status, exit_code, error, duration);
        let _ = self
            .transcript_store
            .append_progress_for_latest_parent_if_present(session_id, progress.clone())
            .await;
        let _ = tx.send(StreamEvent::HookProgress {
            session_id: session_id.to_string(),
            hook_event_name: hook_event_name.to_string(),
            progress,
        });
    }

    async fn append_tool_hook_progress_events(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        progress: &[HookCommandProgress],
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        for progress in progress {
            self.append_hook_progress_event(
                session_id,
                tool_use_id,
                tool_name,
                progress.event_name,
                &progress.command,
                progress.status,
                progress.exit_code,
                progress.error.as_deref(),
                progress.elapsed,
                tx,
            )
            .await;
        }
    }

    async fn append_lifecycle_hook_progress_events(
        &self,
        session_id: &str,
        progress: &[HookCommandProgress],
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        for progress in progress {
            self.append_lifecycle_hook_progress_event(
                session_id,
                progress.event_name,
                &progress.command,
                progress.status,
                progress.exit_code,
                progress.error.as_deref(),
                progress.elapsed,
                tx,
            )
            .await;
        }
    }

    fn hook_command_context<'a>(&'a self, session_id: &'a str) -> HookCommandContext<'a> {
        let config = self.effective_config();
        HookCommandContext::new(session_id, &self.transcript_store, &config.cwd)
    }

    /// Same as [`trusted_hook_matchers`], but additionally overlays matchers
    /// declared by the agent definition's `hooks` frontmatter block. The
    /// overlay only applies to the child loop, so agent-specific hooks never
    /// leak back into the parent session's hook table. Agent hooks sourced
    /// from `.claude/agents/` (project settings) are filtered out when the
    /// project is not trusted, mirroring the `settings.local.json` policy.
    fn trusted_hook_matchers_with_agent_overlay(
        &self,
        event: &str,
        agent_definition: Option<&orbcode_config::AgentDefinition>,
    ) -> (Vec<HookMatcher>, Vec<HookSource>) {
        let (mut matchers, mut sources) = self.trusted_hook_matchers(event);
        let Some(definition) = agent_definition else {
            return (matchers, sources);
        };
        let Some(extra) = definition.hooks.get(event) else {
            return (matchers, sources);
        };
        let from_project = matches!(
            definition.source,
            orbcode_config::AgentSource::ProjectSettings
        );
        if from_project && !self.config.trusted_project {
            return (matchers, sources);
        }
        for matcher in extra {
            matchers.push(matcher.clone());
            sources.push(HookSource::Settings);
        }
        (matchers, sources)
    }

    /// Return matchers and sources for `event`, filtering out matchers backed
    /// by `.claude/settings.local.json` when the project is not trusted. Local
    /// hooks otherwise execute arbitrary commands sourced from the working
    /// directory, so they must require explicit project trust.
    fn trusted_hook_matchers(&self, event: &str) -> (Vec<HookMatcher>, Vec<HookSource>) {
        // Managed policy can restrict hooks to managed settings only. Managed
        // hooks are never loaded into `config.settings.hooks`, so honoring the
        // policy means registering no hooks from user/project/local sources.
        if self.config.policy.allow_managed_hooks_only {
            return (Vec::new(), Vec::new());
        }
        let matchers = self.config.hooks_for_event(event).to_vec();
        let sources = self.config.hook_sources_for_event(event).to_vec();
        if self.config.trusted_project {
            return (matchers, sources);
        }
        let mut filtered_matchers = Vec::with_capacity(matchers.len());
        let mut filtered_sources = Vec::with_capacity(matchers.len());
        for (idx, matcher) in matchers.into_iter().enumerate() {
            // Fail closed on a matcher/source desync: a missing source entry
            // must NOT default to the trusted `Settings` source, or a local
            // hook could execute in an untrusted project. Drop any matcher
            // whose source we cannot positively confirm is non-local.
            match sources.get(idx).copied() {
                Some(source) if !source.is_local() => {
                    filtered_matchers.push(matcher);
                    filtered_sources.push(source);
                }
                _ => continue,
            }
        }
        (filtered_matchers, filtered_sources)
    }
}
