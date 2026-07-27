use ratatui::{
    prelude::Style,
    text::{Line, Span},
};

use crate::render::styled_wrap::wrap_styled_line;
use crate::render::text_utils::{
    StyledLine, compact_blank_lines, display_width_str, fit_table_widths, styled_line_display_width,
};
use crate::tui_theme::{
    code_block_style, heading_style, inline_markdown_style, list_marker_style, quote_style,
    subtle_style,
};

pub(crate) fn render_markdown_body_lines(
    text: &str,
    base_style: Style,
    available_width: usize,
) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut index = 0;

    while index < raw_lines.len() {
        let raw_line = raw_lines[index];
        let trimmed = raw_line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            index += 1;
            continue;
        }

        if !in_code_block
            && let Some((table_lines, next_index)) =
                render_markdown_table_block(&raw_lines, index, base_style, available_width)
        {
            lines.extend(table_lines);
            index = next_index;
            continue;
        }

        if trimmed.is_empty() {
            lines.push(Line::default());
            index += 1;
            continue;
        }

        let content_spans = if in_code_block {
            vec![Span::styled(raw_line.to_string(), code_block_style())]
        } else {
            render_markdown_line_spans(raw_line, base_style)
        };

        lines.extend(wrap_styled_line(
            &Line::from(content_spans),
            available_width.max(1),
        ));
        index += 1;
    }

    compact_blank_lines(lines)
}

pub(crate) fn wrap_inline_markdown_line(text: &str, style: Style, width: usize) -> Vec<StyledLine> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![Line::default()];
    }

    let line = Line::from(render_inline_markdown_spans(text, style));
    wrap_styled_line(&line, width)
}

fn render_markdown_line_spans(line: &str, base_style: Style) -> Vec<Span<'static>> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len().saturating_sub(trimmed.len())];
    let mut spans = Vec::new();

    if !indent.is_empty() {
        spans.push(Span::styled(indent.to_string(), subtle_style()));
    }

    if let Some((level, rest)) = parse_heading_line(trimmed) {
        spans.extend(render_inline_markdown_spans(
            rest,
            heading_style(level).patch(base_style),
        ));
        return spans;
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        spans.push(Span::styled("> ", quote_style()));
        spans.extend(render_inline_markdown_spans(rest, quote_style()));
        return spans;
    }

    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        spans.push(Span::styled("• ", list_marker_style()));
        spans.extend(render_inline_markdown_spans(rest, base_style));
        return spans;
    }

    if let Some((marker, rest)) = parse_ordered_list_line(trimmed) {
        spans.push(Span::styled(marker, list_marker_style()));
        spans.extend(render_inline_markdown_spans(rest, base_style));
        return spans;
    }

    spans.extend(render_inline_markdown_spans(trimmed, base_style));
    spans
}

fn render_inline_markdown_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chars = text.chars().peekable();
    let mut buffer = String::new();
    let mut bold = false;
    let mut code = false;

    while let Some(ch) = chars.next() {
        if ch == '*' && chars.peek() == Some(&'*') {
            chars.next();
            if !buffer.is_empty() {
                spans.push(Span::styled(
                    buffer.clone(),
                    inline_markdown_style(base_style, bold, code),
                ));
                buffer.clear();
            }
            bold = !bold;
            continue;
        }

        if ch == '`' {
            if !buffer.is_empty() {
                spans.push(Span::styled(
                    buffer.clone(),
                    inline_markdown_style(base_style, bold, code),
                ));
                buffer.clear();
            }
            code = !code;
            continue;
        }

        buffer.push(ch);
    }

    if !buffer.is_empty() {
        spans.push(Span::styled(
            buffer,
            inline_markdown_style(base_style, bold, code),
        ));
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }

    spans
}

pub(crate) fn parse_heading_line(line: &str) -> Option<(usize, &str)> {
    let mut level = 0usize;
    for ch in line.chars() {
        if ch == '#' {
            level += 1;
        } else {
            break;
        }
    }
    if (1..=6).contains(&level) {
        let rest = line[level..].trim_start();
        if !rest.is_empty() {
            return Some((level, rest));
        }
    }
    None
}

pub(crate) fn parse_ordered_list_line(line: &str) -> Option<(String, &str)> {
    let marker_len = line.chars().take_while(char::is_ascii_digit).count();
    if marker_len == 0 {
        return None;
    }
    let rest = &line[marker_len..];
    let rest = rest.strip_prefix(". ")?;
    Some((format!("{} ", &line[..marker_len + 1]), rest))
}

fn render_markdown_table_block(
    lines: &[&str],
    start: usize,
    base_style: Style,
    available_width: usize,
) -> Option<(Vec<StyledLine>, usize)> {
    let header = split_markdown_table_row(lines.get(start)?)?;
    if !is_markdown_table_separator(lines.get(start + 1)?) {
        return None;
    }

    let mut rows = vec![header];
    let mut next_index = start + 2;
    while let Some(raw_line) = lines.get(next_index) {
        if let Some(row) = split_markdown_table_row(raw_line) {
            rows.push(row);
            next_index += 1;
        } else {
            break;
        }
    }
    if rows.len() < 2 {
        return None;
    }

    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let normalized_rows = rows
        .into_iter()
        .map(|mut row| {
            row.resize(column_count, String::new());
            row
        })
        .collect::<Vec<_>>();
    let mut widths = vec![0usize; column_count];
    for row in &normalized_rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width_str(cell));
        }
    }

    let widths = fit_table_widths(widths, available_width)?;
    let mut rendered = Vec::new();
    rendered.push(render_table_border("┌", "┬", "┐", &widths));
    rendered.extend(render_table_row(
        &normalized_rows[0],
        &widths,
        heading_style(3).patch(base_style),
    ));
    rendered.push(render_table_border("├", "┼", "┤", &widths));
    for row in normalized_rows.iter().skip(1) {
        rendered.extend(render_table_row(row, &widths, base_style));
    }
    rendered.push(render_table_border("└", "┴", "┘", &widths));
    Some((rendered, next_index))
}

pub(crate) fn split_markdown_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    let row = trimmed.trim_matches('|').trim();
    if row.is_empty() {
        return None;
    }
    Some(row.split('|').map(|cell| cell.trim().to_string()).collect())
}

pub(crate) fn is_markdown_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return false;
    }
    let row = trimmed.trim_matches('|').trim();
    if row.is_empty() {
        return false;
    }
    row.split('|').all(|cell| {
        let cell = cell.trim();
        !cell.is_empty()
            && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
            && cell.contains('-')
    })
}

fn render_table_border(left: &str, middle: &str, right: &str, widths: &[usize]) -> StyledLine {
    let mut spans = vec![Span::styled(left.to_string(), subtle_style())];
    for (index, width) in widths.iter().enumerate() {
        spans.push(Span::styled(
            "─".repeat(width.saturating_add(2)),
            subtle_style(),
        ));
        spans.push(Span::styled(
            if index + 1 == widths.len() {
                right.to_string()
            } else {
                middle.to_string()
            },
            subtle_style(),
        ));
    }
    Line::from(spans)
}

fn render_table_row(cells: &[String], widths: &[usize], style: Style) -> Vec<StyledLine> {
    let wrapped_cells = cells
        .iter()
        .enumerate()
        .map(|(index, cell)| wrap_inline_markdown_line(cell, style, widths[index]))
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut lines = Vec::with_capacity(row_height);

    for line_index in 0..row_height {
        let mut spans = vec![Span::styled("│", subtle_style())];
        for (column_index, width) in widths.iter().enumerate() {
            let content = wrapped_cells[column_index]
                .get(line_index)
                .cloned()
                .unwrap_or_default();
            let content_width = styled_line_display_width(&content);
            spans.push(Span::raw(" "));
            spans.extend(content.spans);
            spans.push(Span::raw(
                " ".repeat(width.saturating_sub(content_width) + 1),
            ));
            spans.push(Span::styled("│", subtle_style()));
        }
        lines.push(Line::from(spans));
    }

    lines
}
