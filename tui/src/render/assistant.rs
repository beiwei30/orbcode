use chrono::{Local, Utc};
use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use crate::render::markdown::render_markdown_body_lines;
use crate::render::styled_wrap::transcript_content_width;
use crate::render::text_utils::{StyledLine, truncate_chars};
use crate::tui_theme::{emphasis_style, inactive_style, subtle_style};

pub(crate) fn assistant_bullet(role: &MessageRole) -> Span<'static> {
    match role {
        MessageRole::User => Span::styled("●", emphasis_style()),
        _ => Span::styled("●", inactive_style()),
    }
}

pub(crate) fn render_assistant_text_line(body: &str, accent: Span<'static>) -> StyledLine {
    Line::from(vec![
        accent,
        Span::raw(" "),
        Span::styled(body.to_string(), inactive_style()),
    ])
}

pub(crate) fn render_assistant_fallback_lines(
    message: &TranscriptMessage,
    transcript_width: usize,
    model_display_name: &str,
    show_metadata: bool,
) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    if matches!(message.role, MessageRole::Assistant)
        && show_metadata
        && let Some(metadata_line) =
            render_assistant_metadata_line(message, transcript_width, model_display_name)
    {
        lines.push(metadata_line);
    }
    lines.push(render_assistant_text_line(
        &message.content,
        assistant_bullet(&message.role),
    ));
    lines
}

pub(crate) fn render_assistant_markdown_lines(
    text: &str,
    base_style: Style,
    available_width: usize,
) -> Vec<StyledLine> {
    let lines = render_markdown_body_lines(text, base_style, available_width);

    if lines.is_empty() {
        vec![Line::from(vec![assistant_bullet(&MessageRole::Assistant)])]
    } else {
        prefix_assistant_markdown_lines(lines)
    }
}

fn prefix_assistant_markdown_lines(lines: Vec<StyledLine>) -> Vec<StyledLine> {
    let mut prefixed = Vec::with_capacity(lines.len());
    let mut first_content_line = true;

    for line in lines {
        if line.spans.is_empty() {
            prefixed.push(line);
            continue;
        }

        let prefix = if first_content_line {
            vec![assistant_bullet(&MessageRole::Assistant), Span::raw(" ")]
        } else {
            vec![Span::styled("  ", subtle_style())]
        };
        first_content_line = false;

        let mut spans = prefix;
        spans.extend(line.spans);
        prefixed.push(Line::from(spans));
    }

    prefixed
}

pub(crate) fn render_pending_assistant_lines(
    pending: &str,
    transcript_width: usize,
) -> Vec<StyledLine> {
    render_assistant_markdown_lines(
        pending,
        inactive_style(),
        transcript_content_width(transcript_width)
            .saturating_sub(2)
            .max(1),
    )
}

fn assistant_metadata_label(
    message: &TranscriptMessage,
    model_display_name: &str,
) -> Option<String> {
    if !matches!(message.role, MessageRole::Assistant) {
        return None;
    }
    let has_visible_text = if message.blocks.is_empty() {
        !message.content.trim().is_empty()
    } else {
        message
            .blocks
            .iter()
            .any(|block| matches!(block, TranscriptBlock::Text { text } if !text.trim().is_empty()))
    };
    if !has_visible_text {
        return None;
    }

    let timestamp = format_brief_timestamp(message.created_at);
    let model_label = model_display_name.trim();
    match (timestamp.is_empty(), model_label.is_empty()) {
        (true, true) => None,
        (false, true) => Some(timestamp),
        (true, false) => Some(model_label.to_string()),
        (false, false) => Some(format!("{timestamp} {model_label}")),
    }
}

pub(crate) fn render_assistant_metadata_line(
    message: &TranscriptMessage,
    transcript_width: usize,
    model_display_name: &str,
) -> Option<StyledLine> {
    let label = assistant_metadata_label(message, model_display_name)?;

    let truncated = truncate_chars(&label, transcript_width.max(1));
    let padding = transcript_width.saturating_sub(truncated.chars().count());
    Some(Line::from(vec![
        Span::raw(" ".repeat(padding)),
        Span::styled(truncated, inactive_style().add_modifier(Modifier::DIM)),
    ]))
}

fn format_brief_timestamp(timestamp: chrono::DateTime<Utc>) -> String {
    let local = timestamp.with_timezone(&Local);
    let now = Local::now();
    let days_ago = now
        .date_naive()
        .signed_duration_since(local.date_naive())
        .num_days();

    if days_ago == 0 {
        local.format("%-I:%M %p").to_string()
    } else if (1..7).contains(&days_ago) {
        local.format("%A, %-I:%M %p").to_string()
    } else {
        local.format("%A, %b %-d, %-I:%M %p").to_string()
    }
}
