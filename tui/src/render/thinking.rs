#[cfg(test)]
use orbcode_protocol::MessageRole;
use orbcode_protocol::{TranscriptBlock, TranscriptMessage};
use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use crate::prompt_state::ActiveThinkingState;
use crate::render::styled_wrap::{render_prefixed_wrapped_spans, transcript_content_width};
use crate::render::text_utils::{
    StyledLine, collapse_inline_whitespace, compact_blank_lines, display_width_str,
    truncate_display_width,
};
use crate::state::TuiState;
use crate::tui_theme::{active_palette, inactive_style, subtle_style};

pub(crate) const THINKING_RETENTION_MS: u64 = 30_000;

impl TuiState {
    pub(crate) fn is_active_thinking_visible(&self) -> bool {
        self.active_thinking.as_ref().is_some_and(|thinking| {
            thinking.is_streaming
                || thinking.completed_at.is_some_and(|completed_at| {
                    completed_at.elapsed().as_millis() < THINKING_RETENTION_MS as u128
                })
        })
    }

    pub(crate) fn prune_active_thinking(&mut self) {
        let should_clear = self.active_thinking.as_ref().is_some_and(|thinking| {
            !thinking.is_streaming
                && thinking.completed_at.is_none_or(|completed_at| {
                    completed_at.elapsed().as_millis() >= THINKING_RETENTION_MS as u128
                })
        });
        if should_clear {
            self.active_thinking = None;
        }
    }
}

pub(crate) fn render_thinking_block_lines(text: &str, transcript_width: usize) -> Vec<StyledLine> {
    let heading_style = inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM);
    let body_style = subtle_style().add_modifier(Modifier::DIM);
    let mut lines = vec![Line::from(Span::styled("∴ Thinking...", heading_style))];

    for line in text.trim().lines() {
        if line.trim().is_empty() {
            lines.push(Line::default());
        } else {
            lines.extend(render_prefixed_wrapped_spans(
                vec![Span::styled(line.to_string(), body_style)],
                "  ",
                "  ",
                subtle_style(),
                transcript_width,
            ));
        }
    }

    compact_blank_lines(lines)
}

pub(crate) fn render_collapsed_committed_thinking_lines(
    text: &str,
    transcript_width: usize,
) -> Vec<StyledLine> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "∴ Thinking",
            inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM),
        ),
        Span::raw(" "),
        Span::styled("(ctrl+o to expand)", subtle_style()),
    ])];
    let preview_width = collapsed_preview_width(transcript_width);
    if let Some(preview) = collapsed_preview_line(text, preview_width) {
        lines.push(Line::from(vec![
            Span::styled("  └ ", subtle_style()),
            Span::styled(
                preview,
                inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM),
            ),
        ]));
    }
    lines
}

pub(crate) fn render_active_thinking_lines(
    thinking: &ActiveThinkingState,
    expanded: bool,
    spinner: char,
    verb: &str,
    transcript_width: usize,
) -> Vec<StyledLine> {
    if expanded {
        if thinking.text.trim().is_empty() {
            return vec![Line::from(Span::styled(
                "∴ Thinking...",
                inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM),
            ))];
        }
        return render_thinking_block_lines(&thinking.text, transcript_width);
    }

    let preview_width = collapsed_preview_width(transcript_width);

    if thinking.is_streaming {
        let mut lines = vec![Line::from(vec![
            Span::styled(
                spinner.to_string(),
                Style::default()
                    .fg(active_palette().claude)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{verb}… (thinking)"),
                inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM),
            ),
        ])];
        if let Some(preview) = collapsed_preview_line(&thinking.text, preview_width) {
            lines.push(Line::from(vec![
                Span::styled("  └ ", subtle_style()),
                Span::styled(
                    preview,
                    inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM),
                ),
            ]));
        }
        return lines;
    }

    let mut lines = vec![Line::from(vec![
        Span::styled(
            "∴ Thinking",
            inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM),
        ),
        Span::raw(" "),
        Span::styled("(ctrl+o to expand)", subtle_style()),
    ])];
    if let Some(preview) = collapsed_preview_line(&thinking.text, preview_width) {
        lines.push(Line::from(vec![
            Span::styled("  └ ", subtle_style()),
            Span::styled(
                preview,
                inactive_style().add_modifier(Modifier::ITALIC | Modifier::DIM),
            ),
        ]));
    }
    lines
}

pub(crate) fn message_has_non_thinking_block(message: &TranscriptMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|block| !matches!(block, TranscriptBlock::Thinking { .. }))
}

pub(crate) fn message_contains_matching_thinking_block(
    message: &TranscriptMessage,
    thinking_message: &TranscriptMessage,
) -> bool {
    thinking_message
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::Thinking { text, .. } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .any(|thinking_text| {
            message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::Thinking { text, .. } if text.trim() == thinking_text
                )
            })
        })
}

#[cfg(test)]
pub(crate) fn last_visible_thinking_block(
    messages: &[TranscriptMessage],
) -> Option<(String, usize)> {
    for message in messages.iter().rev() {
        match message.role {
            MessageRole::Assistant => {
                for (index, block) in message.blocks.iter().enumerate().rev() {
                    if matches!(block, TranscriptBlock::Thinking { .. }) {
                        return Some((message.id.clone(), index));
                    }
                }
            }
            MessageRole::User => {
                let has_tool_result = message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, TranscriptBlock::ToolResult { .. }));
                if !has_tool_result {
                    return None;
                }
            }
            _ => {}
        }
    }

    None
}

fn collapsed_preview_width(transcript_width: usize) -> usize {
    transcript_content_width(transcript_width)
        .saturating_sub(display_width_str("  └ "))
        .max(1)
}

fn collapsed_preview_line(text: &str, max_display_width: usize) -> Option<String> {
    text.lines()
        .rev()
        .map(collapse_inline_whitespace)
        .find(|line| !line.trim().is_empty())
        .map(|line| truncate_display_width(&line, max_display_width))
}
