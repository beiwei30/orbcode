use std::collections::HashSet;
use std::path::Path;

use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};

use crate::history_cell::agent_activity::{AgentActivityGroup, build_agent_activity_group};
use crate::history_cell::collapsed_activity::{
    CollapsedActivityGroup, build_collapsed_activity_group_with_results, parse_collapsible_tool_use,
};
use crate::render::text_utils::push_unique_line;
use crate::tool_cell::summary::{
    bash_result_preview, default_active_tool_status_line, edit_change_summary,
    edit_diff_preview_lines, format_tool_activity_title, format_tool_card_status_line,
    glob_match_preview_lines, grep_match_preview_lines, is_bash_like_tool, is_file_edit_tool,
    is_file_read_like_tool, is_file_write_tool, summarize_file_read_result, summarize_glob_result,
    summarize_grep_result, summarize_write_result, tool_activity_detail_lines,
    tool_activity_progress_messages, tool_activity_prompt, tool_activity_response_text,
    tool_activity_result_details, tool_activity_title_style,
};
use crate::tool_cell::utils::parse_tool_input;
use crate::tool_cell::{ToolCell, ToolResultIndex, ToolResultRecord, ToolUseSpec};

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum TranscriptCell {
    AgentGroup(AgentActivityGroup),
    ActivityGroup(CollapsedActivityGroup),
    Tool(ToolCell),
    Message(TranscriptMessage),
}

impl TranscriptCell {
    pub(crate) fn has_tool_use_id(&self, tool_use_id: &str) -> bool {
        match self {
            TranscriptCell::Tool(card) => card.tool_use_id == tool_use_id,
            TranscriptCell::AgentGroup(group) => group.has_tool_use_id(tool_use_id),
            // Only claim a tool once its result has arrived (`matched_tool_use_ids`).
            // While a collapsible tool is still in-progress it is deliberately
            // shown as a live activity ("Running bash…"); claiming it here on
            // `tool_use_ids` would suppress that live cell. De-duplicating the
            // brief overlap with the collapsed group's aggregate count would
            // require the group to defer rendering unresolved tools, not hiding
            // the live activity (see code-review 2026-07 notes).
            TranscriptCell::ActivityGroup(group) => {
                group.matched_tool_use_ids.contains(tool_use_id)
            }
            TranscriptCell::Message(_) => false,
        }
    }
}

pub(crate) const ORPHANED_TOOL_RESULT: &str = "Interrupted";

pub(crate) fn transcript_cells_from_messages(
    messages: &[TranscriptMessage],
    cwd: &Path,
) -> Vec<TranscriptCell> {
    let normalized_messages = normalize_transcript_messages_for_rendering(messages);
    let tool_results = collect_tool_results_by_id(&normalized_messages);
    let mut cells = Vec::new();
    let mut handled_tool_use_ids = HashSet::new();
    let mut index = 0;
    while index < normalized_messages.len() {
        if let Some((group, next_index)) = build_collapsed_activity_group_with_results(
            &normalized_messages,
            index,
            cwd,
            &tool_results,
        ) {
            handled_tool_use_ids.extend(group.tool_use_ids.iter().cloned());
            cells.push(TranscriptCell::ActivityGroup(group));
            index = next_index;
            continue;
        }

        if let Some((tool_cells, next_index)) = build_tool_cells_from_message_with_results(
            &normalized_messages,
            index,
            cwd,
            &tool_results,
        ) {
            handled_tool_use_ids.extend(tool_cells.iter().map(|card| card.tool_use_id.clone()));
            if let Some(group) = build_agent_activity_group(&tool_cells) {
                cells.push(TranscriptCell::AgentGroup(group));
            } else {
                cells.extend(tool_cells.into_iter().map(TranscriptCell::Tool));
            }
            index = next_index;
            continue;
        }

        if let Some((card, next_index)) =
            build_tool_cell_with_results(&normalized_messages, index, cwd, &tool_results)
        {
            handled_tool_use_ids.insert(card.tool_use_id.clone());
            cells.push(TranscriptCell::Tool(card));
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
        if matches!(message.role, MessageRole::Assistant) {
            handled_tool_use_ids.extend(message.blocks.iter().filter_map(|block| match block {
                TranscriptBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            }));
        }
        cells.push(TranscriptCell::Message(message));
        index += 1;
    }

    cells
}

pub(crate) fn normalize_transcript_messages_for_rendering(
    messages: &[TranscriptMessage],
) -> Vec<TranscriptMessage> {
    let mut normalized = Vec::new();
    for message in messages {
        if let Some(split_messages) = split_transcript_message_for_rendering(message) {
            normalized.extend(split_messages);
        } else {
            normalized.push(message.clone());
        }
    }
    normalized
}

pub(crate) fn is_tool_result_only_message(message: &TranscriptMessage) -> bool {
    matches!(message.role, MessageRole::User)
        && !message.blocks.is_empty()
        && message.blocks.iter().all(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { .. } | TranscriptBlock::Thinking { .. }
            )
        })
}

pub(crate) fn is_tool_use_only_assistant_message(message: &TranscriptMessage) -> bool {
    matches!(message.role, MessageRole::Assistant)
        && !message.blocks.is_empty()
        && message.blocks.iter().all(|block| {
            matches!(
                block,
                TranscriptBlock::ToolUse { .. } | TranscriptBlock::Thinking { .. }
            )
        })
}

pub(crate) fn is_plain_assistant_text_message(message: &TranscriptMessage) -> bool {
    matches!(message.role, MessageRole::Assistant)
        && !message.blocks.is_empty()
        && message
            .blocks
            .iter()
            .all(|block| matches!(block, TranscriptBlock::Text { .. }))
}

pub(crate) fn is_plain_user_text_message(message: &TranscriptMessage) -> bool {
    !message.blocks.is_empty()
        && message
            .blocks
            .iter()
            .all(|block| matches!(block, TranscriptBlock::Text { .. }))
}

pub(crate) fn transcript_has_unresolved_tool_use_in_messages(
    messages: &[TranscriptMessage],
    tool_use_id: &str,
) -> bool {
    let mut saw_tool_use = false;
    for message in messages {
        for block in &message.blocks {
            match block {
                TranscriptBlock::ToolUse { id, .. } if id == tool_use_id => {
                    saw_tool_use = true;
                }
                TranscriptBlock::ToolResult {
                    tool_use_id: id, ..
                } if id == tool_use_id => {
                    return false;
                }
                _ => {}
            }
        }
    }

    saw_tool_use
}

pub(crate) fn is_pending_tool_tail_neutral_message(message: &TranscriptMessage) -> bool {
    matches!(message.role, MessageRole::System)
        || is_tool_result_only_message(message)
        || (matches!(message.role, MessageRole::User)
            && is_plain_user_text_message(message)
            && message
                .content
                .lines()
                .next()
                .is_some_and(|line| line.trim_end().ends_with(" hook context:")))
}

pub(crate) fn transcript_has_tool_result_in_messages(
    messages: &[TranscriptMessage],
    tool_use_id: &str,
) -> bool {
    messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { tool_use_id: id, .. } if id == tool_use_id
            )
        })
    })
}

pub(crate) fn filtered_tool_result_blocks_for_rendering(
    message: &TranscriptMessage,
    handled_tool_use_ids: &HashSet<String>,
) -> Option<Vec<TranscriptBlock>> {
    if !is_tool_result_only_message(message) {
        return None;
    }

    Some(
        message
            .blocks
            .iter()
            .filter(|block| match block {
                TranscriptBlock::ToolResult { tool_use_id, .. } => {
                    !handled_tool_use_ids.contains(tool_use_id)
                }
                _ => true,
            })
            .cloned()
            .collect(),
    )
}

pub(crate) fn collect_tool_results_by_id(messages: &[TranscriptMessage]) -> ToolResultIndex {
    let mut results = ToolResultIndex::new();
    for message in messages {
        if !matches!(message.role, MessageRole::User) {
            continue;
        }
        for block in &message.blocks {
            if let TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                metadata,
            } = block
            {
                results
                    .entry(tool_use_id.clone())
                    .or_insert_with(|| ToolResultRecord {
                        content: content.to_string(),
                        is_error: *is_error,
                        metadata: metadata.clone(),
                    });
            }
        }
    }
    results
}

fn non_collapsible_tool_use_specs<'a>(
    message: &'a TranscriptMessage,
    cwd: &Path,
) -> Option<Vec<ToolUseSpec<'a>>> {
    let tool_uses = tool_use_specs(message)?;
    if tool_uses.iter().any(|tool_use| {
        parse_collapsible_tool_use(tool_use.id, tool_use.name, tool_use.input, cwd).is_some()
    }) {
        return None;
    }
    Some(tool_uses)
}

fn collapsible_tool_use_specs<'a>(
    message: &'a TranscriptMessage,
    cwd: &Path,
) -> Option<Vec<ToolUseSpec<'a>>> {
    let tool_uses = tool_use_specs(message)?;
    if tool_uses.iter().all(|tool_use| {
        parse_collapsible_tool_use(tool_use.id, tool_use.name, tool_use.input, cwd).is_some()
    }) {
        Some(tool_uses)
    } else {
        None
    }
}

fn tool_use_specs<'a>(message: &'a TranscriptMessage) -> Option<Vec<ToolUseSpec<'a>>> {
    if !matches!(message.role, MessageRole::Assistant) || message.blocks.is_empty() {
        return None;
    }

    let mut tool_uses = Vec::new();
    for block in &message.blocks {
        match block {
            TranscriptBlock::Thinking { .. } => {}
            TranscriptBlock::ToolUse { id, name, input } => {
                tool_uses.push(ToolUseSpec { id, name, input })
            }
            _ => return None,
        }
    }

    (!tool_uses.is_empty()).then_some(tool_uses)
}

fn build_tool_cells_from_message_with_results(
    messages: &[TranscriptMessage],
    start_index: usize,
    cwd: &Path,
    tool_results: &ToolResultIndex,
) -> Option<(Vec<ToolCell>, usize)> {
    let message = messages.get(start_index)?;
    let tool_uses = non_collapsible_tool_use_specs(message, cwd)?;
    if tool_uses.len() <= 1 {
        return None;
    }

    Some((
        tool_uses
            .into_iter()
            .map(|tool_use| build_tool_cell_from_use(tool_use, cwd, tool_results, false))
            .collect(),
        start_index + 1,
    ))
}

#[allow(dead_code)]
#[cfg_attr(not(test), allow(unused))]
pub(crate) fn build_collapsible_tool_cells_from_message(
    messages: &[TranscriptMessage],
    start_index: usize,
    cwd: &Path,
    mark_missing_result_as_interrupted: bool,
) -> Option<(Vec<ToolCell>, usize)> {
    let tool_results = collect_tool_results_by_id(messages);
    build_collapsible_tool_cells_from_message_with_results(
        messages,
        start_index,
        cwd,
        &tool_results,
        mark_missing_result_as_interrupted,
    )
}

pub(crate) fn build_collapsible_tool_cells_from_message_with_results(
    messages: &[TranscriptMessage],
    start_index: usize,
    cwd: &Path,
    tool_results: &ToolResultIndex,
    mark_missing_result_as_interrupted: bool,
) -> Option<(Vec<ToolCell>, usize)> {
    let message = messages.get(start_index)?;
    let tool_uses = collapsible_tool_use_specs(message, cwd)?;

    Some((
        tool_uses
            .into_iter()
            .map(|tool_use| {
                build_tool_cell_from_use(
                    tool_use,
                    cwd,
                    tool_results,
                    mark_missing_result_as_interrupted,
                )
            })
            .collect(),
        start_index + 1,
    ))
}

fn build_tool_cell_from_use(
    tool_use: ToolUseSpec<'_>,
    cwd: &Path,
    tool_results: &ToolResultIndex,
    mark_missing_result_as_interrupted: bool,
) -> ToolCell {
    let mut status_line: String;
    let mut detail_lines = tool_activity_detail_lines(tool_use.name, tool_use.input, cwd);
    let prompt = tool_activity_prompt(tool_use.name, tool_use.input);
    let mut progress_messages = Vec::new();
    let mut collapsed_preview_lines = Vec::new();
    let mut response = None;
    let is_error: bool;
    let is_active: bool;

    if let Some(result) = tool_results.get(tool_use.id) {
        is_error = result.is_error;
        is_active = false;
        status_line = format_tool_card_status_line(
            &result.content,
            result.is_error,
            result.metadata.as_deref(),
        );
        progress_messages = tool_activity_progress_messages(result.metadata.as_deref());
        if let Some(full_response) =
            tool_activity_response_text(tool_use.name, result.metadata.as_deref(), &result.content)
        {
            response = Some(full_response);
        }
        if is_file_read_like_tool(tool_use.name) && !result.is_error {
            status_line = summarize_file_read_result(&result.content, result.metadata.as_deref());
            detail_lines.clear();
            collapsed_preview_lines.clear();
        } else if is_file_edit_tool(tool_use.name) && !result.is_error {
            status_line = edit_change_summary(tool_use.input, result.metadata.as_deref());
            detail_lines = edit_diff_preview_lines(tool_use.input, result.metadata.as_deref(), 10);
            collapsed_preview_lines = detail_lines.clone();
        } else if is_file_write_tool(tool_use.name)
            && !result.is_error
            && has_write_content_field(tool_use.input)
        {
            status_line = summarize_write_result(tool_use.input);
            detail_lines.clear();
            collapsed_preview_lines.clear();
        } else if tool_use.name.eq_ignore_ascii_case("grep") && !result.is_error {
            status_line = summarize_grep_result(&result.content);
            detail_lines = grep_match_preview_lines(&result.content, 5);
            collapsed_preview_lines.clear();
        } else if tool_use.name.eq_ignore_ascii_case("glob") && !result.is_error {
            status_line = summarize_glob_result(&result.content);
            detail_lines = glob_match_preview_lines(&result.content, 5, cwd);
            collapsed_preview_lines.clear();
        } else if is_bash_like_tool(tool_use.name) && !result.is_error {
            if let Some((bash_status_line, bash_detail_lines, bash_preview_lines)) =
                bash_result_preview(&result.content, 2)
            {
                status_line = bash_status_line;
                detail_lines = bash_detail_lines;
                collapsed_preview_lines = bash_preview_lines;
            } else {
                if status_line.starts_with("Executed `") {
                    status_line = "Done".to_string();
                }
                detail_lines.clear();
            }
        } else if let Some(extra) =
            tool_activity_result_details(tool_use.name, result.metadata.as_deref(), &result.content)
        {
            let mut extra_lines = extra.into_iter();
            if status_line.trim().is_empty()
                && let Some(first_line) = extra_lines.next()
            {
                status_line = first_line;
            }
            for line in extra_lines {
                push_unique_line(&mut detail_lines, line.clone());
                push_unique_line(&mut collapsed_preview_lines, line);
            }
        }
        // metadata (Duration, Timeout, Exit code, etc.) intentionally omitted for all tools
    } else if mark_missing_result_as_interrupted {
        status_line = ORPHANED_TOOL_RESULT.to_string();
        is_error = true;
        is_active = false;
    } else {
        status_line = default_active_tool_status_line(tool_use.name);
        is_error = false;
        is_active = true;
    }

    ToolCell {
        tool_use_id: tool_use.id.to_string(),
        tool_name: tool_use.name.to_string(),
        title: format_tool_activity_title(tool_use.name, tool_use.input, cwd),
        title_style: tool_activity_title_style(tool_use.name, tool_use.input),
        status_line,
        detail_lines,
        collapsed_preview_lines,
        prompt,
        progress_messages,
        response,
        collapsed_preview_limit: if tool_use.name.eq_ignore_ascii_case("agent") {
            3
        } else if is_file_edit_tool(tool_use.name) {
            10
        } else {
            1
        },
        is_error,
        is_active,
    }
}

#[cfg(test)]
pub(crate) fn build_tool_cell(
    messages: &[TranscriptMessage],
    start_index: usize,
    cwd: &Path,
) -> Option<(ToolCell, usize)> {
    let tool_results = collect_tool_results_by_id(messages);
    build_tool_cell_with_results(messages, start_index, cwd, &tool_results)
}

fn build_tool_cell_with_results(
    messages: &[TranscriptMessage],
    start_index: usize,
    cwd: &Path,
    tool_results: &ToolResultIndex,
) -> Option<(ToolCell, usize)> {
    let message = messages.get(start_index)?;
    let tool_uses = non_collapsible_tool_use_specs(message, cwd)?;
    let [tool_use] = tool_uses.as_slice() else {
        return None;
    };

    Some((
        build_tool_cell_from_use(*tool_use, cwd, tool_results, true),
        start_index + 1,
    ))
}

fn split_transcript_message_for_rendering(
    message: &TranscriptMessage,
) -> Option<Vec<TranscriptMessage>> {
    let tool_use_blocks = message
        .blocks
        .iter()
        .filter(|block| matches!(block, TranscriptBlock::ToolUse { .. }))
        .count();
    let tool_result_blocks = message
        .blocks
        .iter()
        .filter(|block| matches!(block, TranscriptBlock::ToolResult { .. }))
        .count();

    if matches!(message.role, MessageRole::Assistant) && tool_use_blocks > 1 {
        if assistant_message_is_agent_tool_group(message) {
            return None;
        }
        if message.blocks.iter().any(|block| {
            !matches!(
                block,
                TranscriptBlock::ToolUse { .. } | TranscriptBlock::Thinking { .. }
            )
        }) {
            return split_transcript_blocks_preserving_order(message, |block| {
                matches!(block, TranscriptBlock::ToolUse { .. })
            });
        }
        return split_transcript_blocks(message, |block| {
            matches!(block, TranscriptBlock::ToolUse { .. })
        });
    }
    if matches!(message.role, MessageRole::Assistant)
        && tool_use_blocks == 1
        && message.blocks.iter().any(|block| {
            !matches!(
                block,
                TranscriptBlock::ToolUse { .. } | TranscriptBlock::Thinking { .. }
            )
        })
    {
        return split_transcript_blocks_preserving_order(message, |block| {
            matches!(block, TranscriptBlock::ToolUse { .. })
        });
    }
    if matches!(message.role, MessageRole::User) && tool_result_blocks > 1 {
        if message.blocks.iter().any(|block| {
            !matches!(
                block,
                TranscriptBlock::ToolResult { .. } | TranscriptBlock::Thinking { .. }
            )
        }) {
            return split_transcript_blocks_with_targets_first(message, |block| {
                matches!(block, TranscriptBlock::ToolResult { .. })
            });
        }
        return split_transcript_blocks(message, |block| {
            matches!(block, TranscriptBlock::ToolResult { .. })
        });
    }
    if matches!(message.role, MessageRole::User)
        && tool_result_blocks == 1
        && message.blocks.iter().any(|block| {
            !matches!(
                block,
                TranscriptBlock::ToolResult { .. } | TranscriptBlock::Thinking { .. }
            )
        })
    {
        return split_transcript_blocks_with_targets_first(message, |block| {
            matches!(block, TranscriptBlock::ToolResult { .. })
        });
    }

    None
}

fn assistant_message_is_agent_tool_group(message: &TranscriptMessage) -> bool {
    matches!(message.role, MessageRole::Assistant)
        && message.blocks.iter().all(|block| match block {
            TranscriptBlock::Thinking { .. } => true,
            TranscriptBlock::ToolUse { name, .. } => name.eq_ignore_ascii_case("Agent"),
            _ => false,
        })
}

fn split_transcript_blocks<F>(
    message: &TranscriptMessage,
    predicate: F,
) -> Option<Vec<TranscriptMessage>>
where
    F: Fn(&TranscriptBlock) -> bool,
{
    let shared_blocks = message
        .blocks
        .iter()
        .filter(|block| matches!(block, TranscriptBlock::Thinking { .. }))
        .cloned()
        .collect::<Vec<_>>();
    let target_blocks = message
        .blocks
        .iter()
        .filter(|block| predicate(block))
        .cloned()
        .collect::<Vec<_>>();
    if target_blocks.len() <= 1 {
        return None;
    }

    Some(
        target_blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| {
                let mut blocks = Vec::new();
                if index == 0 {
                    blocks.extend(shared_blocks.clone());
                }
                blocks.push(block);
                TranscriptMessage::from_blocks(message.role.clone(), blocks)
            })
            .collect(),
    )
}

fn split_transcript_blocks_preserving_order<F>(
    message: &TranscriptMessage,
    predicate: F,
) -> Option<Vec<TranscriptMessage>>
where
    F: Fn(&TranscriptBlock) -> bool,
{
    let mut split = Vec::new();
    let mut pending = Vec::new();

    for block in &message.blocks {
        if predicate(block) {
            if !pending.is_empty() {
                split.push(TranscriptMessage::from_blocks(
                    message.role.clone(),
                    std::mem::take(&mut pending),
                ));
            }
            split.push(TranscriptMessage::from_blocks(
                message.role.clone(),
                vec![block.clone()],
            ));
        } else {
            pending.push(block.clone());
        }
    }

    if !pending.is_empty() {
        split.push(TranscriptMessage::from_blocks(
            message.role.clone(),
            pending,
        ));
    }

    (split.len() > 1).then_some(split)
}

fn has_write_content_field(input: &str) -> bool {
    parse_tool_input(input)
        .and_then(|v| v.get("content").cloned())
        .and_then(|v| v.as_str().map(|s| !s.is_empty()))
        .unwrap_or(false)
}

fn split_transcript_blocks_with_targets_first<F>(
    message: &TranscriptMessage,
    predicate: F,
) -> Option<Vec<TranscriptMessage>>
where
    F: Fn(&TranscriptBlock) -> bool,
{
    let mut split = Vec::new();
    let mut non_target_blocks = Vec::new();

    for block in &message.blocks {
        if predicate(block) {
            split.push(TranscriptMessage::from_blocks(
                message.role.clone(),
                vec![block.clone()],
            ));
        } else {
            non_target_blocks.push(block.clone());
        }
    }

    if !non_target_blocks.is_empty() {
        split.push(TranscriptMessage::from_blocks(
            message.role.clone(),
            non_target_blocks,
        ));
    }

    (split.len() > 1).then_some(split)
}
