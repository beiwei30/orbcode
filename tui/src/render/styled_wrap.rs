use ratatui::{
    layout::Constraint,
    prelude::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use crate::render::text_utils::{StyledLine, display_width, display_width_str};

pub(crate) fn wrap_styled_lines(lines: &[StyledLine], width: usize) -> Vec<StyledLine> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    for line in lines {
        wrapped.extend(wrap_styled_line(line, width));
    }
    wrapped
}

/// A width-specific projection from logical styled lines to terminal visual
/// rows. Consumers use the source map for selection anchoring and use the
/// projected rows directly, so bounds and rendering cannot apply different
/// wrapping passes.
#[derive(Clone, Debug, Default)]
pub(crate) struct ViewportProjection {
    pub(crate) visual_rows: Vec<StyledLine>,
    pub(crate) source_line_by_visual_row: Vec<usize>,
}

pub(crate) fn ensure_source_range_visible(
    source_line_by_visual_row: &[usize],
    visual_row_count: usize,
    requested_scroll: usize,
    viewport_height: usize,
    source_start: usize,
    source_end: usize,
) -> usize {
    if viewport_height == 0 {
        return 0;
    }
    let first = source_line_by_visual_row
        .iter()
        .position(|source| *source == source_start);
    let last = source_line_by_visual_row
        .iter()
        .rposition(|source| *source == source_end)
        .map(|row| row.saturating_add(1));
    let max_scroll = visual_row_count.saturating_sub(viewport_height);
    let (Some(first), Some(last)) = (first, last) else {
        return requested_scroll.min(max_scroll);
    };
    let mut scroll = requested_scroll.min(max_scroll);
    if first < scroll {
        scroll = first;
    } else if last > scroll.saturating_add(viewport_height) {
        scroll = last.saturating_sub(viewport_height);
    }
    scroll.min(max_scroll)
}

pub(crate) fn project_styled_lines(lines: &[StyledLine], width: usize) -> ViewportProjection {
    let mut projection = ViewportProjection::default();
    for (source_line, line) in lines.iter().enumerate() {
        let wrapped = wrap_styled_line(line, width);
        projection
            .source_line_by_visual_row
            .extend(std::iter::repeat_n(source_line, wrapped.len()));
        projection.visual_rows.extend(wrapped);
    }
    projection
}

/// Code-oriented wrapping that preserves whitespace, expands tabs at four-cell
/// stops, and keeps zero-width combining marks attached to the current row.
pub(crate) fn wrap_styled_line_preserving_whitespace(
    line: &StyledLine,
    width: usize,
) -> Vec<StyledLine> {
    const TAB_STOP: usize = 4;

    let width = width.max(1);
    let mut wrapped = Vec::new();
    let mut current_spans = Vec::new();
    let mut current_width = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            let ch_width = if ch == '\t' {
                TAB_STOP - (current_width % TAB_STOP)
            } else {
                UnicodeWidthChar::width(ch).unwrap_or(0)
            };
            if current_width > 0 && current_width.saturating_add(ch_width) > width {
                push_preserved_visual_row(&mut wrapped, &mut current_spans, &mut current_width);
            }
            if ch == '\t' {
                for _ in 0..ch_width {
                    push_styled_char(&mut current_spans, ' ', span.style);
                }
            } else {
                push_styled_char(&mut current_spans, ch, span.style);
            }
            current_width = current_width.saturating_add(ch_width);
        }
    }
    if current_spans.is_empty() {
        if wrapped.is_empty() {
            wrapped.push(Line::default());
        }
    } else {
        push_preserved_visual_row(&mut wrapped, &mut current_spans, &mut current_width);
    }
    wrapped
}

fn push_preserved_visual_row(
    wrapped: &mut Vec<StyledLine>,
    current_spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
) {
    wrapped.push(Line::from(std::mem::take(current_spans)));
    *current_width = 0;
}

pub(crate) const TRANSCRIPT_RIGHT_PADDING: usize = 2;

pub(crate) fn transcript_content_width(width: usize) -> usize {
    width.saturating_sub(TRANSCRIPT_RIGHT_PADDING).max(1)
}

pub(crate) fn transcript_layout_constraint(
    content_height: u16,
    transcript_budget: u16,
) -> Constraint {
    if transcript_budget == 0 {
        Constraint::Min(1)
    } else if content_height <= transcript_budget {
        Constraint::Length(content_height)
    } else {
        Constraint::Length(transcript_budget)
    }
}

pub(crate) fn wrap_styled_line(line: &StyledLine, width: usize) -> Vec<StyledLine> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for span in &line.spans {
        let style = span.style;
        for token in styled_wrap_tokens(span.content.as_ref()) {
            if token.is_whitespace {
                if current_width == 0 && !wrapped.is_empty() {
                    continue;
                }
                if current_width + token.width <= width {
                    push_styled_str(&mut current_spans, token.text, style);
                    current_width += token.width;
                } else {
                    push_wrapped_styled_line(&mut wrapped, &mut current_spans, &mut current_width);
                }
                continue;
            }

            if current_width > 0 && current_width + token.width > width {
                push_wrapped_styled_line(&mut wrapped, &mut current_spans, &mut current_width);
            }

            if token.width <= width {
                push_styled_str(&mut current_spans, token.text, style);
                current_width += token.width;
                continue;
            }

            for ch in token.text.chars() {
                let ch_width = display_width(ch);
                if current_width > 0 && current_width + ch_width > width {
                    push_wrapped_styled_line(&mut wrapped, &mut current_spans, &mut current_width);
                }
                push_styled_char(&mut current_spans, ch, style);
                current_width += ch_width;
            }
        }
    }

    if current_spans.is_empty() {
        vec![Line::default()]
    } else {
        push_wrapped_styled_line(&mut wrapped, &mut current_spans, &mut current_width);
        wrapped
    }
}

pub(crate) fn render_prefixed_wrapped_spans(
    spans: Vec<Span<'static>>,
    first_prefix: &str,
    continuation_prefix: &str,
    prefix_style: Style,
    width: usize,
) -> Vec<StyledLine> {
    let prefix_width = display_width_str(first_prefix).max(display_width_str(continuation_prefix));
    let content_width = transcript_content_width(width)
        .saturating_sub(prefix_width)
        .max(1);
    let wrapped = wrap_styled_line(&Line::from(spans), content_width);
    if wrapped.is_empty() {
        return vec![Line::from(vec![Span::styled(
            first_prefix.to_string(),
            prefix_style,
        )])];
    }

    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 {
                first_prefix
            } else {
                continuation_prefix
            };
            let mut spans = vec![Span::styled(prefix.to_string(), prefix_style)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

pub(crate) fn tool_body_prefix(index: usize, _total: usize) -> &'static str {
    if index == 0 { "  └ " } else { "    " }
}

pub(crate) fn tool_body_tree_prefix(index: usize, total: usize) -> &'static str {
    if index + 1 == total {
        "  └ "
    } else {
        "  │ "
    }
}

struct StyledWrapToken<'a> {
    text: &'a str,
    width: usize,
    is_whitespace: bool,
}

fn styled_wrap_tokens(text: &str) -> Vec<StyledWrapToken<'_>> {
    let mut tokens = Vec::new();
    let mut token_start = 0;
    let mut token_is_whitespace: Option<bool> = None;

    for (index, ch) in text.char_indices() {
        let is_whitespace = ch.is_whitespace();
        match token_is_whitespace {
            Some(current) if current != is_whitespace => {
                let token_text = &text[token_start..index];
                tokens.push(StyledWrapToken {
                    text: token_text,
                    width: display_width_str(token_text),
                    is_whitespace: current,
                });
                token_start = index;
                token_is_whitespace = Some(is_whitespace);
            }
            None => token_is_whitespace = Some(is_whitespace),
            _ => {}
        }
    }

    if let Some(is_whitespace) = token_is_whitespace {
        let token_text = &text[token_start..];
        tokens.push(StyledWrapToken {
            text: token_text,
            width: display_width_str(token_text),
            is_whitespace,
        });
    }

    tokens
}

fn push_styled_str(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    for ch in text.chars() {
        push_styled_char(spans, ch, style);
    }
}

fn push_wrapped_styled_line(
    wrapped: &mut Vec<StyledLine>,
    current_spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
) {
    trim_trailing_styled_whitespace(current_spans, current_width);
    if current_spans.is_empty() {
        wrapped.push(Line::default());
    } else {
        wrapped.push(Line::from(std::mem::take(current_spans)));
    }
    *current_width = 0;
}

fn trim_trailing_styled_whitespace(spans: &mut Vec<Span<'static>>, width: &mut usize) {
    while let Some(last) = spans.last_mut() {
        let mut chars = last.content.chars().collect::<Vec<_>>();
        let mut removed = false;
        while let Some(ch) = chars.last().copied() {
            if !ch.is_whitespace() {
                break;
            }
            chars.pop();
            *width = width.saturating_sub(display_width(ch));
            removed = true;
        }

        if chars.is_empty() {
            spans.pop();
        } else {
            if removed {
                last.content = chars.into_iter().collect::<String>().into();
            }
            break;
        }
    }
}

pub(crate) fn push_styled_char(spans: &mut Vec<Span<'static>>, ch: char, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content = format!("{}{}", last.content, ch).into();
        return;
    }

    spans.push(Span::styled(ch.to_string(), style));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(line: &StyledLine) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn wrap_stability_earlier_lines_unchanged_as_text_grows() {
        let width = 40;
        let base =
            "The quick brown fox jumps over the lazy dog and then runs across the wide open field";
        let wrapped_base = wrap_styled_line(&Line::from(base), width);
        assert!(
            wrapped_base.len() >= 3,
            "test setup: paragraph should wrap into >=3 lines"
        );

        let extended = format!(
            "{base} while the cat watches from the fence and the birds sing in the morning light"
        );
        let ext_line = Line::from(vec![Span::raw(extended)]);
        let wrapped_ext = wrap_styled_line(&ext_line, width);
        assert!(wrapped_ext.len() > wrapped_base.len());

        let stable_count = wrapped_base.len() - 1;
        for i in 0..stable_count {
            assert_eq!(
                plain(&wrapped_base[i]),
                plain(&wrapped_ext[i]),
                "visual line {i} changed after appending text"
            );
        }
    }

    #[test]
    fn wrap_stability_incremental_token_append() {
        let width = 30;
        let tokens = [
            "Hello ",
            "world, ",
            "this ",
            "is ",
            "a ",
            "streaming ",
            "response ",
            "that ",
            "grows ",
            "token ",
            "by ",
            "token ",
            "to ",
            "verify ",
            "wrap ",
            "stability.",
        ];

        let mut text = String::new();
        let mut prev_lines: Vec<String> = Vec::new();
        for token in &tokens {
            text.push_str(token);
            let line = Line::from(vec![Span::raw(text.clone())]);
            let wrapped = wrap_styled_line(&line, width);
            let current_lines: Vec<String> = wrapped.iter().map(plain).collect();

            let check_count = prev_lines.len().saturating_sub(1);
            for i in 0..check_count {
                assert_eq!(
                    prev_lines[i], current_lines[i],
                    "line {i} shifted after appending '{token}'"
                );
            }
            prev_lines = current_lines;
        }
        assert!(prev_lines.len() >= 2, "test should produce multiple lines");
    }

    #[test]
    fn scroll_anchoring_preserves_position_when_content_grows() {
        use crate::history_cell::viewport::visible_transcript_lines;

        let width = 60;
        let height = 10;
        let base_lines: Vec<StyledLine> = (0..30)
            .map(|i| Line::from(format!("committed line {i:02}")))
            .collect();
        let base_view = visible_transcript_lines(&base_lines, width, height, 15);
        assert_eq!(base_view.actual_scroll, 15);
        let anchored_row_start = base_view.visible_row_start;
        let anchored_first_text = plain(&base_view.visible_lines[0]);

        let mut extended: Vec<StyledLine> = base_lines.clone();
        extended.extend((30..40).map(|i| Line::from(format!("new streaming line {i:02}"))));
        let ext_view = visible_transcript_lines(&extended, width, height, 15 + 10);
        assert_eq!(ext_view.actual_scroll, 25);
        assert_eq!(ext_view.visible_row_start, anchored_row_start);
        assert_eq!(plain(&ext_view.visible_lines[0]), anchored_first_text);
    }
}
