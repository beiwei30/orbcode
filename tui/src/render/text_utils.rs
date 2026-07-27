use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

pub(crate) type StyledLine = Line<'static>;

pub(crate) fn display_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
}

pub(crate) fn display_width_str(text: &str) -> usize {
    text.chars().map(display_width).sum()
}

pub(crate) fn fit_table_widths(
    mut widths: Vec<usize>,
    available_width: usize,
) -> Option<Vec<usize>> {
    if widths.is_empty() {
        return Some(widths);
    }

    let min_column_width = 3usize;
    let border_overhead = widths.len().saturating_mul(3).saturating_add(1);
    if available_width <= border_overhead + widths.len().saturating_mul(min_column_width) {
        return None;
    }

    let max_content_width = available_width.saturating_sub(border_overhead);
    while widths.iter().sum::<usize>() > max_content_width {
        let Some((index, width)) = widths.iter().enumerate().max_by_key(|(_, width)| **width)
        else {
            break;
        };
        if *width <= min_column_width {
            return None;
        }
        widths[index] = widths[index].saturating_sub(1);
    }
    Some(widths)
}

pub(crate) fn styled_line_display_width(line: &StyledLine) -> usize {
    line.spans
        .iter()
        .map(|span| display_width_str(span.content.as_ref()))
        .sum()
}

pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let total = input.chars().count();
    for ch in input.chars().take(max_chars) {
        output.push(ch);
    }
    if total > max_chars && max_chars > 1 {
        output.pop();
        output.push('…');
    }
    output
}

pub(crate) fn truncate_display_width(input: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut output = String::new();
    let mut width = 0usize;
    let mut truncated = false;
    for ch in input.chars() {
        let ch_width = display_width(ch);
        if width + ch_width > max_width {
            truncated = true;
            break;
        }
        width += ch_width;
        output.push(ch);
    }

    if truncated && max_width > 0 {
        while display_width_str(&output) + display_width('…') > max_width {
            if output.pop().is_none() {
                break;
            }
        }
        output.push('…');
    }

    output
}

pub(crate) fn pad_or_truncate(input: &str, width: usize) -> String {
    let truncated = truncate_chars(input, width);
    let padding = width.saturating_sub(display_width_str(&truncated));
    format!("{truncated}{}", " ".repeat(padding))
}

pub(crate) fn truncate_path_tail(input: &str, max_chars: usize) -> String {
    let total = input.chars().count();
    if total <= max_chars {
        return input.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }

    let tail = input
        .chars()
        .rev()
        .take(max_chars - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("…{tail}")
}

pub(crate) fn format_duration_short(duration_ms: u64) -> String {
    let secs = ((duration_ms as f64) / 1000.0).round() as u64;
    if secs < 60 {
        format!("{secs}s")
    } else {
        let minutes = secs / 60;
        let seconds = secs % 60;
        format!("{minutes}m {seconds}s")
    }
}

pub(crate) fn collapse_inline_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn push_unique_line(target: &mut Vec<String>, value: String) {
    if !value.trim().is_empty() && !target.iter().any(|existing| existing == &value) {
        target.push(value);
    }
}

pub(crate) fn is_blank_line(line: &StyledLine) -> bool {
    line.spans.is_empty() || line.spans.iter().all(|span| span.content.trim().is_empty())
}

pub(crate) fn push_blank_line_if_needed(lines: &mut Vec<StyledLine>) {
    if !lines.is_empty() && !lines.last().is_some_and(is_blank_line) {
        lines.push(StyledLine::default());
    }
}

pub(crate) fn compact_blank_lines(lines: Vec<StyledLine>) -> Vec<StyledLine> {
    let mut compacted = Vec::with_capacity(lines.len());
    let mut previous_blank = true;

    for line in lines {
        let blank = is_blank_line(&line);
        if blank {
            if previous_blank {
                continue;
            }
            previous_blank = true;
            compacted.push(StyledLine::default());
        } else {
            previous_blank = false;
            compacted.push(line);
        }
    }

    while compacted.last().is_some_and(is_blank_line) {
        compacted.pop();
    }

    compacted
}
