use agent_client_protocol::schema::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate, ToolCall, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{Client, ConnectionTo, Result};
use orbcode_protocol::{ToolUseCompletionKind, format_tool_title};

pub(super) fn send_agent_text(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    text: impl Into<String>,
) -> Result<()> {
    let update =
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text.into())));
    connection.send_notification(SessionNotification::new(session_id.to_string(), update))
}

pub(super) fn send_session_update(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    update: SessionUpdate,
) -> Result<()> {
    connection.send_notification(SessionNotification::new(session_id.to_string(), update))
}

pub(super) fn tool_call_update(
    tool_call_id: &str,
    title: impl Into<String>,
    tool_name: &str,
    tool_input: &str,
) -> ToolCallUpdate {
    let raw_input = serde_json::from_str(tool_input)
        .unwrap_or_else(|_| serde_json::Value::String(tool_input.to_string()));
    ToolCallUpdate::new(
        tool_call_id.to_string(),
        ToolCallUpdateFields::new()
            .kind(tool_kind_for(tool_name))
            .status(ToolCallStatus::Pending)
            .title(title.into())
            .raw_input(raw_input),
    )
}

pub(super) fn tool_call_started(tool_call_id: &str, tool_name: &str, tool_input: &str) -> ToolCall {
    let raw_input = serde_json::from_str(tool_input)
        .unwrap_or_else(|_| serde_json::Value::String(tool_input.to_string()));
    ToolCall::new(
        tool_call_id.to_string(),
        format_tool_title(tool_name, tool_input),
    )
    .kind(tool_kind_for(tool_name))
    .status(ToolCallStatus::Pending)
    .raw_input(raw_input)
}

pub(super) fn tool_call_replay_started(
    tool_call_id: &str,
    tool_name: &str,
    tool_input: &str,
) -> ToolCall {
    tool_call_started(tool_call_id, tool_name, tool_input)
}

pub(super) fn tool_result_replay_update(
    tool_call_id: &str,
    content: &str,
    is_error: bool,
) -> ToolCallUpdate {
    let status = if is_error {
        ToolCallStatus::Failed
    } else {
        ToolCallStatus::Completed
    };
    ToolCallUpdate::new(
        tool_call_id.to_string(),
        ToolCallUpdateFields::new()
            .status(status)
            .raw_output(serde_json::Value::String(content.to_string())),
    )
}

pub(super) fn tool_progress_update(
    tool_call_id: &str,
    tool_name: &str,
    progress: serde_json::Value,
    cached_title: Option<&str>,
) -> ToolCallUpdate {
    let mut fields = ToolCallUpdateFields::new()
        .kind(tool_kind_for(tool_name))
        .status(ToolCallStatus::InProgress)
        .raw_output(progress.clone());
    if let Some(title) = extract_progress_title(&progress) {
        fields = fields.title(title);
    } else if let Some(cached) = cached_title {
        fields = fields.title(cached.to_string());
    } else if let Some(status) = progress_status_fallback(&progress) {
        fields = fields.title(status);
    }
    ToolCallUpdate::new(tool_call_id.to_string(), fields)
}

pub(super) fn tool_completion_update(
    tool_call_id: &str,
    kind: ToolUseCompletionKind,
) -> ToolCallUpdate {
    ToolCallUpdate::new(
        tool_call_id.to_string(),
        ToolCallUpdateFields::new()
            .status(tool_completion_status(kind))
            .raw_output(serde_json::json!({ "kind": kind.as_str() })),
    )
}

fn tool_completion_status(kind: ToolUseCompletionKind) -> ToolCallStatus {
    match kind {
        ToolUseCompletionKind::Success => ToolCallStatus::Completed,
        _ => ToolCallStatus::Failed,
    }
}

pub(super) fn extract_progress_title(progress: &serde_json::Value) -> Option<String> {
    let data = progress.pointer("/data").or(Some(progress));
    let progress_type = data
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str);
    match progress_type {
        Some("tool_progress" | "agent_progress") => {
            if let Some(title) = title_from_assistant_message(data) {
                return Some(title);
            }
            if let Some(title) = title_from_user_tool_result(data) {
                return Some(title);
            }
            None
        }
        // bash_progress carries no command data — caller should fall back to
        // a cached title from the parent agent_progress record.
        Some("bash_progress") => None,
        _ => None,
    }
}

fn title_from_assistant_message(data: Option<&serde_json::Value>) -> Option<String> {
    let message = data?.get("message")?;
    if message.get("type").and_then(serde_json::Value::as_str) != Some("assistant") {
        return None;
    }
    let content = message.pointer("/message/content")?.as_array()?;
    let block = content
        .iter()
        .find(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use"))?;
    let name = block.get("name").and_then(serde_json::Value::as_str)?;
    let input = block
        .get("input")
        .map_or_else(String::new, ToString::to_string);
    Some(format_tool_title(name, &input))
}

fn title_from_user_tool_result(data: Option<&serde_json::Value>) -> Option<String> {
    let message = data?.get("message")?;
    if message.get("type").and_then(serde_json::Value::as_str) != Some("user") {
        return None;
    }
    let tool_use_result = message.get("toolUseResult")?;
    if let Some(summary) = tool_use_result
        .get("summary")
        .and_then(serde_json::Value::as_str)
        && !summary.trim().is_empty()
    {
        return Some(summary.to_string());
    }
    let tool_name = tool_use_result
        .get("toolName")
        .and_then(serde_json::Value::as_str)?;
    Some(format!("{tool_name} completed"))
}

fn progress_status_fallback(progress: &serde_json::Value) -> Option<String> {
    progress
        .pointer("/data/status")
        .or_else(|| progress.get("status"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

pub(super) fn tool_kind_for(tool_name: &str) -> ToolKind {
    match tool_name {
        "Bash" | "bash" => ToolKind::Execute,
        "Read" => ToolKind::Read,
        "Write" | "Edit" | "MultiEdit" => ToolKind::Edit,
        "Glob" | "Grep" | "LS" => ToolKind::Search,
        "WebFetch" | "WebSearch" => ToolKind::Fetch,
        "TodoWrite" | "Task" | "Agent" => ToolKind::Think,
        _ => ToolKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::ToolUseCompletionKind;
    use serde_json::json;

    #[test]
    fn tool_use_started_maps_to_acp_tool_call() {
        let call = tool_call_started("toolu-1", "Bash", r#"{"command":"ls -la"}"#);

        assert_eq!(call.tool_call_id.to_string(), "toolu-1");
        assert_eq!(call.title, "Bash(ls -la)");
        assert_eq!(call.kind, ToolKind::Execute);
        assert_eq!(call.status, ToolCallStatus::Pending);
        assert_eq!(
            call.raw_input,
            Some(serde_json::json!({"command": "ls -la"}))
        );
    }

    #[test]
    fn tool_use_started_with_empty_input_falls_back_to_tool_name() {
        let call = tool_call_started("toolu-2", "Bash", "");

        assert_eq!(call.title, "Bash");
        assert_eq!(call.kind, ToolKind::Execute);
        assert_eq!(call.status, ToolCallStatus::Pending);
        assert_eq!(
            call.raw_input,
            Some(serde_json::Value::String(String::new()))
        );
    }

    #[test]
    fn tool_use_started_agent_uses_descriptive_title() {
        let call = tool_call_started(
            "toolu-3",
            "Agent",
            r#"{"subagent_type":"Explore","description":"orbcode project structure"}"#,
        );

        assert_eq!(call.title, "Explore(orbcode project structure)");
        assert_eq!(call.kind, ToolKind::Think);
    }

    #[test]
    fn tool_progress_status_only_falls_back_to_status_string() {
        let progress = json!({
            "data": {
                "status": "Running 1 bash command",
            },
        });

        let update = tool_progress_update("toolu-2", "Bash", progress.clone(), None);

        assert_eq!(update.tool_call_id.to_string(), "toolu-2");
        assert_eq!(update.fields.kind, Some(ToolKind::Execute));
        assert_eq!(update.fields.status, Some(ToolCallStatus::InProgress));
        assert_eq!(
            update.fields.title,
            Some("Running 1 bash command".to_string())
        );
        assert_eq!(update.fields.raw_output, Some(progress));
    }

    #[test]
    fn tool_progress_assistant_message_yields_descriptive_title() {
        let progress = json!({
            "data": {
                "type": "agent_progress",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "child-1",
                                "name": "Read",
                                "input": { "file_path": "/repo/Cargo.toml" }
                            }
                        ]
                    }
                }
            }
        });

        let update = tool_progress_update("toolu-3", "Agent", progress, None);
        assert_eq!(
            update.fields.title,
            Some("Read(/repo/Cargo.toml)".to_string())
        );
    }

    #[test]
    fn tool_progress_user_tool_result_uses_summary_or_completed() {
        let with_summary = json!({
            "data": {
                "type": "agent_progress",
                "message": {
                    "type": "user",
                    "toolUseResult": {
                        "summary": "Read 42 lines",
                        "toolName": "Read"
                    }
                }
            }
        });
        let update = tool_progress_update("toolu-4", "Agent", with_summary, None);
        assert_eq!(update.fields.title, Some("Read 42 lines".to_string()));

        let without_summary = json!({
            "data": {
                "type": "agent_progress",
                "message": {
                    "type": "user",
                    "toolUseResult": { "toolName": "Bash" }
                }
            }
        });
        let update = tool_progress_update("toolu-5", "Agent", without_summary, None);
        assert_eq!(update.fields.title, Some("Bash completed".to_string()));
    }

    #[test]
    fn bash_progress_uses_cached_title_when_available() {
        let progress = json!({
            "data": {
                "type": "bash_progress",
                "status": "streaming stdout"
            }
        });

        let cached = "Bash(ls -la /tmp)".to_string();
        let update = tool_progress_update("toolu-6", "Bash", progress.clone(), Some(&cached));
        assert_eq!(update.fields.title, Some(cached));
    }

    #[test]
    fn bash_progress_without_cache_falls_back_to_status() {
        let progress = json!({
            "data": {
                "type": "bash_progress",
                "status": "streaming stdout"
            }
        });

        let update = tool_progress_update("toolu-7", "Bash", progress, None);
        assert_eq!(update.fields.title, Some("streaming stdout".to_string()));
    }

    #[test]
    fn extract_progress_title_handles_initial_tool_progress() {
        let progress = json!({
            "data": {
                "type": "tool_progress",
                "status": "Reading 1 file",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "tu-1",
                                "name": "Bash",
                                "input": { "command": "ls -la" }
                            }
                        ]
                    }
                }
            }
        });

        let title = extract_progress_title(&progress);
        assert_eq!(title, Some("Bash(ls -la)".to_string()));
    }

    #[test]
    fn extract_progress_title_returns_none_for_bash_progress() {
        let progress = json!({
            "data": {
                "type": "bash_progress",
                "status": "streaming stdout"
            }
        });

        assert_eq!(extract_progress_title(&progress), None);
    }

    #[test]
    fn tool_completion_maps_success_and_failures_to_acp_status() {
        let success = tool_completion_update("toolu-3", ToolUseCompletionKind::Success);
        let failed = tool_completion_update("toolu-4", ToolUseCompletionKind::ExecutionFailed);
        let denied = tool_completion_update("toolu-5", ToolUseCompletionKind::PermissionDenied);

        assert_eq!(success.fields.status, Some(ToolCallStatus::Completed));
        assert_eq!(failed.fields.status, Some(ToolCallStatus::Failed));
        assert_eq!(denied.fields.status, Some(ToolCallStatus::Failed));
        assert_eq!(
            success.fields.raw_output,
            Some(json!({ "kind": "success" }))
        );
    }
}
