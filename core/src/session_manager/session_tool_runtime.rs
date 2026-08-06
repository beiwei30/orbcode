use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use async_trait::async_trait;
use orbcode_config::mcp_permission_target;
use orbcode_mcp::McpServerTrust;
use orbcode_protocol::{
    McpTrustApprovalRequest, McpTrustResolutionKind, PermissionRequest, PermissionResolutionKind,
    StreamEvent, ToolUseCompletionKind,
};
use orbcode_tools::{
    SkillDefinition, ToolCancellationToken, ToolContext, ToolOutcome, ToolProgressReporter,
    ToolSpec, ToolStatus, load_skill_definitions_with_bounded_mcp_for_session,
    parse_mcp_provider_tool_name, read_background_task_record, task_record_to_view,
    tool_result_metadata,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{LiveToolProgressReporter, SessionManager};
use crate::{
    CoreError,
    hooks::PreToolPhaseOutcome,
    interaction_runtime::InteractionRuntime,
    permission_state::PermissionDecision,
    permissions::PermissionContext,
    tool_flow::{
        BufferedToolResult, BufferedToolUseCompletion, McpTrustResolutionOutcome,
        ToolDenyPrecedenceStage, ToolLookupOutcome, ToolPermissionResolutionOutcome,
        ToolUseOutcome, tool_result_content,
    },
    tool_progress::initial_tool_progress_record,
    tool_runtime::{ToolRuntime, ToolRuntimeHost, ToolSuccessResultDetails},
};

const PERMISSION_POLL_MS: u64 = 100;
const MCP_SKILL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const WORKFLOW_TOOL_SPEC: ToolSpec = ToolSpec {
    name: "Workflow",
    status: ToolStatus::Available,
    summary: "Start a generated dynamic workflow as a durable background task.",
    requires_tools_permission: true,
    requires_network_permission: false,
    provider_hidden: false,
};

#[derive(Debug, serde::Deserialize)]
struct WorkflowToolInput {
    #[serde(default)]
    name: String,
    spec: serde_json::Value,
    #[serde(default)]
    arguments: String,
}

fn parse_workflow_tool_input(tool_input: &str) -> Result<WorkflowToolInput, CoreError> {
    let value = parse_workflow_json_value(tool_input, false)?;
    let value = match value {
        serde_json::Value::String(encoded) => parse_workflow_json_value(&encoded, true)?,
        other => other,
    };

    if !value.is_object() {
        return Err(CoreError::Tool(format!(
            "invalid Workflow input: expected a valid JSON object with `spec` plus optional `name` and `arguments`; got {}",
            workflow_json_type_name(&value)
        )));
    }

    serde_json::from_value(value).map_err(|error| {
        CoreError::Tool(format!(
            "invalid Workflow input: expected a valid JSON object with `spec` plus optional `name` and `arguments`; {error}"
        ))
    })
}

fn parse_workflow_json_value(
    input: &str,
    decoded_from_string: bool,
) -> Result<serde_json::Value, CoreError> {
    match serde_json::from_str(input) {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(repaired) = repair_missing_workflow_step_object_closes(input)
                && let Ok(value) = serde_json::from_str(&repaired)
            {
                return Ok(value);
            }

            let excerpt = json_error_excerpt(input, error.line(), error.column());
            let quoted_note = if decoded_from_string {
                " The tool input was a quoted JSON string; pass the Workflow arguments as an object instead."
            } else {
                ""
            };
            Err(CoreError::Tool(format!(
                "invalid Workflow input: expected a valid JSON object with `spec` plus optional `name` and `arguments`; {error}. Near error: {excerpt}.{quoted_note} For parallel agent steps, close each child object fully: {{\"parallel\":{{\"steps\":[{{\"agent\":{{\"description\":\"task2\",\"prompt\":\"...\"}}}},{{\"agent\":{{\"description\":\"task3\",\"prompt\":\"...\"}}}}]}}}}."
            )))
        }
    }
}

fn workflow_json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn repair_missing_workflow_step_object_closes(input: &str) -> Option<String> {
    let mut repaired = String::with_capacity(input.len());
    let mut changed = false;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        repaired.push(ch);

        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            continue;
        }

        if ch == '}' && should_insert_missing_step_close(input, index) {
            repaired.push('}');
            changed = true;
        }
    }

    changed.then_some(repaired)
}

fn should_insert_missing_step_close(input: &str, close_index: usize) -> bool {
    if previous_non_ws_before(input, close_index) == Some('}') {
        return false;
    }

    let Some((next_index, next)) = next_non_ws_after(input, close_index + 1) else {
        return false;
    };

    match next {
        ']' => true,
        ',' => next_starts_agent_step(input, next_index + 1),
        _ => false,
    }
}

fn previous_non_ws_before(input: &str, index: usize) -> Option<char> {
    input[..index].chars().rev().find(|ch| !ch.is_whitespace())
}

fn next_non_ws_after(input: &str, index: usize) -> Option<(usize, char)> {
    input[index..]
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(offset, ch)| (index + offset, ch))
}

fn next_starts_agent_step(input: &str, index: usize) -> bool {
    let Some((open_index, '{')) = next_non_ws_after(input, index) else {
        return false;
    };
    let Some((key_index, '"')) = next_non_ws_after(input, open_index + 1) else {
        return false;
    };
    input[key_index..].starts_with("\"agent\"")
}

fn json_error_excerpt(input: &str, line: usize, column: usize) -> String {
    let error_offset = byte_offset_for_line_column(input, line, column).unwrap_or(input.len());
    let start = input[..error_offset]
        .char_indices()
        .rev()
        .nth(80)
        .map_or(0, |(index, _)| index);
    let end = input[error_offset..]
        .char_indices()
        .nth(80)
        .map_or(input.len(), |(index, _)| error_offset + index);
    let mut excerpt = String::new();
    if start > 0 {
        excerpt.push_str("...");
    }
    excerpt.push_str(&input[start..end]);
    if end < input.len() {
        excerpt.push_str("...");
    }
    excerpt
}

fn byte_offset_for_line_column(input: &str, line: usize, column: usize) -> Option<usize> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut current_line = 1;
    let mut line_start = 0;
    for (index, ch) in input.char_indices() {
        if current_line == line {
            break;
        }
        if ch == '\n' {
            current_line += 1;
            line_start = index + ch.len_utf8();
        }
    }
    if current_line != line {
        return None;
    }
    Some(
        input[line_start..]
            .char_indices()
            .nth(column.saturating_sub(1))
            .map_or(input.len(), |(index, _)| line_start + index),
    )
}

/// Synthetic spec used when the model addresses an MCP tool by its stable
/// `mcp__{server}__{tool}` name. We don't keep a static `ToolSpec` per MCP
/// tool (server contents change at runtime), so the runtime treats every such
/// name as the same shape: tools-permission required, no network gate.
const MCP_PROVIDER_TOOL_SPEC: ToolSpec = ToolSpec {
    name: "mcp__",
    status: ToolStatus::Available,
    summary: "MCP server tool invoked through the model tool-use path.",
    requires_tools_permission: true,
    requires_network_permission: false,
    provider_hidden: false,
};

impl SessionManager {
    pub async fn skill_definitions_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<SkillDefinition>, CoreError> {
        let config = self.effective_config();
        let cwd = self.cwd_for_session(session_id).await?;
        Ok(load_skill_definitions_with_bounded_mcp_for_session(
            &config.home_dir,
            &cwd,
            &self.mcp,
            session_id,
            MCP_SKILL_DISCOVERY_TIMEOUT,
        )
        .await?)
    }

    pub(super) async fn skill_definitions_visible_to_session(
        &self,
        session_id: &str,
    ) -> Vec<SkillDefinition> {
        let config = self.effective_config();
        let cwd = self
            .cwd_for_session(session_id)
            .await
            .unwrap_or_else(|_| config.cwd.clone());
        load_skill_definitions_with_bounded_mcp_for_session(
            &config.home_dir,
            &cwd,
            &self.mcp,
            session_id,
            MCP_SKILL_DISCOVERY_TIMEOUT,
        )
        .await
        .unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) async fn execute_tool_use(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolUseOutcome, CoreError> {
        self.begin_tool_result_context_queue(session_id).await;
        let result = self
            .execute_tool_use_in_active_context_queue(
                session_id,
                tool_use_id,
                tool_name,
                tool_input,
                tx,
                cancel_flag,
            )
            .await;
        if result.is_ok() {
            self.flush_tool_result_context_queue(session_id, tx).await?;
        } else {
            self.discard_tool_result_context_queue(session_id).await;
        }
        result
    }

    pub(super) async fn execute_tool_use_in_active_context_queue(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolUseOutcome, CoreError> {
        ToolRuntime::new(&self.tools, self)
            .execute_tool_use(
                session_id,
                tool_use_id,
                tool_name,
                tool_input,
                tx,
                cancel_flag,
            )
            .await
    }

    async fn lookup_tool_spec_or_append_unknown(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolLookupOutcome, CoreError> {
        if let Some(spec) = self.tools.spec(tool_name).cloned() {
            return Ok(ToolLookupOutcome::Found(spec));
        }
        if tool_name.eq_ignore_ascii_case("Workflow") {
            return Ok(ToolLookupOutcome::Found(WORKFLOW_TOOL_SPEC));
        }
        if parse_mcp_provider_tool_name(tool_name).is_some() {
            return Ok(ToolLookupOutcome::Found(MCP_PROVIDER_TOOL_SPEC));
        }
        self.append_tool_result_message(
            session_id,
            tool_use_id,
            format!("unknown tool requested by provider: {tool_name}"),
            true,
            None,
            tx,
        )
        .await?;
        self.emit_tool_use_completed(
            session_id,
            tool_use_id,
            tool_name,
            ToolUseCompletionKind::UnknownTool,
            tx,
        );
        Ok(ToolLookupOutcome::UnknownHandled)
    }

    async fn run_pre_tool_phase(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<PreToolPhaseOutcome, CoreError> {
        let hook_outcome = self
            .run_pre_tool_hooks(session_id, tool_use_id, tool_name, tool_input, tx)
            .await?;
        self.append_hook_additional_contexts(
            session_id,
            "PreToolUse",
            hook_outcome.additional_contexts,
            tx,
        )
        .await?;

        Ok(PreToolPhaseOutcome {
            tool_input: hook_outcome.tool_input,
            decision: hook_outcome.decision,
            reason: hook_outcome.reason,
        })
    }

    async fn resolve_tool_permission_request(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        spec: &ToolSpec,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> ToolPermissionResolutionOutcome {
        let request = PermissionRequest {
            request_id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            tool_input: tool_input.to_string(),
            requires_tools_permission: spec.requires_tools_permission,
            requires_network_permission: spec.requires_network_permission,
        };

        let outcome = match self
            .permission_runtime
            .await_permission_decision(
                &request,
                || {
                    tx.send(StreamEvent::PermissionRequested {
                        request: request.clone(),
                    })
                    .is_ok()
                },
                cancel_flag,
                Duration::from_millis(PERMISSION_POLL_MS),
            )
            .await
        {
            Some(PermissionDecision::Approve) => ToolPermissionResolutionOutcome::Approved,
            Some(PermissionDecision::ApproveAlways(rule)) => {
                self.permission_runtime
                    .remember_permission_rule(tool_name, &rule)
                    .await;
                ToolPermissionResolutionOutcome::Approved
            }
            Some(PermissionDecision::ApproveAlwaysMany(rules)) => {
                for rule in rules {
                    self.permission_runtime
                        .remember_permission_rule(tool_name, &rule)
                        .await;
                }
                ToolPermissionResolutionOutcome::Approved
            }
            Some(PermissionDecision::Deny) => {
                self.permission_runtime
                    .remember_denied_tool_call(tool_name, tool_input)
                    .await;
                ToolPermissionResolutionOutcome::Denied
            }
            None => ToolPermissionResolutionOutcome::Interrupted,
        };

        let kind = match outcome {
            ToolPermissionResolutionOutcome::Approved => PermissionResolutionKind::Approved,
            ToolPermissionResolutionOutcome::Denied => PermissionResolutionKind::Denied,
            ToolPermissionResolutionOutcome::Interrupted => PermissionResolutionKind::Interrupted,
        };
        let _ = tx.send(StreamEvent::PermissionResolved {
            session_id: session_id.to_string(),
            request_id: request.request_id,
            kind,
        });

        outcome
    }

    pub(super) async fn tool_deny_precedence_reason(
        &self,
        permissions: &PermissionContext,
        tool_name: &str,
        tool_input: &str,
        stage: ToolDenyPrecedenceStage,
    ) -> Option<String> {
        let suffix = stage.reason_suffix();
        let denied_tool =
            mcp_permission_target(tool_name, tool_input).unwrap_or_else(|| tool_name.to_string());
        if permissions.tool_denied(tool_name, tool_input).is_some() {
            return Some(format!(
                "permission denied for tool `{denied_tool}` by configured deny rule{suffix}"
            ));
        }
        if self
            .permission_runtime
            .matches_denied_tool_call(tool_name, tool_input)
            .await
        {
            return Some(format!(
                "permission denied for tool `{denied_tool}` by previous user denial{suffix}"
            ));
        }
        None
    }

    async fn invoke_workflow_tool_and_buffer_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        tx: Option<&mpsc::UnboundedSender<StreamEvent>>,
    ) -> Result<BufferedToolUseCompletion, CoreError> {
        let result = async {
            let input = parse_workflow_tool_input(tool_input)?;
            let task_id = self
                .start_dynamic_workflow_with_progress_tx(
                    session_id,
                    &input.name,
                    input.spec,
                    &input.arguments,
                    tx.cloned(),
                )
                .await?;
            if let Some(tx) = tx
                && let Ok(Some(record)) =
                    read_background_task_record(&self.config.home_dir, &task_id).await
            {
                let _ = tx.send(StreamEvent::BackgroundTaskUpdated {
                    session_id: session_id.to_string(),
                    task: task_record_to_view(&record),
                });
            }
            Ok::<_, CoreError>(task_id)
        }
        .await;

        match result {
            Ok(task_id) => Ok(BufferedToolUseCompletion {
                outcome: ToolUseOutcome::Continue,
                result: BufferedToolResult {
                    tool_use_id: tool_use_id.to_string(),
                    tool_name: "Workflow".to_string(),
                    content: serde_json::json!({
                        "task_id": task_id,
                        "status": "started",
                        "message": "Dynamic workflow started. Use TaskOutput with task_id to inspect final output and TaskStop to cancel."
                    })
                    .to_string(),
                    is_error: false,
                    metadata: None,
                    completion_kind: ToolUseCompletionKind::Success,
                },
            }),
            Err(error) => Ok(BufferedToolUseCompletion {
                outcome: ToolUseOutcome::Continue,
                result: BufferedToolResult {
                    tool_use_id: tool_use_id.to_string(),
                    tool_name: "Workflow".to_string(),
                    content: error.to_string(),
                    is_error: true,
                    metadata: None,
                    completion_kind: ToolUseCompletionKind::ExecutionFailed,
                },
            }),
        }
    }
}

#[async_trait]
impl ToolRuntimeHost for SessionManager {
    async fn lookup_tool_spec_or_append_unknown(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolLookupOutcome, CoreError> {
        SessionManager::lookup_tool_spec_or_append_unknown(
            self,
            session_id,
            tool_use_id,
            tool_name,
            tx,
        )
        .await
    }

    fn permission_context(&self, session_id: &str) -> PermissionContext {
        SessionManager::permission_context_for_session(self, session_id)
    }

    async fn tool_deny_precedence_reason(
        &self,
        permissions: &PermissionContext,
        tool_name: &str,
        tool_input: &str,
        stage: ToolDenyPrecedenceStage,
    ) -> Option<String> {
        SessionManager::tool_deny_precedence_reason(self, permissions, tool_name, tool_input, stage)
            .await
    }

    async fn run_pre_tool_phase(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<PreToolPhaseOutcome, CoreError> {
        SessionManager::run_pre_tool_phase(self, session_id, tool_use_id, tool_name, tool_input, tx)
            .await
    }

    async fn deny_tool_use(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        reason: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError> {
        SessionManager::deny_tool_use_with_result(
            self,
            session_id,
            tool_use_id,
            tool_name,
            tool_input,
            reason,
            tx,
        )
        .await
    }

    async fn matches_permission_rule(&self, tool_name: &str, tool_input: &str) -> bool {
        self.permission_runtime
            .matches_permission_rule(tool_name, tool_input)
            .await
    }

    async fn resolve_tool_permission_request(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        spec: &ToolSpec,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> ToolPermissionResolutionOutcome {
        SessionManager::resolve_tool_permission_request(
            self,
            session_id,
            tool_use_id,
            tool_name,
            tool_input,
            spec,
            tx,
            cancel_flag,
        )
        .await
    }

    async fn resolve_mcp_trust_if_needed(
        &self,
        session_id: &str,
        _tool_use_id: &str,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> McpTrustResolutionOutcome {
        let Some((server_id, mcp_tool_name)) = parse_mcp_provider_tool_name(tool_name) else {
            return McpTrustResolutionOutcome::Proceed;
        };

        let Some(trust) = self
            .mcp
            .server_trust_for_session(session_id, server_id)
            .await
        else {
            return McpTrustResolutionOutcome::Denied;
        };
        match trust {
            McpServerTrust::Trusted => return McpTrustResolutionOutcome::Proceed,
            McpServerTrust::Denied => return McpTrustResolutionOutcome::Denied,
            McpServerTrust::Unknown => {}
        }

        let request_id = Uuid::new_v4().to_string();
        let _ = tx.send(StreamEvent::McpTrustApprovalRequested {
            request: McpTrustApprovalRequest {
                request_id: request_id.clone(),
                session_id: session_id.to_string(),
                server_id: server_id.to_string(),
                tool_name: mcp_tool_name.to_string(),
            },
        });

        // Poll until the trust state changes (set by the pump via
        // set_mcp_server_trust) or until the turn is cancelled.
        const POLL_INTERVAL: Duration = Duration::from_millis(100);
        const TIMEOUT: Duration = Duration::from_secs(300);
        let deadline = tokio::time::Instant::now() + TIMEOUT;

        loop {
            if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = tx.send(StreamEvent::McpTrustApprovalResolved {
                    session_id: session_id.to_string(),
                    request_id,
                    kind: McpTrustResolutionKind::Interrupted,
                });
                return McpTrustResolutionOutcome::Interrupted;
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = tx.send(StreamEvent::McpTrustApprovalResolved {
                    session_id: session_id.to_string(),
                    request_id,
                    kind: McpTrustResolutionKind::Denied,
                });
                return McpTrustResolutionOutcome::Denied;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = tx.send(StreamEvent::McpTrustApprovalResolved {
                    session_id: session_id.to_string(),
                    request_id,
                    kind: McpTrustResolutionKind::Interrupted,
                });
                return McpTrustResolutionOutcome::Interrupted;
            }
            let Some(current) = self
                .mcp
                .server_trust_for_session(session_id, server_id)
                .await
            else {
                let _ = tx.send(StreamEvent::McpTrustApprovalResolved {
                    session_id: session_id.to_string(),
                    request_id,
                    kind: McpTrustResolutionKind::Denied,
                });
                return McpTrustResolutionOutcome::Denied;
            };
            match current {
                McpServerTrust::Trusted => {
                    let _ = tx.send(StreamEvent::McpTrustApprovalResolved {
                        session_id: session_id.to_string(),
                        request_id,
                        kind: McpTrustResolutionKind::Trusted,
                    });
                    return McpTrustResolutionOutcome::Trusted;
                }
                McpServerTrust::Denied => {
                    let _ = tx.send(StreamEvent::McpTrustApprovalResolved {
                        session_id: session_id.to_string(),
                        request_id,
                        kind: McpTrustResolutionKind::Denied,
                    });
                    return McpTrustResolutionOutcome::Denied;
                }
                McpServerTrust::Unknown => {}
            }
        }
    }

    async fn append_initial_tool_progress_event(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        self.append_tool_progress_event(
            session_id,
            tool_use_id,
            tool_name,
            initial_tool_progress_record(tool_use_id, tool_name, tool_input),
            tx,
        )
        .await
    }

    fn live_tool_progress_reporter(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Arc<dyn ToolProgressReporter> {
        Arc::new(LiveToolProgressReporter {
            manager: self.clone(),
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            tx: tx.clone(),
        })
    }

    fn tool_context(
        &self,
        session_id: &str,
        allow_tools: bool,
        allow_network: bool,
        progress: Arc<dyn ToolProgressReporter>,
        cancel_flag: Arc<AtomicBool>,
    ) -> ToolContext {
        let config = self.effective_config();
        ToolContext {
            cwd: config.cwd.clone(),
            additional_directories: self.additional_directories(),
            home_dir: config.home_dir.clone(),
            sandbox_mode: config.sandbox_mode,
            sandbox_allow_network: config.sandbox_allow_network,
            allow_network,
            allow_tools,
            mcp: self.mcp.clone(),
            progress: Some(progress),
            cancellation: ToolCancellationToken::from_flag(cancel_flag),
            read_state: Some(Arc::clone(&self.read_state)),
            session_id: Some(session_id.to_string()),
            local_shell_tasks: Some(self.local_shell_tasks().clone()),
            on_cwd_change: {
                let runtime_state = self.runtime_state.clone();
                Some(std::sync::Arc::new(move |new_cwd: &std::path::Path| {
                    runtime_state.set_cwd_override(Some(new_cwd.to_path_buf()));
                }))
            },
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: config.settings.env.clone(),
            skill_definitions: None,
        }
    }

    async fn skill_definitions(&self, session_id: &str) -> Vec<SkillDefinition> {
        self.skill_definitions_visible_to_session(session_id).await
    }

    fn ask_user_pending(&self) -> InteractionRuntime {
        self.interaction_runtime.clone()
    }

    async fn active_interaction_context(
        &self,
        session_id: &str,
    ) -> Option<(Uuid, crate::TurnInteractionContext)> {
        self.active_turns.interaction_context(session_id).await
    }

    async fn tool_success_result_details(
        &self,
        session_id: &str,
        tool_use_id: &str,
        outcome: &ToolOutcome,
    ) -> Result<ToolSuccessResultDetails, CoreError> {
        let content = self
            .maybe_persist_large_tool_result(
                session_id,
                tool_use_id,
                &outcome.name,
                tool_result_content(outcome),
            )
            .await?;
        Ok(ToolSuccessResultDetails {
            content,
            metadata: tool_result_metadata(outcome),
        })
    }

    async fn run_agent_tool(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolUseOutcome, CoreError> {
        self.invoke_agent_tool_and_append_result(
            session_id,
            tool_use_id,
            tool_input,
            allow_tools,
            allow_network,
            tx,
            cancel_flag,
        )
        .await
    }

    async fn run_workflow_tool(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError> {
        let completion = self
            .invoke_workflow_tool_and_buffer_result(session_id, tool_use_id, tool_input, Some(tx))
            .await?;
        self.append_tool_result(
            session_id,
            tool_use_id,
            completion.result.content,
            completion.result.is_error,
            completion.result.metadata,
            tx,
        )
        .await?;
        self.emit_tool_use_completed(
            session_id,
            tool_use_id,
            "Workflow",
            completion.result.completion_kind,
            tx,
        );
        Ok(completion.outcome)
    }

    async fn run_agent_tool_buffered(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<BufferedToolUseCompletion, CoreError> {
        self.invoke_agent_tool_and_buffer_result(
            session_id,
            tool_use_id,
            tool_input,
            allow_tools,
            allow_network,
            tx,
            cancel_flag,
        )
        .await
    }

    async fn run_workflow_tool_buffered(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<BufferedToolUseCompletion, CoreError> {
        self.invoke_workflow_tool_and_buffer_result(session_id, tool_use_id, tool_input, Some(tx))
            .await
    }

    async fn append_tool_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        content: impl Into<String> + Send,
        is_error: bool,
        metadata: Option<String>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        self.append_tool_result_message(session_id, tool_use_id, content, is_error, metadata, tx)
            .await
    }

    async fn append_post_tool_contexts(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        outcome: &ToolOutcome,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        self.append_post_tool_hook_contexts(
            session_id,
            tool_use_id,
            tool_name,
            tool_input,
            outcome,
            tx,
        )
        .await
    }

    async fn append_post_tool_failure_contexts(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        error_message: &str,
        is_interrupt: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        self.append_post_tool_failure_hook_contexts(
            session_id,
            tool_use_id,
            tool_name,
            tool_input,
            error_message,
            is_interrupt,
            tx,
        )
        .await
    }

    fn emit_tool_use_started(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        SessionManager::emit_tool_use_started(
            self,
            session_id,
            tool_use_id,
            tool_name,
            tool_input,
            tx,
        );
    }

    fn emit_tool_use_completed(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        kind: ToolUseCompletionKind,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        SessionManager::emit_tool_use_completed(self, session_id, tool_use_id, tool_name, kind, tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_workflow_tool_input_accepts_valid_object() {
        let input = parse_workflow_tool_input(
            r#"{"name":"dynamic:ok","spec":{"schema_version":1,"steps":[{"log":{"message":"ok"}}]}}"#,
        )
        .expect("workflow input");

        assert_eq!(input.name, "dynamic:ok");
        assert_eq!(input.spec["schema_version"], 1);
    }

    #[test]
    fn parse_workflow_tool_input_accepts_json_object_encoded_as_string() {
        let encoded = serde_json::to_string(
            r#"{"name":"dynamic:quoted","spec":{"schema_version":1,"steps":[{"log":{"message":"ok"}}]}}"#,
        )
        .expect("encode quoted workflow input");
        let input = parse_workflow_tool_input(&encoded).expect("workflow input");

        assert_eq!(input.name, "dynamic:quoted");
        assert_eq!(input.spec["steps"][0]["log"]["message"], "ok");
    }

    #[test]
    fn parse_workflow_tool_input_repairs_missing_step_object_closes() {
        let input = parse_workflow_tool_input(
            r#"{"name":"dynamic:bad","spec":{"schema_version":1,"steps":[{"parallel":{"steps":[{"agent":{"description":"task2","prompt":"done"}, {"agent":{"description":"task3","prompt":"done"}}]}}]}}"#,
        )
        .expect("repaired workflow input");

        let steps = input.spec["steps"][0]["parallel"]["steps"]
            .as_array()
            .expect("parallel steps");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["agent"]["description"], "task2");
        assert_eq!(steps[1]["agent"]["description"], "task3");
    }

    #[test]
    fn parse_workflow_tool_input_repairs_real_model_ui_verification_shape() {
        let input = parse_workflow_tool_input(
            r#"{"name":"dynamic:ui-verify","spec":{"description":"Verify UI rendering with sequential, parallel, and final stages","schema_version":1,"steps":[{"agent":{"description":"task1: Initial sequential step","prompt":"task1"}},{"parallel":{"steps":[{"agent":{"description":"task2: First parallel branch","prompt":"task2"}, {"agent":{"description":"task3: Second parallel branch","prompt":"task3"}}]},{"agent":{"description":"task4: Final sequential step","prompt":"task4"}]}}"#,
        )
        .expect("repaired workflow input");

        let steps = input.spec["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 3);
        assert_eq!(
            steps[0]["agent"]["description"],
            "task1: Initial sequential step"
        );
        assert_eq!(
            steps[1]["parallel"]["steps"][0]["agent"]["description"],
            "task2: First parallel branch"
        );
        assert_eq!(
            steps[1]["parallel"]["steps"][1]["agent"]["description"],
            "task3: Second parallel branch"
        );
        assert_eq!(
            steps[2]["agent"]["description"],
            "task4: Final sequential step"
        );
    }

    #[test]
    fn parse_workflow_tool_input_reports_context_for_unrecoverable_json() {
        let error = parse_workflow_tool_input(
            r#"{"name":"dynamic:bad","spec":{"schema_version":1,"steps":[{"parallel":{"steps":[{"agent":{"description":"task2","prompt":"done"}, {"agent": ]}}]}}"#,
        )
        .expect_err("unrecoverable workflow input");
        let message = error.to_string();

        assert!(message.contains("expected a valid JSON object"));
        assert!(message.contains("Near error:"));
        assert!(message.contains("task2"));
        assert!(message.contains("For parallel agent steps"));
    }
}
