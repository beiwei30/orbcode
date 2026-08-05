use std::collections::HashSet;
use std::path::Path;

use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

#[cfg(test)]
use crate::history_cell::cells::collect_tool_results_by_id;
use crate::history_cell::cells::{
    build_collapsible_tool_cells_from_message_with_results,
    filtered_tool_result_blocks_for_rendering, is_plain_assistant_text_message,
    is_tool_result_only_message, is_tool_use_only_assistant_message,
    normalize_transcript_messages_for_rendering,
};
use crate::history_cell::hook_note::parse_hook_transcript_note;
use crate::render::message::render_message_lines;
use crate::render::permission_labels::grep_regex_display_line;
use crate::render::styled_wrap::tool_body_prefix;
use crate::render::text_utils::{
    StyledLine, collapse_inline_whitespace, compact_blank_lines, push_unique_line,
};
use crate::tool_cell::ToolResultIndex;
use crate::tool_cell::render::{black_circle_glyph, render_tool_cell_lines};
use crate::tool_cell::summary::format_tool_result_summary;
use crate::tool_cell::utils::{
    display_tool_path, extract_read_path_from_command, extract_search_pattern_from_command,
    first_string_field, is_list_command, is_read_command, is_search_command, parse_tool_input,
};
use crate::tui_theme::{active_palette, inactive_style, subtle_style};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CollapsibleActivityKind {
    Search,
    Read,
    List,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct CollapsibleToolUse {
    pub(crate) id: String,
    pub(crate) kind: CollapsibleActivityKind,
    pub(crate) hint: Option<String>,
    pub(crate) detail_line: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct CollapsedActivityGroup {
    pub(crate) search_count: usize,
    pub(crate) read_paths: Vec<String>,
    pub(crate) read_operation_count: usize,
    pub(crate) read_tool_use_ids: HashSet<String>,
    pub(crate) failed_read_tool_use_ids: HashSet<String>,
    pub(crate) list_count: usize,
    pub(crate) latest_hint: Option<String>,
    pub(crate) detail_lines: Vec<String>,
    pub(crate) error_messages: Vec<String>,
    pub(crate) messages: Vec<TranscriptMessage>,
    pub(crate) tool_use_ids: HashSet<String>,
    pub(crate) matched_tool_use_ids: HashSet<String>,
    pub(crate) tool_results: ToolResultIndex,
}

#[allow(dead_code)]
impl CollapsedActivityGroup {
    pub(crate) fn read_count(&self) -> usize {
        // New groups count tool-use operations so duplicate paths and failures
        // share one identity domain. Keep the path count as a compatibility
        // fallback for older/materialized groups that predate that counter.
        self.read_operation_count.max(self.read_paths.len())
    }

    pub(crate) fn failed_read_count(&self) -> usize {
        self.failed_read_tool_use_ids.len().min(self.read_count())
    }

    pub(crate) fn has_content(&self) -> bool {
        self.search_count > 0 || self.read_count() > 0 || self.list_count > 0
    }

    pub(crate) fn has_unresolved_tool_uses(&self) -> bool {
        self.tool_use_ids
            .iter()
            .any(|tool_use_id| !self.matched_tool_use_ids.contains(tool_use_id))
    }
}

#[cfg(test)]
pub(crate) fn build_collapsed_activity_group(
    messages: &[TranscriptMessage],
    start_index: usize,
    cwd: &Path,
) -> Option<(CollapsedActivityGroup, usize)> {
    let tool_results = collect_tool_results_by_id(messages);
    build_collapsed_activity_group_with_results(messages, start_index, cwd, &tool_results)
}

pub(crate) fn build_collapsed_activity_group_with_results(
    messages: &[TranscriptMessage],
    start_index: usize,
    cwd: &Path,
    tool_results: &ToolResultIndex,
) -> Option<(CollapsedActivityGroup, usize)> {
    let mut group = CollapsedActivityGroup::default();
    let mut index = start_index;
    let mut saw_tool_use = false;

    while index < messages.len() {
        let message = &messages[index];

        if is_hidden_thinking_message(message) {
            index += 1;
            continue;
        }

        if let Some(tool_uses) = collapsible_tool_uses(message, cwd) {
            saw_tool_use = true;
            group.messages.push(message.clone());
            for tool_use in tool_uses {
                group.tool_use_ids.insert(tool_use.id.clone());
                if let Some(hint) = tool_use.hint.clone() {
                    group.latest_hint = Some(hint);
                }
                if let Some(detail_line) = tool_use.detail_line {
                    push_unique_line(&mut group.detail_lines, detail_line);
                }
                match tool_use.kind {
                    CollapsibleActivityKind::Search => group.search_count += 1,
                    CollapsibleActivityKind::Read => {
                        group.read_operation_count += 1;
                        group.read_tool_use_ids.insert(tool_use.id.clone());
                        if let Some(path) = tool_use.hint {
                            push_unique_line(&mut group.read_paths, path);
                        }
                    }
                    CollapsibleActivityKind::List => group.list_count += 1,
                }
            }
            index += 1;
            continue;
        }

        if saw_tool_use {
            if is_tool_use_only_assistant_message(message)
                || is_plain_assistant_text_message(message)
            {
                break;
            }
            if is_tool_result_only_message(message) {
                index += 1;
                continue;
            }
            if parse_hook_transcript_note(message).is_some() {
                index += 1;
                continue;
            }
        }

        break;
    }

    if saw_tool_use && group.has_content() {
        apply_tool_results_to_activity_group(&mut group, tool_results);
        Some((group, index))
    } else {
        None
    }
}

fn is_hidden_thinking_message(message: &TranscriptMessage) -> bool {
    matches!(message.role, MessageRole::Assistant)
        && !message.blocks.is_empty()
        && message
            .blocks
            .iter()
            .all(|block| matches!(block, TranscriptBlock::Thinking { .. }))
}

#[allow(dead_code)]
fn collapsible_tool_uses(
    message: &TranscriptMessage,
    cwd: &Path,
) -> Option<Vec<CollapsibleToolUse>> {
    if !matches!(message.role, MessageRole::Assistant) || message.blocks.is_empty() {
        return None;
    }

    let mut tool_uses = Vec::new();
    for block in &message.blocks {
        match block {
            TranscriptBlock::Thinking { .. } => {}
            TranscriptBlock::ToolUse { id, name, input } => {
                let summary = parse_collapsible_tool_use(id, name, input, cwd)?;
                tool_uses.push(summary);
            }
            _ => return None,
        }
    }

    if tool_uses.is_empty() {
        None
    } else {
        Some(tool_uses)
    }
}

pub(crate) fn parse_collapsible_tool_use(
    id: &str,
    name: &str,
    input: &str,
    cwd: &Path,
) -> Option<CollapsibleToolUse> {
    let lowered = name.to_ascii_lowercase();
    let parsed_input = parse_tool_input(input);
    let file_path = first_string_field(parsed_input.as_ref(), &["file_path", "filePath", "path"]);
    let pattern = first_string_field(parsed_input.as_ref(), &["pattern", "query", "glob"]);
    let command = first_string_field(parsed_input.as_ref(), &["command", "cmd", "script"]);

    let (kind, hint, detail_line) = if matches!(lowered.as_str(), "read" | "file-read") {
        let path = file_path
            .as_deref()
            .map(|path| display_tool_path(path, cwd));
        (
            CollapsibleActivityKind::Read,
            path.clone(),
            path.or_else(|| command.clone()),
        )
    } else if matches!(
        lowered.as_str(),
        "glob" | "grep" | "websearch" | "web-search"
    ) {
        if lowered == "grep" {
            let search_hint = pattern.as_deref().map(grep_regex_display_line);
            let search_scope = file_path
                .as_deref()
                .map(|path| display_tool_path(path, cwd));
            let detail_line = match (pattern.as_deref(), search_scope.as_deref()) {
                (Some(pattern), Some(scope)) => {
                    Some(format!("{} in {}", grep_regex_display_line(pattern), scope))
                }
                (Some(pattern), None) => Some(grep_regex_display_line(pattern)),
                (None, Some(scope)) => Some(format!("Search in {scope}")),
                (None, None) => None,
            };
            (
                CollapsibleActivityKind::Search,
                search_hint.or_else(|| search_scope.clone()),
                detail_line,
            )
        } else {
            let search_hint = pattern
                .clone()
                .map(|value| format!("\"{}\"", collapse_inline_whitespace(&value)));
            (
                CollapsibleActivityKind::Search,
                search_hint.clone().or_else(|| file_path.clone()),
                search_hint.or_else(|| file_path.map(|path| display_tool_path(&path, cwd))),
            )
        }
    } else if matches!(lowered.as_str(), "bash" | "shell") {
        let command = command?;
        let normalized = collapse_inline_whitespace(&command);
        if is_search_command(&normalized) {
            let hint = pattern
                .map(|value| format!("\"{}\"", collapse_inline_whitespace(&value)))
                .or_else(|| extract_search_pattern_from_command(&normalized))
                .or_else(|| Some(format!("$ {normalized}")));
            (
                CollapsibleActivityKind::Search,
                hint.clone(),
                Some(format!("$ {normalized}")),
            )
        } else if is_list_command(&normalized) {
            (
                CollapsibleActivityKind::List,
                Some(format!("$ {normalized}")),
                Some(format!("$ {normalized}")),
            )
        } else if is_read_command(&normalized) {
            let hint = file_path
                .map(|path| display_tool_path(&path, cwd))
                .or_else(|| extract_read_path_from_command(&normalized, cwd))
                .or_else(|| Some(format!("$ {normalized}")));
            (
                CollapsibleActivityKind::Read,
                hint.clone(),
                Some(format!("$ {normalized}")),
            )
        } else {
            return None;
        }
    } else if lowered == "ls" {
        let hint = file_path
            .map(|path| display_tool_path(&path, cwd))
            .or_else(|| command.clone());
        (CollapsibleActivityKind::List, hint.clone(), hint)
    } else {
        return None;
    };

    Some(CollapsibleToolUse {
        id: id.to_string(),
        kind,
        hint,
        detail_line,
    })
}

fn apply_tool_results_to_activity_group(
    group: &mut CollapsedActivityGroup,
    tool_results: &ToolResultIndex,
) {
    for tool_use_id in &group.tool_use_ids {
        let Some(result) = tool_results.get(tool_use_id) else {
            continue;
        };
        group.matched_tool_use_ids.insert(tool_use_id.clone());
        group
            .tool_results
            .insert(tool_use_id.clone(), result.clone());
        if result.is_error {
            push_unique_line(
                &mut group.error_messages,
                format_tool_result_summary(&result.content, true),
            );
            if group.read_tool_use_ids.contains(tool_use_id) {
                group.failed_read_tool_use_ids.insert(tool_use_id.clone());
            }
        }
    }
}

fn activity_summary_verb(
    is_active: bool,
    parts_is_empty: bool,
    active_first: &'static str,
    active_next: &'static str,
    completed_first: &'static str,
    completed_next: &'static str,
) -> &'static str {
    match (is_active, parts_is_empty) {
        (true, true) => active_first,
        (true, false) => active_next,
        (false, true) => completed_first,
        (false, false) => completed_next,
    }
}

pub(crate) fn collapsed_activity_summary_text(
    group: &CollapsedActivityGroup,
    is_active: bool,
) -> String {
    let mut parts = Vec::new();
    if group.search_count > 0 {
        let verb = activity_summary_verb(
            is_active,
            parts.is_empty(),
            "Searching for",
            "searching for",
            "Searched for",
            "searched for",
        );
        parts.push(format!(
            "{verb} {} {}",
            group.search_count,
            if group.search_count == 1 {
                "pattern"
            } else {
                "patterns"
            }
        ));
    }
    let failed_read_count = group.failed_read_count();
    let displayed_read_count = if is_active {
        group.read_count()
    } else {
        group.read_count().saturating_sub(failed_read_count)
    };
    if displayed_read_count > 0 {
        let verb = activity_summary_verb(
            is_active,
            parts.is_empty(),
            "Reading",
            "reading",
            "Read",
            "read",
        );
        parts.push(format!(
            "{verb} {} {}",
            displayed_read_count,
            if displayed_read_count == 1 {
                "file"
            } else {
                "files"
            }
        ));
    }
    if failed_read_count > 0 {
        if parts.is_empty() {
            parts.push(format!(
                "Failed to read {} {}",
                failed_read_count,
                if failed_read_count == 1 {
                    "file"
                } else {
                    "files"
                }
            ));
        } else {
            parts.push(format!(
                "{} {} failed",
                failed_read_count,
                if failed_read_count == 1 {
                    "file"
                } else {
                    "files"
                }
            ));
        }
    }
    if group.list_count > 0 {
        let verb = activity_summary_verb(
            is_active,
            parts.is_empty(),
            "Listing",
            "listing",
            "Listed",
            "listed",
        );
        parts.push(format!(
            "{verb} {} {}",
            group.list_count,
            if group.list_count == 1 {
                "directory"
            } else {
                "directories"
            }
        ));
    }
    let summary = parts.join(", ");
    if is_active {
        format!("{summary}...")
    } else {
        summary
    }
}

pub(crate) fn render_collapsed_activity_group_lines(
    group: &CollapsedActivityGroup,
    expanded: bool,
    is_active: bool,
    blink_visible: bool,
) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    let has_error = !group.error_messages.is_empty();
    let summary_style = inactive_style();
    let base_summary = collapsed_activity_summary_text(group, is_active);
    let summary = if expanded {
        if is_active {
            format!("{base_summary}(ctrl+o to collapse)")
        } else {
            format!("{base_summary} (ctrl+o to collapse)")
        }
    } else if is_active {
        format!("{base_summary}(ctrl+o to expand)")
    } else {
        format!("{base_summary} (ctrl+o to expand)")
    };

    let prefix = if is_active {
        Span::styled(
            if blink_visible {
                black_circle_glyph().to_string()
            } else {
                " ".to_string()
            },
            if has_error {
                Style::default().fg(active_palette().error)
            } else {
                inactive_style()
            },
        )
    } else {
        Span::raw(" ")
    };

    lines.push(Line::from(vec![
        prefix,
        Span::raw(" "),
        Span::styled(summary, summary_style),
    ]));

    let mut child_lines = Vec::new();
    if !expanded
        && is_active
        && let Some(hint) = &group.latest_hint
    {
        child_lines.push((hint.clone(), inactive_style()));
    }
    child_lines.extend(
        group
            .error_messages
            .iter()
            .take(2)
            .cloned()
            .map(|error| (error, inactive_style().add_modifier(Modifier::DIM))),
    );

    let child_line_count = child_lines.len();
    for (index, (line, style)) in child_lines.into_iter().enumerate() {
        let prefix = tool_body_prefix(index, child_line_count);
        lines.push(Line::from(vec![
            Span::styled(prefix, subtle_style()),
            Span::styled(line, style),
        ]));
    }

    compact_blank_lines(lines)
}

pub(crate) fn render_collapsed_activity_group_cell_lines(
    group: &CollapsedActivityGroup,
    expanded: bool,
    is_active: bool,
    blink_visible: bool,
    cwd: &Path,
    transcript_width: usize,
    model_display_name: &str,
    last_thinking_block: Option<&(String, usize)>,
) -> Vec<StyledLine> {
    let mut lines =
        render_collapsed_activity_group_lines(group, expanded, is_active, blink_visible);
    if !expanded {
        return lines;
    }

    let expanded_lines = render_expanded_collapsed_activity_group_messages(
        group,
        is_active,
        cwd,
        transcript_width,
        model_display_name,
        last_thinking_block,
    );
    if !expanded_lines.is_empty() {
        lines.push(Line::default());
        lines.extend(expanded_lines);
    }

    compact_blank_lines(lines)
}

fn render_expanded_collapsed_activity_group_messages(
    group: &CollapsedActivityGroup,
    parent_group_is_active: bool,
    cwd: &Path,
    transcript_width: usize,
    model_display_name: &str,
    last_thinking_block: Option<&(String, usize)>,
) -> Vec<StyledLine> {
    let normalized_messages = normalize_transcript_messages_for_rendering(&group.messages);
    let mut lines = Vec::new();
    let mut handled_tool_use_ids = HashSet::new();
    let mut index = 0;

    while index < normalized_messages.len() {
        if let Some((tool_cells, next_index)) =
            build_collapsible_tool_cells_from_message_with_results(
                &normalized_messages,
                index,
                cwd,
                &group.tool_results,
                !parent_group_is_active && group.has_unresolved_tool_uses(),
            )
        {
            for card in &tool_cells {
                handled_tool_use_ids.insert(card.tool_use_id.clone());
                let indicator_override = (!parent_group_is_active && card.is_active)
                    .then_some((black_circle_glyph().to_string(), inactive_style()));
                let mut rendered =
                    render_tool_cell_lines(card, true, indicator_override, transcript_width, cwd);
                if !parent_group_is_active && card.is_active && !card.is_error {
                    rendered.truncate(1);
                }
                if !rendered.is_empty() && !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.extend(rendered);
            }
            index = next_index;
            continue;
        }

        let filtered_blocks = filtered_tool_result_blocks_for_rendering(
            &normalized_messages[index],
            &handled_tool_use_ids,
        );
        if filtered_blocks.as_ref().is_some_and(Vec::is_empty) {
            index += 1;
            continue;
        }

        let filtered_message = filtered_blocks.map(|blocks| {
            let mut message = normalized_messages[index].clone();
            message.blocks = blocks;
            message
        });
        let message = filtered_message.unwrap_or_else(|| normalized_messages[index].clone());
        let nested_lines = render_message_lines(
            &message,
            cwd,
            true,
            last_thinking_block,
            transcript_width,
            model_display_name,
            false,
        );
        if !nested_lines.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.extend(nested_lines);
        }
        index += 1;
    }

    compact_blank_lines(lines)
}
