use ratatui::{
    prelude::Style,
    text::{Line, Span},
};

use crate::history_cell::local_note::LocalTranscriptNote;
use crate::render::request_status::{WAITING_COMPLETION_VERBS, format_token_estimate};
use crate::render::slash::{render_context_compacted_lines, render_slash_command_output_lines};
use crate::render::text_utils::{StyledLine, format_duration_short};
use crate::tui_theme::{active_palette, emphasis_style, subtle_style};

pub(crate) fn render_local_transcript_note_lines(
    note: LocalTranscriptNote,
    transcript_width: usize,
    show_tool_details: bool,
) -> Vec<StyledLine> {
    match note {
        LocalTranscriptNote::TurnDuration {
            duration_ms,
            verb_index,
            total_tokens,
        } => {
            let verb = WAITING_COMPLETION_VERBS[verb_index % WAITING_COMPLETION_VERBS.len()];
            let style = emphasis_style();
            let mut spans = vec![
                Span::styled("✻", style),
                Span::raw(" "),
                Span::styled(
                    format!("{verb} for {}", format_duration_short(duration_ms)),
                    style,
                ),
            ];
            if total_tokens > 0 {
                spans.push(Span::styled(
                    format!("  · {} tokens", format_token_estimate(total_tokens)),
                    subtle_style(),
                ));
            }
            vec![Line::from(spans)]
        }
        LocalTranscriptNote::Error { message } => render_local_error_lines(&message),
        LocalTranscriptNote::ContextCompacted {
            duration_ms,
            summary,
        } => render_context_compacted_lines(
            duration_ms,
            summary.as_deref(),
            transcript_width,
            show_tool_details,
        ),
        LocalTranscriptNote::SlashCommandOutput {
            command,
            summary,
            detail_markdown,
            deferred_feedback,
        } => render_slash_command_output_lines(
            &command,
            &summary,
            detail_markdown.as_deref(),
            deferred_feedback,
            transcript_width,
            show_tool_details,
        ),
    }
}

fn render_local_error_lines(message: &str) -> Vec<StyledLine> {
    let style = Style::default().fg(active_palette().error);
    let mut lines = Vec::new();
    let mut body_lines = message.lines();

    if let Some(first) = body_lines.next() {
        lines.push(Line::from(vec![
            Span::styled("●", style),
            Span::raw(" "),
            Span::styled(first.to_string(), style),
        ]));
    } else {
        lines.push(Line::from(vec![Span::styled("●", style)]));
    }

    for line in body_lines {
        if line.trim().is_empty() {
            lines.push(Line::default());
        } else {
            lines.push(Line::from(vec![
                Span::styled("  ", subtle_style()),
                Span::styled(line.to_string(), style),
            ]));
        }
    }

    lines
}
