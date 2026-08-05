use std::fmt::Write as _;
use std::path::Path;

use ratatui::prelude::Style;
use serde::Deserialize;
use serde_json::Value;

use crate::embedded_progress::should_render_tool_progress_message;
use crate::render::permission_labels::grep_regex_display_line;
use crate::render::text_utils::{
    collapse_inline_whitespace, format_duration_short, push_unique_line, truncate_chars,
};
use crate::tool_cell::utils::{display_tool_path, first_string_field, parse_tool_input};
use crate::tui_theme::{active_palette, inactive_style};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolResultMetadata {
    pub(crate) summary: Option<String>,
    pub(crate) total_tool_use_count: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
    pub(crate) total_duration_ms: Option<u64>,
    #[allow(dead_code)]
    pub(crate) duration_ms: Option<u64>,
    pub(crate) diff: Option<String>,
    pub(crate) diff_truncated: Option<bool>,
    pub(crate) lines_added: Option<u64>,
    pub(crate) lines_removed: Option<u64>,
    pub(crate) usage: Option<ToolResultUsage>,
    #[allow(dead_code)]
    pub(crate) bash: Option<BashMetadata>,
    pub(crate) content: Option<Vec<ContentBlock>>,
    pub(crate) changed_paths: Option<Vec<String>>,
    #[serde(alias = "progress_messages")]
    pub(crate) progress_messages: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolResultUsage {
    pub(crate) output_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlock {
    pub(crate) text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct BashMetadata {
    pub(crate) duration_ms: Option<u64>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) timed_out: Option<bool>,
    pub(crate) exit_code: Option<i64>,
    pub(crate) signal: Option<i64>,
    pub(crate) interrupted: Option<bool>,
    pub(crate) output_truncated: Option<bool>,
    pub(crate) omitted_chars: Option<u64>,
    pub(crate) workspace_impact: Option<BashWorkspaceImpact>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct BashWorkspaceImpact {
    pub(crate) git: Option<BashGitImpact>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct BashGitImpact {
    pub(crate) repo_initialized: Option<bool>,
    pub(crate) repo_removed: Option<bool>,
    pub(crate) branch_changed: Option<bool>,
    pub(crate) pre_branch: Option<String>,
    pub(crate) post_branch: Option<String>,
    pub(crate) head_changed: Option<bool>,
    pub(crate) pre_head: Option<String>,
    pub(crate) post_head: Option<String>,
    pub(crate) working_tree_changed: Option<bool>,
    pub(crate) pre_dirty_files: Option<u64>,
    pub(crate) post_dirty_files: Option<u64>,
    pub(crate) dirty_delta: Option<i64>,
}

pub(crate) const SHELL_CWD_RESET_PREFIX: &str = "Shell cwd was reset to ";

pub(crate) fn default_active_tool_status_line(name: &str) -> String {
    if is_file_read_like_tool(name) {
        "Reading…".to_string()
    } else {
        "Running…".to_string()
    }
}

pub(crate) fn format_tool_activity_title(name: &str, input: &str, cwd: &Path) -> String {
    let parsed_input = parse_tool_input(input);
    let lowered = name.to_ascii_lowercase();
    if lowered == "agent" {
        let agent_type =
            first_string_field(parsed_input.as_ref(), &["subagent_type", "subagentType"])
                .map_or_else(
                    || "Agent".to_string(),
                    |agent_type| {
                        if agent_type == "worker" {
                            "Agent".to_string()
                        } else {
                            agent_type
                        }
                    },
                );
        if let Some(description) = first_string_field(parsed_input.as_ref(), &["description"]) {
            return format!("{agent_type}({})", collapse_inline_whitespace(&description));
        }
        return agent_type;
    }

    if matches!(
        lowered.as_str(),
        "taskcreate" | "taskget" | "tasklist" | "taskupdate" | "taskoutput" | "taskstop"
    ) {
        let label = if lowered == "taskcreate" {
            first_string_field(parsed_input.as_ref(), &["title", "description"])
        } else {
            first_string_field(
                parsed_input.as_ref(),
                &["task_id", "taskId", "id", "shell_id", "shellId"],
            )
        };
        if let Some(label) = label {
            return format!("{name}({})", collapse_inline_whitespace(&label));
        }
    }

    if lowered == "skill"
        && let Some(skill_name) = first_string_field(parsed_input.as_ref(), &["name", "skill"])
    {
        return format!("Skill({})", collapse_inline_whitespace(&skill_name));
    }

    if lowered == "toolsearch"
        && let Some(query) = first_string_field(parsed_input.as_ref(), &["query", "pattern"])
    {
        return format!("ToolSearch({})", collapse_inline_whitespace(&query));
    }

    if matches!(lowered.as_str(), "read" | "file-read") {
        if let Some(file_path) =
            first_string_field(parsed_input.as_ref(), &["file_path", "filePath", "path"])
        {
            let mut summary = display_tool_path(&file_path, cwd);
            if let Some(pages) = parsed_input
                .as_ref()
                .and_then(|value| value.get("pages"))
                .and_then(Value::as_u64)
            {
                write!(summary, " · pages {pages}").expect("writing to String cannot fail");
            } else {
                let offset = parsed_input
                    .as_ref()
                    .and_then(|value| value.get("offset"))
                    .and_then(Value::as_u64);
                let limit = parsed_input
                    .as_ref()
                    .and_then(|value| value.get("limit"))
                    .and_then(Value::as_u64);
                if let Some(offset) = offset {
                    if let Some(limit) = limit {
                        write!(
                            summary,
                            " · lines {offset}-{}",
                            offset + limit.saturating_sub(1)
                        )
                        .expect("writing to String cannot fail");
                    } else {
                        write!(summary, " · from line {offset}")
                            .expect("writing to String cannot fail");
                    }
                }
            }
            return format!("Read({})", truncate_chars(&summary, 120));
        }
        return "Read".to_string();
    }

    if matches!(
        lowered.as_str(),
        "grep" | "glob" | "websearch" | "web-search"
    ) {
        if lowered == "grep" {
            let regex = first_string_field(parsed_input.as_ref(), &["pattern", "query"]);
            let path = first_string_field(parsed_input.as_ref(), &["path"]);
            let mut parts = Vec::new();
            if let Some(regex) = regex {
                parts.push(format!("regex: {}", collapse_inline_whitespace(&regex)));
            }
            if let Some(path) = path {
                parts.push(format!("in: {}", collapse_inline_whitespace(&path)));
            }
            if parts.is_empty() {
                return "Search".to_string();
            }
            return format!("Search({})", truncate_chars(&parts.join(", "), 120));
        }

        let mut parts = Vec::new();
        if let Some(pattern) =
            first_string_field(parsed_input.as_ref(), &["pattern", "query", "glob"])
        {
            parts.push(format!(
                "pattern: \"{}\"",
                collapse_inline_whitespace(&pattern)
            ));
        }
        if let Some(path) =
            first_string_field(parsed_input.as_ref(), &["path", "file_path", "filePath"])
        {
            parts.push(format!("path: \"{}\"", collapse_inline_whitespace(&path)));
        }
        if parts.is_empty() {
            return "Search".to_string();
        }
        return format!("Search({})", truncate_chars(&parts.join(", "), 120));
    }

    if matches!(
        lowered.as_str(),
        "edit" | "file-edit" | "update" | "write" | "file-write"
    ) {
        let label = if matches!(lowered.as_str(), "edit" | "file-edit" | "update") {
            "Update"
        } else {
            "Write"
        };
        if let Some(file_path) =
            first_string_field(parsed_input.as_ref(), &["file_path", "filePath", "path"])
        {
            return format!(
                "{label}({})",
                truncate_chars(&display_tool_path(&file_path, cwd), 120)
            );
        }
        return label.to_string();
    }

    if lowered.starts_with("mcp__")
        && let Some((server, tool)) = lowered.strip_prefix("mcp__").and_then(|rest| {
            let parts: Vec<&str> = rest.splitn(2, "__").collect();
            if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                Some((parts[0], parts[1]))
            } else {
                None
            }
        })
    {
        return format!("{server}:{tool}");
    }

    if matches!(lowered.as_str(), "bash" | "shell") {
        if let Some(command) =
            first_string_field(parsed_input.as_ref(), &["command", "cmd", "script"])
        {
            return format!("{name}({})", collapse_inline_whitespace(&command));
        }
        if let Some(description) = first_string_field(parsed_input.as_ref(), &["description"]) {
            return format!("{name}({})", collapse_inline_whitespace(&description));
        }
    }

    if lowered == "lsp"
        && let Some(operation) = first_string_field(parsed_input.as_ref(), &["operation"])
    {
        return format!("LSP({})", collapse_inline_whitespace(&operation));
    }

    if matches!(
        lowered.as_str(),
        "listmcpresourcestool" | "listmcptoolstool" | "readmcpresourcetool" | "callmcptool"
    ) {
        let server = first_string_field(parsed_input.as_ref(), &["server_id", "serverId"]);
        let item = if lowered == "callmcptool" {
            first_string_field(parsed_input.as_ref(), &["tool_name", "tool"])
        } else if lowered == "readmcpresourcetool" {
            first_string_field(parsed_input.as_ref(), &["uri"])
        } else {
            None
        };
        if let Some(server) = server {
            if let Some(item) = item {
                return format!(
                    "{name}({}.{})",
                    collapse_inline_whitespace(&server),
                    collapse_inline_whitespace(&item)
                );
            }
            return format!("{name}({})", collapse_inline_whitespace(&server));
        }
    }

    if let Some(description) = first_string_field(parsed_input.as_ref(), &["description"]) {
        return format!("{name}({})", collapse_inline_whitespace(&description));
    }

    name.to_string()
}

pub(crate) fn tool_activity_title_style(name: &str, input: &str) -> Style {
    let parsed_input = parse_tool_input(input);
    let lowered = name.to_ascii_lowercase();
    if lowered == "agent" {
        let agent_type =
            first_string_field(parsed_input.as_ref(), &["subagent_type", "subagentType"]);
        if matches!(agent_type.as_deref(), Some("Explore")) {
            return Style::default().fg(active_palette().success);
        }
    }

    inactive_style()
}

pub(crate) fn tool_activity_detail_lines(name: &str, input: &str, cwd: &Path) -> Vec<String> {
    if name.eq_ignore_ascii_case("agent") {
        return Vec::new();
    }

    let parsed_input = parse_tool_input(input);
    let mut lines = Vec::new();
    let lowered = name.to_ascii_lowercase();

    if let Some(task_id) = first_string_field(
        parsed_input.as_ref(),
        &["task_id", "taskId", "id", "shell_id", "shellId"],
    ) {
        push_unique_line(
            &mut lines,
            format!("Task: {}", collapse_inline_whitespace(&task_id)),
        );
    }
    if let Some(title) = first_string_field(parsed_input.as_ref(), &["title"]) {
        push_unique_line(
            &mut lines,
            format!("Title: {}", collapse_inline_whitespace(&title)),
        );
    }
    if let Some(status) = first_string_field(parsed_input.as_ref(), &["status"]) {
        push_unique_line(
            &mut lines,
            format!("Status: {}", collapse_inline_whitespace(&status)),
        );
    }
    if lowered == "skill" {
        if let Some(skill_name) = first_string_field(parsed_input.as_ref(), &["name", "skill"]) {
            push_unique_line(
                &mut lines,
                format!("Skill: {}", collapse_inline_whitespace(&skill_name)),
            );
        }
        if let Some(arguments) = first_string_field(parsed_input.as_ref(), &["arguments", "args"]) {
            push_unique_line(
                &mut lines,
                format!("Args: {}", collapse_inline_whitespace(&arguments)),
            );
        }
    }
    if lowered == "lsp"
        && let Some(operation) = first_string_field(parsed_input.as_ref(), &["operation"])
    {
        push_unique_line(
            &mut lines,
            format!("Operation: {}", collapse_inline_whitespace(&operation)),
        );
    }
    if matches!(lowered.as_str(), "lsp" | "toolsearch")
        && let Some(query) = first_string_field(parsed_input.as_ref(), &["query", "symbol"])
    {
        push_unique_line(
            &mut lines,
            format!("Query: {}", collapse_inline_whitespace(&query)),
        );
    }
    if let Some(server_id) = first_string_field(parsed_input.as_ref(), &["server_id", "serverId"]) {
        push_unique_line(
            &mut lines,
            format!("Server: {}", collapse_inline_whitespace(&server_id)),
        );
    }
    if lowered == "callmcptool"
        && let Some(tool_name) = first_string_field(parsed_input.as_ref(), &["tool_name", "tool"])
    {
        push_unique_line(
            &mut lines,
            format!("Tool: {}", collapse_inline_whitespace(&tool_name)),
        );
    }
    if let Some(uri) = first_string_field(parsed_input.as_ref(), &["uri"]) {
        push_unique_line(
            &mut lines,
            truncate_chars(&collapse_inline_whitespace(&uri), 120),
        );
    }

    if let Some(description) = first_string_field(parsed_input.as_ref(), &["description"]) {
        push_unique_line(
            &mut lines,
            format!("Description: {}", collapse_inline_whitespace(&description)),
        );
    }
    if let Some(file_path) =
        first_string_field(parsed_input.as_ref(), &["file_path", "filePath", "path"])
    {
        push_unique_line(&mut lines, display_tool_path(&file_path, cwd));
    }
    if let Some(pattern) = first_string_field(parsed_input.as_ref(), &["pattern", "query", "glob"])
    {
        let pattern_line = if lowered == "grep" {
            grep_regex_display_line(&pattern)
        } else {
            format!("\"{}\"", collapse_inline_whitespace(&pattern))
        };
        push_unique_line(&mut lines, pattern_line);
    }
    if let Some(command) = first_string_field(parsed_input.as_ref(), &["command", "cmd", "script"])
    {
        push_unique_line(
            &mut lines,
            truncate_chars(&format!("$ {}", collapse_inline_whitespace(&command)), 120),
        );
    }

    lines
}

pub(crate) fn tool_activity_prompt(name: &str, input: &str) -> Option<String> {
    if !name.eq_ignore_ascii_case("agent") {
        return None;
    }
    let parsed_input = parse_tool_input(input);
    first_string_field(parsed_input.as_ref(), &["prompt"])
}

pub(crate) fn format_tool_card_status_line(
    content: &str,
    is_error: bool,
    metadata: Option<&str>,
) -> String {
    if is_error {
        return format_tool_result_summary(content, true);
    }

    if let Some(summary) = metadata.and_then(tool_result_metadata_summary) {
        return summary;
    }

    if let Some(ref parsed) = metadata.and_then(parse_tool_result_metadata)
        && let Some(summary) = tool_completion_summary(parsed)
    {
        return summary;
    }

    if content.trim().is_empty() {
        "Done".to_string()
    } else {
        format_tool_result_summary(content, false)
    }
}

pub(crate) fn tool_activity_result_details(
    tool_name: &str,
    metadata: Option<&str>,
    content: &str,
) -> Option<Vec<String>> {
    let mut lines = Vec::new();

    if let Some(metadata) = metadata.and_then(parse_tool_result_metadata) {
        if let Some(changed_paths) = metadata.changed_paths {
            for path in changed_paths.iter().take(3) {
                push_unique_line(&mut lines, path.to_string());
            }
        }
        if tool_name.eq_ignore_ascii_case("agent")
            && let Some(blocks) = metadata.content
        {
            for block in blocks.iter().take(3) {
                if let Some(text) = block.text.as_deref().map(collapse_inline_whitespace) {
                    push_unique_line(&mut lines, truncate_chars(&text, 120));
                }
            }
        }
    }

    if is_task_panel_tool(tool_name) {
        return if lines.is_empty() { None } else { Some(lines) };
    }

    if lines.is_empty() && !content.trim().is_empty() {
        for line in content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(3)
        {
            push_unique_line(
                &mut lines,
                truncate_chars(&collapse_inline_whitespace(line), 120),
            );
        }
    }

    if lines.is_empty() { None } else { Some(lines) }
}

fn is_task_panel_tool(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "taskcreate"
            | "task-create"
            | "taskupdate"
            | "task-update"
            | "tasklist"
            | "task-list"
            | "taskget"
            | "task-get"
    )
}

pub(crate) fn tool_activity_response_text(
    tool_name: &str,
    metadata: Option<&str>,
    content: &str,
) -> Option<String> {
    if tool_name.eq_ignore_ascii_case("agent") {
        if let Some(metadata) = metadata.and_then(parse_tool_result_metadata)
            && let Some(blocks) = metadata.content
        {
            let text_blocks = blocks
                .iter()
                .filter_map(|block| block.text.as_deref())
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>();
            if !text_blocks.is_empty() {
                return Some(text_blocks.join("\n\n"));
            }
        }

        let trimmed = content.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }

    if is_task_panel_tool(tool_name) {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }

    None
}

pub(crate) fn tool_activity_progress_messages(metadata: Option<&str>) -> Vec<Value> {
    metadata
        .and_then(parse_tool_result_metadata)
        .and_then(|m| m.progress_messages)
        .map(|progress_messages| {
            progress_messages
                .into_iter()
                .filter(should_render_tool_progress_message)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn merge_tool_result_progress_metadata(
    metadata: Option<String>,
    progress_messages: &[Value],
) -> Option<String> {
    if progress_messages.is_empty() {
        return metadata;
    }

    let mut metadata_value: Value = metadata
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .filter(|value: &Value| value.is_object())
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    let mut merged_progress = tool_activity_progress_messages(metadata.as_deref());
    for progress in progress_messages {
        if !merged_progress.iter().any(|existing| existing == progress) {
            merged_progress.push(progress.clone());
        }
    }

    if let Some(object) = metadata_value.as_object_mut() {
        object.insert(
            "progressMessages".to_string(),
            Value::Array(merged_progress),
        );
    }

    Some(metadata_value.to_string())
}

fn parse_tool_result_metadata(metadata: &str) -> Option<ToolResultMetadata> {
    serde_json::from_str(metadata).ok()
}

fn tool_result_metadata_summary(metadata: &str) -> Option<String> {
    parse_tool_result_metadata(metadata)
        .and_then(|m| m.summary)
        .map(|summary| collapse_inline_whitespace(&summary))
}

fn tool_completion_summary(metadata: &ToolResultMetadata) -> Option<String> {
    let total_tool_uses = metadata.total_tool_use_count?;
    let total_tokens = metadata
        .total_tokens
        .or_else(|| metadata.usage.as_ref().and_then(|u| u.output_tokens))
        .unwrap_or(0);
    let total_duration_ms = metadata.total_duration_ms?;

    Some(format!(
        "Done ({} {} · {} tokens · {})",
        total_tool_uses,
        if total_tool_uses == 1 {
            "tool use"
        } else {
            "tool uses"
        },
        total_tokens,
        format_duration_short(total_duration_ms),
    ))
}

pub(crate) fn is_bash_like_tool(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "bash" | "shell")
}

pub(crate) fn is_file_read_like_tool(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "read" | "file-read")
}

pub(crate) fn is_file_edit_tool(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "edit" | "file-edit" | "update"
    )
}

pub(crate) fn is_file_write_tool(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "write" | "file-write")
}

pub(crate) fn summarize_file_read_result(content: &str, metadata: Option<&str>) -> String {
    const TRUNCATION_NOTE_PREFIX: &str = "File output truncated for transcript safety.";

    let content = file_read_text_from_metadata(metadata).unwrap_or_else(|| content.to_string());
    let line_count = content
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.starts_with(TRUNCATION_NOTE_PREFIX))
        .count();

    format!(
        "Read {line_count} {}",
        if line_count == 1 { "line" } else { "lines" }
    )
}

fn file_read_text_from_metadata(metadata: Option<&str>) -> Option<String> {
    metadata
        .and_then(parse_tool_result_metadata)
        .and_then(|m| m.content)
        .and_then(|blocks| blocks.into_iter().find_map(|block| block.text))
}

fn split_trailing_cwd_reset_warning(content: &str) -> (Vec<String>, Option<String>) {
    let mut lines = content
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    let cwd_reset_warning = lines
        .last()
        .filter(|line| line.starts_with(SHELL_CWD_RESET_PREFIX))
        .cloned();
    if cwd_reset_warning.is_some() {
        lines.pop();
    }

    (lines, cwd_reset_warning)
}

pub(crate) const BASH_EXPANDED_OUTPUT_DETAIL_LIMIT: usize = 80;

pub(crate) fn bash_result_preview(
    content: &str,
    preview_line_limit: usize,
) -> Option<(String, Vec<String>, Vec<String>)> {
    let (output_lines, cwd_reset_warning) = split_trailing_cwd_reset_warning(content);
    if output_lines.is_empty() {
        return cwd_reset_warning.map(|warning| (warning, Vec::new(), Vec::new()));
    }

    let status_line = output_lines[0].clone();
    let mut remaining_lines = output_lines.iter().skip(1).cloned().collect::<Vec<_>>();
    let regular_preview_lines = remaining_lines
        .iter()
        .filter(|line| !is_bash_tail_diagnostic_line(line))
        .take(preview_line_limit)
        .cloned()
        .collect::<Vec<_>>();
    let visible_regular_count = regular_preview_lines.len();
    let hidden_count = remaining_lines
        .iter()
        .filter(|line| !is_bash_tail_diagnostic_line(line))
        .count()
        .saturating_sub(visible_regular_count);
    let tail_diagnostic_lines = remaining_lines
        .iter()
        .filter(|line| is_bash_tail_diagnostic_line(line))
        .cloned()
        .collect::<Vec<_>>();
    let mut preview_lines = regular_preview_lines;
    if hidden_count > 0 {
        preview_lines.push(format!("… +{hidden_count} lines (ctrl+o to expand)"));
    }
    for line in tail_diagnostic_lines {
        push_unique_line(&mut preview_lines, line);
    }
    if let Some(warning) = cwd_reset_warning {
        preview_lines.push(warning.clone());
        remaining_lines.push(warning);
    }

    if status_line.is_empty() {
        return None;
    }

    Some((
        status_line,
        bounded_bash_detail_lines(&remaining_lines),
        preview_lines,
    ))
}

fn bounded_bash_detail_lines(lines: &[String]) -> Vec<String> {
    if lines.len() <= BASH_EXPANDED_OUTPUT_DETAIL_LIMIT
        && !lines
            .iter()
            .any(|line| is_bash_transcript_truncation_note(line))
    {
        return lines.to_vec();
    }

    let regular_lines = lines
        .iter()
        .filter(|line| !is_bash_tail_diagnostic_line(line))
        .cloned()
        .collect::<Vec<_>>();
    if regular_lines.len() <= BASH_EXPANDED_OUTPUT_DETAIL_LIMIT {
        return lines.to_vec();
    }

    let mut bounded = regular_lines
        .iter()
        .take(BASH_EXPANDED_OUTPUT_DETAIL_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    bounded.push(format!(
        "… +{} lines omitted from expanded Bash preview",
        regular_lines
            .len()
            .saturating_sub(BASH_EXPANDED_OUTPUT_DETAIL_LIMIT)
    ));
    for line in lines
        .iter()
        .filter(|line| is_bash_tail_diagnostic_line(line))
    {
        push_unique_line(&mut bounded, line.clone());
    }
    bounded
}

fn is_bash_tail_diagnostic_line(line: &str) -> bool {
    is_bash_transcript_truncation_note(line) || line.starts_with(SHELL_CWD_RESET_PREFIX)
}

fn is_bash_transcript_truncation_note(line: &str) -> bool {
    line.trim_start_matches('[')
        .starts_with("Bash output truncated for transcript safety.")
}

pub(crate) fn edit_change_summary(input: &str, metadata: Option<&str>) -> String {
    if let Some((added, removed)) = edit_change_counts_from_metadata(metadata) {
        return format_edit_change_summary(added, removed);
    }

    let parsed = parse_tool_input(input);
    let old = first_string_field(parsed.as_ref(), &["old_string", "find"]).unwrap_or_default();
    let new = first_string_field(parsed.as_ref(), &["new_string", "replace"]).unwrap_or_default();
    let removed = if old.is_empty() {
        0
    } else {
        old.lines().count().max(1)
    };
    let added = if new.is_empty() {
        0
    } else {
        new.lines().count().max(1)
    };
    format_edit_change_summary(added as u64, removed as u64)
}

fn edit_change_counts_from_metadata(metadata: Option<&str>) -> Option<(u64, u64)> {
    let parsed = metadata.and_then(parse_tool_result_metadata)?;
    Some((parsed.lines_added?, parsed.lines_removed?))
}

fn format_edit_change_summary(added: u64, removed: u64) -> String {
    let mut parts = Vec::new();
    if added > 0 {
        parts.push(format!(
            "Added {} {}",
            added,
            if added == 1 { "line" } else { "lines" }
        ));
    }
    if removed > 0 {
        parts.push(format!(
            "removed {} {}",
            removed,
            if removed == 1 { "line" } else { "lines" }
        ));
    }
    if parts.is_empty() {
        "No changes".to_string()
    } else {
        parts.join(", ")
    }
}

pub(crate) fn edit_diff_preview_lines(
    input: &str,
    metadata: Option<&str>,
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }

    if let Some(lines) =
        metadata.and_then(|metadata| edit_diff_preview_lines_from_metadata(metadata, limit))
        && !lines.is_empty()
    {
        return lines;
    }

    let parsed = parse_tool_input(input);
    let old = first_string_field(parsed.as_ref(), &["old_string", "find"]).unwrap_or_default();
    let new = first_string_field(parsed.as_ref(), &["new_string", "replace"]).unwrap_or_default();
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let max_lineno = old_lines.len().max(new_lines.len());
    let width = if max_lineno == 0 {
        1
    } else {
        max_lineno.ilog10() as usize + 1
    };
    let mut lines = Vec::new();
    for (i, line) in old_lines.iter().enumerate() {
        lines.push(format!(
            "{:>width$} -{}",
            i + 1,
            collapse_inline_whitespace(line)
        ));
    }
    for (i, line) in new_lines.iter().enumerate() {
        lines.push(format!(
            "{:>width$} +{}",
            i + 1,
            collapse_inline_whitespace(line)
        ));
    }
    limit_diff_preview_lines(lines, limit)
}

fn edit_diff_preview_lines_from_metadata(metadata: &str, limit: usize) -> Option<Vec<String>> {
    let parsed = parse_tool_result_metadata(metadata)?;
    let diff = parsed.diff.as_deref()?;
    let mut lines = render_unified_diff_preview_lines(diff)?;
    if parsed.diff_truncated.unwrap_or(false) {
        lines.push("… diff truncated".to_string());
    }
    Some(limit_diff_preview_lines(lines, limit))
}

enum DiffPreviewLine {
    Diff {
        line_number: usize,
        marker: char,
        content: String,
    },
    Note(String),
}

fn render_unified_diff_preview_lines(diff: &str) -> Option<Vec<String>> {
    let mut parsed_lines = Vec::new();
    let mut old_line = None;
    let mut new_line = None;

    for raw_line in diff.lines() {
        if let Some((old_start, new_start)) = parse_diff_hunk_header(raw_line) {
            old_line = Some(old_start);
            new_line = Some(new_start);
            continue;
        }

        let Some(marker) = raw_line.chars().next() else {
            continue;
        };
        let content = raw_line.get(marker.len_utf8()..).unwrap_or_default();
        match marker {
            ' ' => {
                let line_number = new_line?;
                parsed_lines.push(DiffPreviewLine::Diff {
                    line_number,
                    marker: ' ',
                    content: collapse_inline_whitespace(content),
                });
                old_line = old_line.map(|line| line + 1);
                new_line = new_line.map(|line| line + 1);
            }
            '-' => {
                let line_number = old_line?;
                parsed_lines.push(DiffPreviewLine::Diff {
                    line_number,
                    marker: '-',
                    content: collapse_inline_whitespace(content),
                });
                old_line = old_line.map(|line| line + 1);
            }
            '+' => {
                let line_number = new_line?;
                parsed_lines.push(DiffPreviewLine::Diff {
                    line_number,
                    marker: '+',
                    content: collapse_inline_whitespace(content),
                });
                new_line = new_line.map(|line| line + 1);
            }
            _ if raw_line.starts_with("...") || raw_line.starts_with('…') => {
                parsed_lines.push(DiffPreviewLine::Note(raw_line.to_string()));
            }
            _ => {}
        }
    }

    if parsed_lines.is_empty() {
        return None;
    }

    let width = parsed_lines
        .iter()
        .filter_map(|line| match line {
            DiffPreviewLine::Diff { line_number, .. } => Some(*line_number),
            DiffPreviewLine::Note(_) => None,
        })
        .max()
        .map_or(1, line_number_width);

    Some(
        parsed_lines
            .into_iter()
            .map(|line| match line {
                DiffPreviewLine::Diff {
                    line_number,
                    marker,
                    content,
                } => format!("{line_number:>width$} {marker}{content}"),
                DiffPreviewLine::Note(note) => note,
            })
            .collect(),
    )
}

fn parse_diff_hunk_header(line: &str) -> Option<(usize, usize)> {
    let body = line.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = body.split_whitespace();
    let old_range = parts.next()?.strip_prefix('-')?;
    let new_range = parts.next()?.strip_prefix('+')?;
    Some((
        parse_diff_range_start(old_range)?,
        parse_diff_range_start(new_range)?,
    ))
}

fn parse_diff_range_start(range: &str) -> Option<usize> {
    range.split(',').next()?.parse().ok()
}

fn line_number_width(value: usize) -> usize {
    if value == 0 {
        1
    } else {
        value.ilog10() as usize + 1
    }
}

fn limit_diff_preview_lines(mut lines: Vec<String>, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    if lines.len() > limit {
        // The summary itself occupies the final retained slot, so that replaced
        // real line is omitted too.
        let omitted = lines.len().saturating_sub(limit.saturating_sub(1));
        lines.truncate(limit);
        if let Some(last) = lines.last_mut() {
            *last = format!("… {omitted} more diff lines");
        }
    }
    lines
}

pub(crate) fn summarize_write_result(input: &str) -> String {
    let parsed = parse_tool_input(input);
    let content = first_string_field(parsed.as_ref(), &["content"]).unwrap_or_default();
    let line_count = if content.is_empty() {
        0
    } else {
        content.lines().count().max(1)
    };
    format!(
        "Wrote {line_count} {}",
        if line_count == 1 { "line" } else { "lines" }
    )
}

pub(crate) fn summarize_grep_result(content: &str) -> String {
    let match_count = grep_content_lines(content).count();
    match match_count {
        0 => "No matches".to_string(),
        1 => "1 match".to_string(),
        n => format!("{n} matches"),
    }
}

pub(crate) fn grep_match_preview_lines(content: &str, limit: usize) -> Vec<String> {
    let lines: Vec<&str> = grep_content_lines(content).collect();
    let total = lines.len();
    let mut preview: Vec<String> = lines
        .iter()
        .take(limit)
        .map(|line| truncate_chars(&collapse_inline_whitespace(line), 120))
        .collect();
    if total > limit {
        preview.push(format!("… +{} matches", total - limit));
    }
    preview
}

fn grep_content_lines(content: &str) -> impl Iterator<Item = &str> {
    content.lines().filter(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !is_grep_summary_line(trimmed)
    })
}

fn is_grep_summary_line(line: &str) -> bool {
    (line.starts_with("Found ") && (line.contains(" file") || line.contains(" total ")))
        || line == "No files found"
        || line == "No matches found"
}

pub(crate) fn summarize_glob_result(content: &str) -> String {
    let file_count = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    match file_count {
        0 => "No files matched".to_string(),
        1 => "1 file matched".to_string(),
        n => format!("{n} files matched"),
    }
}

pub(crate) fn glob_match_preview_lines(content: &str, limit: usize, cwd: &Path) -> Vec<String> {
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let total = lines.len();
    let mut preview: Vec<String> = lines
        .iter()
        .take(limit)
        .map(|line| display_tool_path(line.trim(), cwd))
        .collect();
    if total > limit {
        preview.push(format!("… +{} files", total - limit));
    }
    preview
}

pub(crate) fn format_tool_use_summary(name: &str, input: &str, cwd: &Path) -> String {
    let parsed_input = parse_tool_input(input);
    let file_path = first_string_field(parsed_input.as_ref(), &["file_path", "filePath", "path"]);
    let pattern = first_string_field(parsed_input.as_ref(), &["pattern", "query", "glob"]);
    let command = first_string_field(parsed_input.as_ref(), &["command", "cmd", "script"])
        .map(|command| collapse_inline_whitespace(&command));

    if let Some(path) = file_path {
        return format!("{name}({})", display_tool_path(&path, cwd));
    }
    if let Some(pattern) = pattern {
        return format!("{name}(\"{}\")", collapse_inline_whitespace(&pattern));
    }
    if let Some(command) = command {
        return format!("{name}({})", truncate_chars(&format!("$ {command}"), 72));
    }
    name.to_string()
}

pub(crate) fn format_tool_result_summary(content: &str, is_error: bool) -> String {
    let cleaned = content
        .replace("<tool_use_error>", "")
        .replace("</tool_use_error>", "")
        .replace("<system-reminder>", "")
        .replace("</system-reminder>", "");
    let normalized_lower = cleaned.to_ascii_lowercase();
    if is_error
        && (normalized_lower.contains("inputvalidationerror:")
            || normalized_lower.contains("invalid tool input:")
            || normalized_lower.contains("invalid tool parameters"))
    {
        return "Invalid tool parameters".to_string();
    }
    let first_line = cleaned
        .lines()
        .find(|line| !line.trim().is_empty())
        .map_or_else(
            || {
                if is_error {
                    "Tool failed".to_string()
                } else {
                    "Done".to_string()
                }
            },
            collapse_inline_whitespace,
        );

    if is_error {
        truncate_chars(&first_line, 96)
    } else if cleaned.lines().count() > 1 {
        String::new()
    } else {
        truncate_chars(&first_line, 96)
    }
}

#[cfg(test)]
mod diff_preview_limit_tests {
    use super::limit_diff_preview_lines;

    #[test]
    fn diff_preview_summary_counts_the_replaced_retained_line_as_omitted() {
        let lines = (1..=5).map(|line| format!("line {line}")).collect();
        assert_eq!(
            limit_diff_preview_lines(lines, 3),
            vec![
                "line 1".to_string(),
                "line 2".to_string(),
                "… 3 more diff lines".to_string(),
            ]
        );
    }

    #[test]
    fn diff_preview_limit_boundaries_are_exact() {
        assert!(limit_diff_preview_lines(vec!["one".to_string()], 0).is_empty());
        assert_eq!(
            limit_diff_preview_lines(vec!["one".to_string()], 1),
            vec!["one".to_string()]
        );
        assert_eq!(
            limit_diff_preview_lines(vec!["one".to_string(), "two".to_string()], 1),
            vec!["… 2 more diff lines".to_string()]
        );
    }
}
