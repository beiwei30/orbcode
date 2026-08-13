mod context;
mod diagnostics;
mod settings;

use std::collections::HashMap;

use orbcode_app_server_client::ProviderRequestDebugSnapshot;
#[cfg(test)]
use orbcode_protocol::TurnContext;
use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Typed structs for provider activity trace deserialization
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct ActivityRecord {
    #[serde(rename = "type", default)]
    activity_type: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    messages: Vec<ActivityMessage>,
}

#[derive(Deserialize, Default)]
struct ActivityMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Deserialize, Default)]
struct ContentBlock {
    #[serde(rename = "type", default)]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct ToolCall {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ToolCallFunction>,
}

#[derive(Deserialize, Default)]
struct ToolCallFunction {
    #[serde(default)]
    name: Option<String>,
}

pub(crate) use context::render_context_overview;
pub(crate) use diagnostics::{
    render_cost_overview, render_doctor_report, render_stats_overview, render_stats_summary,
    render_usage_overview,
};
pub(crate) use settings::{
    format_release_notes, parse_changelog_release_notes, render_agent_definitions_with_warnings,
    render_auth_overview, render_hook_discovery, render_plan_overview, render_skill_definitions,
    render_status_overview, workspace_diff_changed_path_count,
};
#[cfg(test)]
pub(crate) use settings::{
    render_agent_definitions, render_memory_overview, render_workspace_diff,
};

pub(crate) const LAST_REQUEST_BODY_PREVIEW_CHARS: usize = 20_000;

fn format_context_tokens(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format_scaled_tokens(tokens, 1_000_000, "m")
    } else if tokens >= 1_000 {
        format_scaled_tokens(tokens, 1_000, "k")
    } else {
        tokens.to_string()
    }
}

fn format_scaled_tokens(tokens: u32, scale: u32, suffix: &str) -> String {
    let whole = tokens / scale;
    let tenth = ((tokens % scale) * 10 + scale / 2) / scale;
    if tenth == 0 {
        format!("{whole}{suffix}")
    } else if tenth == 10 {
        format!("{}{suffix}", whole + 1)
    } else {
        format!("{whole}.{tenth}{suffix}")
    }
}

#[cfg(test)]
pub(crate) fn render_turn_context(context: &TurnContext) -> String {
    let mut lines = vec![
        "Context snapshot:".to_string(),
        format!("cwd: {}", context.cwd),
        format!("date: {}", context.current_date),
        format!(
            "repo root: {}",
            context.repo_root.as_deref().unwrap_or("not available")
        ),
        format!(
            "repo subdir: {}",
            context
                .cwd_relative_to_repo
                .as_deref()
                .unwrap_or("not available")
        ),
        format!(
            "additional directories: {}",
            context.additional_directories.len()
        ),
        format!(
            "git branch: {}",
            context.git_branch.as_deref().unwrap_or("not available")
        ),
        format!(
            "git status: {}",
            context
                .git_status
                .as_deref()
                .unwrap_or("clean or unavailable")
        ),
        format!(
            "AGENTS.md: {}",
            context
                .claude_md
                .as_ref()
                .map_or("not available", |_| "loaded")
        ),
    ];
    if let Some(claude_md) = context.claude_md.as_deref() {
        lines.push(String::new());
        lines.push("AGENTS.md contents:".to_string());
        lines.push(claude_md.to_string());
    }
    if !context.additional_directories.is_empty() {
        lines.push(String::new());
        lines.push("Additional directories:".to_string());
        for directory in &context.additional_directories {
            lines.push(format!("  - {directory}"));
        }
    }
    lines.join("\n")
}

pub(crate) fn render_last_provider_request_snapshot(
    snapshot: &ProviderRequestDebugSnapshot,
) -> (String, String) {
    let mut detail = render_provider_request_body_section(snapshot);
    let recent_activity = render_recent_activity_trace(&snapshot.recent_activity_json);
    if !recent_activity.trim().is_empty() {
        detail.push_str("\n\n● Recent activity\n");
        detail.push_str(&recent_activity);
    }

    ("Recent LLM activity loaded.".to_string(), detail)
}

pub(crate) fn render_provider_request_body_section(
    snapshot: &ProviderRequestDebugSnapshot,
) -> String {
    let body = truncate_last_request_body_for_ui(&snapshot.body_json);
    format!(
        "● Provider request body\nprovider: {}\nsource: {}\nmodel: {}\nbase_url: {}\nsession_id: {}\ncaptured_at: {}\n{}",
        snapshot.provider,
        snapshot.source,
        snapshot.model,
        snapshot.base_url,
        snapshot.session_id,
        snapshot.captured_at,
        body
    )
}

fn truncate_last_request_body_for_ui(body_json: &str) -> String {
    let total_chars = body_json.chars().count();
    if total_chars <= LAST_REQUEST_BODY_PREVIEW_CHARS {
        return body_json.to_string();
    }

    let head_chars = LAST_REQUEST_BODY_PREVIEW_CHARS * 3 / 4;
    let tail_chars = LAST_REQUEST_BODY_PREVIEW_CHARS - head_chars;
    let (head, tail, omitted) = split_preview_on_line_boundaries(body_json, head_chars, tail_chars);
    format!(
        "{head}\n\n[Provider request body truncated for interactive responsiveness. Omitted {omitted} middle characters.]\n\n{tail}"
    )
}

fn split_preview_on_line_boundaries(
    content: &str,
    head_chars: usize,
    tail_chars: usize,
) -> (String, String, usize) {
    let chars = content.chars().collect::<Vec<_>>();
    let total_chars = chars.len();
    let initial_head_end = head_chars.min(total_chars);
    let head_end = if initial_head_end >= total_chars
        || chars.get(initial_head_end.saturating_sub(1)) == Some(&'\n')
    {
        initial_head_end
    } else {
        chars[..initial_head_end]
            .iter()
            .rposition(|ch| *ch == '\n')
            .map_or(initial_head_end, |index| index + 1)
    };

    let initial_tail_start = total_chars.saturating_sub(tail_chars);
    let tail_start = if initial_tail_start == 0 || chars.get(initial_tail_start) == Some(&'\n') {
        initial_tail_start
    } else {
        chars[initial_tail_start..]
            .iter()
            .position(|ch| *ch == '\n')
            .map_or(initial_tail_start, |offset| initial_tail_start + offset + 1)
    };

    let head = chars[..head_end].iter().collect::<String>();
    let tail = chars[tail_start..].iter().collect::<String>();
    let omitted = tail_start.saturating_sub(head_end);
    (head, tail, omitted)
}

pub(crate) fn render_recent_activity_trace(recent_activity_json: &str) -> String {
    let Ok(raw_activities) = serde_json::from_str::<Vec<Value>>(recent_activity_json) else {
        return recent_activity_json.to_string();
    };
    if raw_activities.is_empty() {
        return "No recent activity recorded.".to_string();
    }

    let records: Vec<ActivityRecord> = raw_activities
        .iter()
        .map(|v| serde_json::from_value(v.clone()).unwrap_or_default())
        .collect();

    let tool_names = recent_activity_tool_names(&records);
    raw_activities
        .iter()
        .zip(records.iter())
        .map(|(raw, record)| render_recent_activity_json_item(raw, record, &tool_names))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_recent_activity_json_item(
    raw: &Value,
    record: &ActivityRecord,
    tool_names: &HashMap<String, String>,
) -> String {
    let activity_type = if record.activity_type.is_empty() {
        "activity"
    } else {
        &record.activity_type
    };
    let label = if record.label.is_empty() {
        "activity"
    } else {
        &record.label
    };
    let title = recent_activity_flow_title(activity_type, label, record, tool_names);
    let pretty_json = serde_json::to_string_pretty(raw).unwrap_or_else(|_| raw.to_string());
    format!("● {title}\n{pretty_json}")
}

fn recent_activity_flow_title(
    activity_type: &str,
    label: &str,
    record: &ActivityRecord,
    tool_names: &HashMap<String, String>,
) -> String {
    match activity_type {
        "assistant_response_from_llm" => "LLM -> Orb Code".to_string(),
        "tool_result_to_llm" => {
            let tool_name = record
                .messages
                .iter()
                .find_map(|message| {
                    message_tool_result_id(message)
                        .and_then(|id| tool_names.get(id).map(String::as_str))
                })
                .unwrap_or("Tool");
            format!("{tool_name} -> Orb Code -> LLM")
        }
        "hook_context_to_llm" | "hook_feedback_to_llm" | "hook_retry_context_to_llm" => {
            format!("{} -> LLM", hook_title_from_label(label))
        }
        "hook_notice_to_orbcode" => {
            let hook = record.hook_event_name.as_deref().unwrap_or("Hook");
            format!("{hook} hook -> Orb Code")
        }
        "interruption_to_llm" => "Interrupt -> Orb Code -> LLM".to_string(),
        _ => "Orb Code -> LLM".to_string(),
    }
}

fn hook_title_from_label(label: &str) -> String {
    label
        .split_once(" hook")
        .map_or_else(|| "Hook".to_string(), |(hook, _)| format!("{hook} hook"))
}

fn recent_activity_tool_names(records: &[ActivityRecord]) -> HashMap<String, String> {
    let mut tool_names = HashMap::new();
    for record in records {
        for message in &record.messages {
            collect_message_tool_names(message, &mut tool_names);
        }
    }
    tool_names
}

fn collect_message_tool_names(message: &ActivityMessage, tool_names: &mut HashMap<String, String>) {
    for block in &message.content {
        if block.block_type == "tool_use"
            && let (Some(id), Some(name)) = (&block.id, &block.name)
        {
            tool_names.insert(id.clone(), name.clone());
        }
    }
    for call in &message.tool_calls {
        if let (Some(id), Some(name)) = (
            &call.id,
            call.function.as_ref().and_then(|f| f.name.as_ref()),
        ) {
            tool_names.insert(id.clone(), name.clone());
        }
    }
}

fn message_tool_result_id(message: &ActivityMessage) -> Option<&str> {
    message.content.iter().find_map(|block| {
        (block.block_type == "tool_result")
            .then_some(block.tool_use_id.as_deref())
            .flatten()
    })
}
