use crate::render::text_utils::display_width;

pub(crate) struct InputView {
    pub(crate) lines: Vec<String>,
    pub(crate) line_layouts: Vec<InputLineLayout>,
    pub(crate) width: usize,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct InputLineLayout {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
}

pub(crate) struct InputLayout {
    pub(crate) lines: Vec<InputLineLayout>,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
}

pub(crate) fn input_inner_width() -> usize {
    crossterm::terminal::size().map_or(80, |(width, _)| width.saturating_sub(3).max(1) as usize)
}

pub(crate) fn max_input_inner_height(area_height: u16, request_status_height: u16) -> usize {
    area_height
        .saturating_sub(request_status_height)
        .saturating_sub(4)
        .max(1) as usize
}

pub(crate) fn normalize_paste_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) fn prompt_input_submission_line(input: &str) -> Option<String> {
    if input.trim().is_empty() {
        return None;
    }
    if input.trim_start().starts_with('/') {
        return Some(input.trim().to_string());
    }
    Some(input.to_string())
}

pub(crate) fn build_input_layout(input: &str, cursor: usize, width: usize) -> InputLayout {
    let width = width.max(1);
    let mut lines = vec![InputLineLayout {
        start: 0,
        end: 0,
        text: String::new(),
    }];
    let mut cursor_row = 0;
    let mut cursor_col = 0;
    let mut current_width = 0;

    for (index, ch) in input.char_indices() {
        let ch_width = display_width(ch);
        if ch != '\n' && current_width > 0 && current_width + ch_width > width {
            lines.push(InputLineLayout {
                start: index,
                end: index,
                text: String::new(),
            });
            current_width = 0;
        }

        if index == cursor {
            cursor_row = lines.len() - 1;
            cursor_col = current_width;
        }

        if ch == '\n' {
            if let Some(last) = lines.last_mut() {
                last.end = index;
            }
            lines.push(InputLineLayout {
                start: index + ch.len_utf8(),
                end: index + ch.len_utf8(),
                text: String::new(),
            });
            current_width = 0;
            continue;
        }

        if let Some(last) = lines.last_mut() {
            last.text.push(ch);
            last.end = index + ch.len_utf8();
        }
        current_width += ch_width;
    }

    if cursor == input.len() {
        cursor_row = lines.len() - 1;
        cursor_col = current_width;
    }

    InputLayout {
        lines,
        cursor_row,
        cursor_col,
    }
}

pub(crate) fn build_input_view(
    input: &str,
    cursor: usize,
    width: usize,
    max_visible_lines: usize,
) -> InputView {
    build_input_view_with_tail_pin(input, cursor, width, max_visible_lines, false)
}

pub(crate) fn build_input_view_with_tail_pin(
    input: &str,
    cursor: usize,
    width: usize,
    max_visible_lines: usize,
    tail_pinned: bool,
) -> InputView {
    let layout = build_input_layout(input, cursor, width);
    let lines = layout
        .lines
        .iter()
        .map(|line| line.text.clone())
        .collect::<Vec<_>>();
    let cursor_row = layout.cursor_row;
    let cursor_col = layout.cursor_col;

    let visible_count = lines.len().clamp(1, max_visible_lines.max(1));
    let max_start = lines.len().saturating_sub(visible_count);
    let cursor_on_trailing_empty_row = cursor_row == lines.len().saturating_sub(1)
        && lines.get(cursor_row).is_some_and(String::is_empty);
    let last_content_row = lines.iter().rposition(|line| !line.is_empty()).unwrap_or(0);
    let tail_row = if cursor_on_trailing_empty_row {
        cursor_row
    } else {
        last_content_row
    };
    let tail_start = tail_row
        .saturating_add(1)
        .saturating_sub(visible_count)
        .min(max_start);
    let start = if tail_pinned || cursor_row >= tail_start.saturating_sub(1) {
        tail_start
    } else {
        cursor_row.saturating_sub(visible_count / 2).min(max_start)
    };
    let end = (start + visible_count).min(lines.len());

    InputView {
        lines: lines[start..end].to_vec(),
        line_layouts: layout.lines[start..end].to_vec(),
        width,
        cursor_row: cursor_row
            .saturating_sub(start)
            .min(end.saturating_sub(start).saturating_sub(1)),
        cursor_col,
    }
}

pub(crate) fn input_cursor_for_column(line: &InputLineLayout, target_col: usize) -> usize {
    line.start + input_byte_offset_for_display_column(&line.text, target_col)
}

pub(crate) fn input_display_column(input: &str, line_start: usize, cursor: usize) -> usize {
    input[line_start..cursor].chars().map(display_width).sum()
}

pub(crate) fn input_cursor_for_display_column(
    input: &str,
    line_start: usize,
    line_end: usize,
    target_col: usize,
) -> usize {
    line_start + input_byte_offset_for_display_column(&input[line_start..line_end], target_col)
}

fn input_byte_offset_for_display_column(text: &str, target_col: usize) -> usize {
    let mut col = 0;
    for (offset, ch) in text.char_indices() {
        if target_col <= col {
            return offset;
        }
        let next_col = col + display_width(ch);
        if target_col < next_col {
            return offset;
        }
        col = next_col;
    }
    text.len()
}
