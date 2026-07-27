use orbcode_protocol::TranscriptMessage;
use ratatui::text::{Line, Span};

use crate::render::styled_wrap::{transcript_content_width, wrap_styled_line};
use crate::render::text_utils::{StyledLine, display_width};
use crate::tui_theme::user_bar_style;

pub(crate) fn render_user_message_lines(
    message: &TranscriptMessage,
    transcript_width: usize,
) -> Vec<StyledLine> {
    let content = if message.content.is_empty() {
        " ".to_string()
    } else {
        message.content.clone()
    };
    let width = transcript_width.max(1);
    let content_width = transcript_content_width(width);
    let mut lines = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let prefix = if index == 0 { "› " } else { "  " };
        let wrapped = wrap_styled_line(
            &Line::from(vec![
                Span::styled(prefix.to_string(), user_bar_style()),
                Span::styled(line.to_string(), user_bar_style()),
            ]),
            content_width,
        );
        for wrapped_line in wrapped {
            lines.push(fill_user_bar_line(wrapped_line, width));
        }
    }

    if lines.is_empty() {
        lines.push(fill_user_bar_line(
            Line::from(vec![Span::styled("› ".to_string(), user_bar_style())]),
            width,
        ));
    }

    lines
}

pub(crate) fn fill_user_bar_line(line: StyledLine, width: usize) -> StyledLine {
    let mut spans = line.spans;
    let line_width = spans
        .iter()
        .flat_map(|span| span.content.chars())
        .map(display_width)
        .sum::<usize>();
    if line_width < width {
        spans.push(Span::styled(
            " ".repeat(width - line_width),
            user_bar_style(),
        ));
    }
    Line::from(spans)
}
