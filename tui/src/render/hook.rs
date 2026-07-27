use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;

use crate::embedded_progress::{hook_progress_detail_line, hook_progress_is_error};
use crate::history_cell::hook_note::HookTranscriptNote;
use crate::render::styled_wrap::{
    render_prefixed_wrapped_spans, tool_body_prefix, transcript_content_width, wrap_styled_line,
};
use crate::render::text_utils::{StyledLine, compact_blank_lines};
use crate::tui_theme::{active_palette, inactive_style, subtle_style};

pub(crate) fn render_hook_transcript_note_lines(
    note: HookTranscriptNote,
    hook_progress: &[Value],
    transcript_width: usize,
) -> Vec<StyledLine> {
    let width = transcript_width.max(1);
    let content_width = transcript_content_width(width);
    let note_style = hook_note_style(note.is_error);
    let mut lines = wrap_styled_line(
        &Line::from(vec![
            Span::styled("●", note_style),
            Span::raw(" "),
            Span::styled(note.title, note_style),
        ]),
        content_width,
    );

    let mut child_lines = Vec::new();
    if let Some(body) = note.body.filter(|body| !body.trim().is_empty()) {
        for raw_line in body.lines() {
            if raw_line.trim().is_empty() {
                continue;
            }

            child_lines.push((raw_line.to_string(), note_style));
        }
    }

    for progress in hook_progress {
        let Some(line) = hook_progress_detail_line(progress) else {
            continue;
        };
        let progress_style = hook_note_style(hook_progress_is_error(progress));
        child_lines.push((line, progress_style));
    }

    let child_line_count = child_lines.len();
    for (index, (line, style)) in child_lines.into_iter().enumerate() {
        let prefix = tool_body_prefix(index, child_line_count);
        lines.extend(render_prefixed_wrapped_spans(
            vec![Span::styled(line, style)],
            prefix,
            "     ",
            subtle_style(),
            width,
        ));
    }

    compact_blank_lines(lines)
}

fn hook_note_style(is_error: bool) -> Style {
    if is_error {
        Style::default().fg(active_palette().error)
    } else {
        inactive_style().add_modifier(Modifier::DIM)
    }
}
