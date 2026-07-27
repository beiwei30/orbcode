use std::path::Path;

use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;

use crate::embedded_progress::{
    embedded_progress_message_to_transcript, normalize_progress_label,
    should_render_embedded_progress_message, should_render_tool_progress_message,
    tool_progress_status_line,
};
use crate::render::markdown::render_markdown_body_lines;
use crate::render::message::render_message_lines;
use crate::render::styled_wrap::{
    tool_body_prefix, tool_body_tree_prefix, transcript_content_width,
};
use crate::render::text_utils::{
    StyledLine, collapse_inline_whitespace, display_width_str, push_unique_line, truncate_chars,
    truncate_display_width,
};
use crate::tool_cell::ToolCell;
use crate::tool_cell::live_state::LiveToolActivity;
use crate::tool_cell::summary::{
    SHELL_CWD_RESET_PREFIX, default_active_tool_status_line, format_tool_activity_title,
    is_bash_like_tool, is_file_edit_tool, is_file_read_like_tool, tool_activity_detail_lines,
    tool_activity_title_style,
};
use crate::tui_theme::{ERROR_PINK, SUCCESS_GREEN, active_palette, inactive_style, subtle_style};

fn tool_body_tree_lines(
    status_line: &str,
    detail_lines: &[String],
    progress_status_lines: &[String],
) -> Vec<String> {
    let mut lines = vec![status_line.to_string()];
    for line in detail_lines.iter().chain(progress_status_lines.iter()) {
        if line.trim().is_empty() {
            continue;
        }
        let duplicates_status = lines
            .first()
            .is_some_and(|status| tool_card_preview_duplicates_status(status, line));
        if !duplicates_status && !lines.iter().any(|existing| existing == line) {
            lines.push(line.clone());
        }
    }
    lines
}

fn tool_card_line_with_expand_hint(line: &str, transcript_width: usize) -> String {
    const EXPAND_HINT: &str = " (ctrl+o to expand)";

    let body_width = tool_body_width(transcript_width, "  └ ");
    let hint_width = display_width_str(EXPAND_HINT);
    if body_width <= hint_width {
        return truncate_display_width(EXPAND_HINT.trim_start(), body_width);
    }

    let line_width = body_width.saturating_sub(hint_width);
    format!(
        "{}{}",
        truncate_display_width(line, line_width),
        EXPAND_HINT
    )
}

fn fit_tool_body_lines_to_width(lines: &[String], transcript_width: usize) -> Vec<String> {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let width = tool_body_width(transcript_width, tool_body_prefix(index, lines.len()));
            truncate_display_width(line, width)
        })
        .collect()
}

fn indent_styled_lines(lines: Vec<StyledLine>, prefix: &str) -> Vec<StyledLine> {
    lines
        .into_iter()
        .map(|line| {
            if line.spans.is_empty() {
                line
            } else {
                let mut spans = vec![Span::styled(prefix.to_string(), subtle_style())];
                spans.extend(line.spans);
                Line::from(spans)
            }
        })
        .collect()
}

fn render_tool_activity_section_lines(
    title: &str,
    body: &str,
    transcript_width: usize,
) -> Vec<StyledLine> {
    let mut lines = vec![Line::from(vec![
        Span::styled("  └ ", subtle_style()),
        Span::styled(
            title.to_string(),
            Style::default()
                .fg(active_palette().success)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let body_lines = render_markdown_body_lines(
        body,
        inactive_style(),
        transcript_content_width(transcript_width)
            .saturating_sub(6)
            .max(1),
    );
    lines.extend(indent_styled_lines(body_lines, "      "));
    lines
}

fn tool_activity_progress_status_lines(progress_messages: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for progress in progress_messages
        .iter()
        .filter(|progress| should_render_tool_progress_message(progress))
        .filter(|progress| embedded_progress_message_to_transcript(progress).is_none())
    {
        if let Some(status) = tool_progress_status_line(progress) {
            push_unique_line(&mut lines, status);
        }
    }
    lines
}

fn render_tool_activity_progress_lines(
    progress_messages: &[Value],
    cwd: &Path,
    transcript_width: usize,
    allow_embedded_tool_messages: bool,
    include_status_lines: bool,
    current_status_line: Option<&str>,
) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    let mut rendered_status_lines = Vec::new();
    for progress in progress_messages {
        if !should_render_tool_progress_message(progress) {
            continue;
        }
        let Some(message) = embedded_progress_message_to_transcript(progress) else {
            if include_status_lines && let Some(status) = tool_progress_status_line(progress) {
                if current_status_line == Some(status.as_str()) {
                    continue;
                }
                if rendered_status_lines.iter().any(|line| line == &status) {
                    continue;
                }
                rendered_status_lines.push(status.clone());
                lines.push(Line::from(vec![
                    Span::styled("      ", subtle_style()),
                    Span::styled(status, inactive_style().add_modifier(Modifier::DIM)),
                ]));
            }
            continue;
        };
        if !should_render_embedded_progress_message(&message, allow_embedded_tool_messages) {
            continue;
        }
        let rendered = render_message_lines(
            &message,
            cwd,
            true,
            None,
            transcript_content_width(transcript_width)
                .saturating_sub(6)
                .max(1),
            "subagent",
            false,
        );
        lines.extend(indent_styled_lines(rendered, "      "));
    }
    lines
}

fn tool_activity_progress_preview_lines(
    progress_messages: &[Value],
    cwd: &Path,
    transcript_width: usize,
    limit: usize,
    allow_embedded_tool_messages: bool,
) -> Vec<String> {
    let mut previews = Vec::new();
    let preview_width = transcript_content_width(transcript_width)
        .saturating_sub(6)
        .max(24);

    for progress in progress_messages {
        if !should_render_tool_progress_message(progress) {
            continue;
        }
        if let Some(message) = embedded_progress_message_to_transcript(progress) {
            if !should_render_embedded_progress_message(&message, allow_embedded_tool_messages) {
                continue;
            }
            let rendered =
                render_message_lines(&message, cwd, true, None, preview_width, "subagent", false);
            for line in rendered {
                let text = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                let collapsed = collapse_inline_whitespace(&text);
                if collapsed.is_empty() {
                    continue;
                }
                push_unique_line(&mut previews, truncate_chars(&collapsed, preview_width));
            }
            continue;
        }

        if let Some(status) = tool_progress_status_line(progress) {
            push_unique_line(&mut previews, truncate_chars(&status, preview_width));
        }
    }

    let start = previews.len().saturating_sub(limit);
    previews.into_iter().skip(start).collect()
}

pub(crate) fn black_circle_glyph() -> &'static str {
    if cfg!(target_os = "macos") {
        "⏺"
    } else {
        "●"
    }
}

pub(crate) fn render_live_tool_activity_lines(
    activity: &LiveToolActivity,
    expanded: bool,
    cwd: &Path,
    blink_visible: bool,
    transcript_width: usize,
) -> Vec<StyledLine> {
    let cell = tool_cell_from_live_activity(activity, cwd, transcript_width);
    let indicator_override = if activity.is_error {
        None
    } else {
        Some((
            if blink_visible {
                black_circle_glyph().to_string()
            } else {
                " ".to_string()
            },
            inactive_style(),
        ))
    };
    render_tool_cell_lines(&cell, expanded, indicator_override, transcript_width, cwd)
}

fn tool_cell_from_live_activity(
    activity: &LiveToolActivity,
    cwd: &Path,
    transcript_width: usize,
) -> ToolCell {
    ToolCell {
        tool_use_id: activity.tool_use_id.clone(),
        tool_name: activity.tool_name.clone(),
        title: format_tool_activity_title(&activity.tool_name, &activity.tool_input, cwd),
        title_style: tool_activity_title_style(&activity.tool_name, &activity.tool_input),
        status_line: normalize_progress_label(&activity.status_line),
        detail_lines: tool_activity_detail_lines(&activity.tool_name, &activity.tool_input, cwd),
        collapsed_preview_lines: tool_activity_progress_preview_lines(
            &activity.progress_messages,
            cwd,
            transcript_width,
            2,
            activity.tool_name.eq_ignore_ascii_case("agent"),
        ),
        prompt: None,
        progress_messages: activity.progress_messages.clone(),
        response: None,
        collapsed_preview_limit: 2,
        is_error: activity.is_error,
        is_active: true,
    }
}

pub(crate) fn tool_cell_with_live_activity(
    committed: &ToolCell,
    activity: &LiveToolActivity,
    cwd: &Path,
    transcript_width: usize,
) -> ToolCell {
    let mut live = tool_cell_from_live_activity(activity, cwd, transcript_width);
    live.prompt = committed.prompt.clone();
    live.response = committed.response.clone();
    live.collapsed_preview_limit = committed.collapsed_preview_limit;
    if live.detail_lines.is_empty() {
        live.detail_lines = committed.detail_lines.clone();
    }
    if live.collapsed_preview_lines.is_empty() {
        live.collapsed_preview_lines = committed.collapsed_preview_lines.clone();
    }
    live
}

pub(crate) fn active_tool_cell_from_committed_orphan(committed: &ToolCell) -> ToolCell {
    let mut active = committed.clone();
    active.status_line = default_active_tool_status_line(&active.tool_name);
    active.collapsed_preview_lines.clear();
    active.progress_messages.clear();
    active.response = None;
    active.is_error = false;
    active.is_active = true;
    active
}

pub(crate) fn queued_tool_cell_from_committed_orphan(committed: &ToolCell) -> ToolCell {
    let mut queued = committed.clone();
    queued.status_line = "Queued behind permission…".to_string();
    queued.collapsed_preview_lines.clear();
    queued.progress_messages.clear();
    queued.response = None;
    queued.is_error = false;
    queued.is_active = false;
    queued
}

fn render_tool_body_lines(
    body_lines: &[String],
    style_for_line: &impl Fn(&str) -> Style,
    is_error: bool,
) -> Vec<StyledLine> {
    body_lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if is_error {
                tool_body_tree_prefix(index, body_lines.len())
            } else {
                tool_body_prefix(index, body_lines.len())
            };
            Line::from(vec![
                Span::styled(prefix, subtle_style()),
                Span::styled(line.clone(), style_for_line(line)),
            ])
        })
        .collect()
}

pub(crate) fn render_tool_cell_lines(
    card: &ToolCell,
    expanded: bool,
    indicator_override: Option<(String, Style)>,
    transcript_width: usize,
    cwd: &Path,
) -> Vec<StyledLine> {
    let allow_embedded_tool_progress = card.tool_name.eq_ignore_ascii_case("agent");
    let (indicator, indicator_style) = indicator_override.unwrap_or_else(|| {
        (
            black_circle_glyph().to_string(),
            Style::default().fg(if card.is_error {
                ERROR_PINK
            } else {
                SUCCESS_GREEN
            }),
        )
    });
    let body_style = |line: &str| {
        if card.is_error {
            Style::default().fg(active_palette().error)
        } else if is_bash_like_tool(&card.tool_name) && line.starts_with(SHELL_CWD_RESET_PREFIX) {
            subtle_style().add_modifier(Modifier::DIM)
        } else if is_file_edit_tool(&card.tool_name) && is_diff_removal_line(line) {
            Style::default().fg(ERROR_PINK)
        } else if is_file_edit_tool(&card.tool_name) && is_diff_addition_line(line) {
            Style::default().fg(SUCCESS_GREEN)
        } else {
            inactive_style().add_modifier(Modifier::DIM)
        }
    };
    let title_spans = tool_title_spans(&card.title, card.title_style);
    let uses_inline_preview =
        is_bash_like_tool(&card.tool_name) && !card.is_active && !card.is_error;
    let mut title_line_spans = vec![Span::styled(indicator, indicator_style), Span::raw(" ")];
    title_line_spans.extend(title_spans);
    let mut lines = vec![Line::from(title_line_spans)];
    let uses_expanded_tool_body_tree = expanded
        && !card.is_active
        && !card.tool_name.eq_ignore_ascii_case("agent")
        && (is_bash_like_tool(&card.tool_name)
            || !card.detail_lines.is_empty()
            || !card.progress_messages.is_empty());
    let expanded_tool_progress_status_lines =
        if uses_expanded_tool_body_tree && !uses_inline_preview {
            tool_activity_progress_status_lines(&card.progress_messages)
        } else {
            Vec::new()
        };

    let mut collapsed_preview_lines = card.collapsed_preview_lines.clone();
    if !card.progress_messages.is_empty() && !uses_inline_preview {
        for line in tool_activity_progress_preview_lines(
            &card.progress_messages,
            cwd,
            transcript_width,
            card.collapsed_preview_limit.max(1),
            allow_embedded_tool_progress,
        ) {
            push_unique_line(&mut collapsed_preview_lines, line);
        }
    }

    let should_render_collapsed_edit_preview = is_file_edit_tool(&card.tool_name)
        && !card.is_active
        && !card.is_error
        && !collapsed_preview_lines.is_empty();
    let should_render_collapsed_preview_lines = !expanded
        && !uses_inline_preview
        && (card.is_active
            || !card.progress_messages.is_empty()
            || card.is_error
            || should_render_collapsed_edit_preview);
    let primary_body_lines = if uses_expanded_tool_body_tree {
        tool_body_tree_lines(
            &card.status_line,
            &card.detail_lines,
            &expanded_tool_progress_status_lines,
        )
    } else if !expanded && uses_inline_preview {
        std::iter::once(card.status_line.clone())
            .chain(collapsed_preview_lines.iter().cloned())
            .collect::<Vec<_>>()
    } else if should_render_collapsed_preview_lines {
        let preview_start = collapsed_preview_lines
            .len()
            .saturating_sub(card.collapsed_preview_limit.max(1));
        let mut preview_lines = collapsed_preview_lines
            .iter()
            .skip(preview_start)
            .cloned()
            .collect::<Vec<_>>();
        if preview_lines.len() == 1
            && tool_card_preview_duplicates_status(&card.status_line, &preview_lines[0])
        {
            vec![tool_card_line_with_expand_hint(
                &preview_lines[0],
                transcript_width,
            )]
        } else {
            if let Some(last_line) = preview_lines.last_mut() {
                *last_line = tool_card_line_with_expand_hint(last_line, transcript_width);
            }
            std::iter::once(card.status_line.clone())
                .chain(preview_lines)
                .collect::<Vec<_>>()
        }
    } else {
        vec![card.status_line.clone()]
    };
    let primary_body_lines = fit_tool_body_lines_to_width(&primary_body_lines, transcript_width);
    lines.extend(render_tool_body_lines(
        &primary_body_lines,
        &body_style,
        card.is_error,
    ));

    if !expanded
        && !uses_inline_preview
        && (card.prompt.is_some()
            || !card.progress_messages.is_empty()
            || card.response.is_some()
            || !card.detail_lines.is_empty()
            || !collapsed_preview_lines.is_empty())
        && !should_render_collapsed_preview_lines
    {
        lines.push(Line::from(vec![Span::styled(
            "(ctrl+o to expand)",
            subtle_style(),
        )]));
    }

    if expanded {
        if !(card.is_active
            && (is_bash_like_tool(&card.tool_name) || is_file_read_like_tool(&card.tool_name)))
            && !uses_expanded_tool_body_tree
            && !card.detail_lines.is_empty()
        {
            for line in card.detail_lines.iter().take(6) {
                lines.push(Line::from(vec![
                    Span::styled("    ", subtle_style()),
                    Span::styled(line.clone(), inactive_style().add_modifier(Modifier::DIM)),
                ]));
            }
        }
        if let Some(prompt) = &card.prompt {
            lines.extend(render_tool_activity_section_lines(
                "Prompt:",
                prompt,
                transcript_width,
            ));
        }
        let include_progress_status = !uses_expanded_tool_body_tree && !uses_inline_preview;
        let progress_lines = render_tool_activity_progress_lines(
            &card.progress_messages,
            cwd,
            transcript_width,
            allow_embedded_tool_progress,
            include_progress_status,
            Some(&card.status_line),
        );
        if !progress_lines.is_empty() {
            lines.extend(progress_lines);
        }
        if let Some(response) = &card.response {
            lines.extend(render_tool_activity_section_lines(
                "Response:",
                response,
                transcript_width,
            ));
        }
    }

    lines
}

fn tool_body_width(transcript_width: usize, prefix: &str) -> usize {
    transcript_content_width(transcript_width)
        .saturating_sub(display_width_str(prefix))
        .max(1)
}

fn tool_card_preview_duplicates_status(status: &str, preview: &str) -> bool {
    let status = collapse_inline_whitespace(status);
    let preview = collapse_inline_whitespace(preview);
    let status_prefix = status.trim_end_matches('…');
    let preview_prefix = preview.trim_end_matches('…');

    status == preview
        || (!status_prefix.is_empty() && preview.starts_with(status_prefix))
        || (!preview_prefix.is_empty() && status.starts_with(preview_prefix))
}

fn tool_title_spans(title: &str, base_style: Style) -> Vec<Span<'static>> {
    let name_style = base_style.add_modifier(Modifier::BOLD);
    let suffix_style = base_style.remove_modifier(Modifier::BOLD);
    if let Some(start) = title.find('(')
        && title.ends_with(')')
    {
        let (name, suffix) = title.split_at(start);
        return vec![
            Span::styled(name.to_string(), name_style),
            Span::styled(suffix.to_string(), suffix_style),
        ];
    }

    vec![Span::styled(title.to_string(), name_style)]
}

fn is_diff_removal_line(line: &str) -> bool {
    diff_line_marker(line) == Some('-')
}

fn is_diff_addition_line(line: &str) -> bool {
    diff_line_marker(line) == Some('+')
}

fn diff_line_marker(line: &str) -> Option<char> {
    let mut chars = line.trim_start().chars().peekable();
    while chars.peek().is_some_and(char::is_ascii_digit) {
        chars.next();
    }
    while chars.peek().is_some_and(|ch| *ch == ' ') {
        chars.next();
    }
    match chars.next()? {
        '+' => Some('+'),
        '-' => Some('-'),
        _ => None,
    }
}
