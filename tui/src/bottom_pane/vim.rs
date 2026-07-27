use crossterm::event::KeyCode;

use crate::bottom_pane::input_layout::{
    build_input_layout, input_cursor_for_column, input_inner_width,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FindKind {
    Forward,
    Backward,
    TillForward,
    TillBackward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperatorKind {
    Delete,
    Change,
    Yank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextObjectScope {
    Inner,
    Around,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndentDirection {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenLineDirection {
    Above,
    Below,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextObjectKind {
    Word,
    BigWord,
    DoubleQuote,
    SingleQuote,
    Backtick,
    Paren,
    Bracket,
    Brace,
    Angle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionKind {
    Left,
    Right,
    Down,
    Up,
    VisualDown,
    VisualUp,
    Word,
    BackWord,
    EndWord,
    BigWord,
    BackBigWord,
    EndBigWord,
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastLine,
    FirstLine,
    PrevWordEnd,
    PrevBigWordEnd,
    MatchPair,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RecordedChange {
    Insert(String),
    CompositeInsert {
        base: Box<RecordedChange>,
        text: String,
    },
    X {
        count: usize,
    },
    Replace {
        character: char,
        count: usize,
    },
    ToggleCase {
        count: usize,
    },
    Indent {
        direction: IndentDirection,
        count: usize,
    },
    Join {
        count: usize,
    },
    OpenLine {
        direction: OpenLineDirection,
    },
    Paste {
        after: bool,
        count: usize,
    },
    LineOp {
        op: OperatorKind,
        count: usize,
    },
    OperatorMotion {
        op: OperatorKind,
        motion: MotionKind,
        count: usize,
    },
    OperatorFind {
        op: OperatorKind,
        kind: FindKind,
        target: char,
        count: usize,
    },
    OperatorTextObject {
        op: OperatorKind,
        scope: TextObjectScope,
        kind: TextObjectKind,
        count: usize,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct UndoState {
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct VimRuntimeState {
    pub(crate) register: String,
    pub(crate) register_is_linewise: bool,
    pub(crate) last_change: Option<RecordedChange>,
    pub(crate) pending_insert_change: Option<RecordedChange>,
    pub(crate) undo_stack: Vec<UndoState>,
    pub(crate) inserted_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LastFind {
    pub(crate) kind: FindKind,
    pub(crate) target: char,
}

pub(crate) fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    // Largest char boundary strictly less than `cursor`. Walking bytes (rather
    // than slicing `text[..cursor]`) tolerates a `cursor` that lands inside a
    // multibyte char — slicing there would panic.
    let cursor = cursor.min(text.len());
    if cursor == 0 {
        return 0;
    }
    let mut index = cursor - 1;
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(crate) fn next_char_boundary(text: &str, cursor: usize) -> usize {
    // Smallest char boundary strictly greater than `cursor`, tolerating a
    // `cursor` inside a multibyte char (slicing `text[cursor..]` would panic).
    if cursor >= text.len() {
        return text.len();
    }
    let mut index = cursor + 1;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

pub(crate) fn char_at(text: &str, offset: usize) -> Option<char> {
    text.get(offset..)?.chars().next()
}

pub(crate) fn is_vim_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub(crate) fn is_vim_blank(ch: char) -> bool {
    ch.is_whitespace()
}

pub(crate) fn same_vim_word_class(left: char, right: char) -> bool {
    if is_vim_blank(left) || is_vim_blank(right) {
        return false;
    }

    is_vim_word_char(left) == is_vim_word_char(right)
}

pub(crate) fn prev_vim_word_start(text: &str, cursor: usize) -> usize {
    if text.is_empty() || cursor == 0 {
        return 0;
    }

    let mut pos = prev_char_boundary(text, cursor);

    while let Some(ch) = char_at(text, pos) {
        if !is_vim_blank(ch) {
            break;
        }
        if pos == 0 {
            return 0;
        }
        pos = prev_char_boundary(text, pos);
    }

    let Some(current) = char_at(text, pos) else {
        return 0;
    };

    while pos > 0 {
        let previous = prev_char_boundary(text, pos);
        let Some(previous_char) = char_at(text, previous) else {
            break;
        };

        if !same_vim_word_class(previous_char, current) {
            break;
        }

        pos = previous;
    }

    pos
}

pub(crate) fn next_vim_word_start(text: &str, cursor: usize) -> usize {
    if text.is_empty() || cursor >= text.len() {
        return text.len();
    }

    let mut pos = cursor;

    if let Some(current) = char_at(text, pos)
        && !is_vim_blank(current)
    {
        let current_is_word = is_vim_word_char(current);

        loop {
            let next = next_char_boundary(text, pos);
            if next >= text.len() {
                return text.len();
            }

            let Some(next_char) = char_at(text, next) else {
                return text.len();
            };

            if is_vim_blank(next_char) || is_vim_word_char(next_char) != current_is_word {
                pos = next;
                break;
            }

            pos = next;
        }
    }

    while pos < text.len() {
        let Some(ch) = char_at(text, pos) else {
            return text.len();
        };

        if !is_vim_blank(ch) {
            return pos;
        }

        let next = next_char_boundary(text, pos);
        if next == pos {
            break;
        }
        pos = next;
    }

    text.len()
}

pub(crate) fn end_vim_word(text: &str, cursor: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    // Cap at the last char's *start* (a valid boundary), not `len - 1`, which may
    // land inside a trailing multibyte char and later be sliced → panic.
    let last_char_start = prev_char_boundary(text, text.len());
    let mut pos = cursor.min(last_char_start);

    while pos < text.len() {
        let Some(ch) = char_at(text, pos) else {
            return pos;
        };

        if !is_vim_blank(ch) {
            break;
        }

        let next = next_char_boundary(text, pos);
        if next >= text.len() {
            return last_char_start;
        }
        pos = next;
    }

    let Some(current) = char_at(text, pos) else {
        return pos;
    };
    let current_is_word = is_vim_word_char(current);

    loop {
        let next = next_char_boundary(text, pos);
        if next >= text.len() {
            return pos;
        }

        let Some(next_char) = char_at(text, next) else {
            return pos;
        };

        if is_vim_blank(next_char) || is_vim_word_char(next_char) != current_is_word {
            return pos;
        }

        pos = next;
    }
}

pub(crate) fn prev_big_word_start(text: &str, cursor: usize) -> usize {
    if text.is_empty() || cursor == 0 {
        return 0;
    }

    let mut pos = prev_char_boundary(text, cursor);

    while let Some(ch) = char_at(text, pos) {
        if !is_vim_blank(ch) {
            break;
        }
        if pos == 0 {
            return 0;
        }
        pos = prev_char_boundary(text, pos);
    }

    while pos > 0 {
        let previous = prev_char_boundary(text, pos);
        let Some(previous_char) = char_at(text, previous) else {
            break;
        };

        if is_vim_blank(previous_char) {
            break;
        }

        pos = previous;
    }

    pos
}

pub(crate) fn next_big_word_start(text: &str, cursor: usize) -> usize {
    if text.is_empty() || cursor >= text.len() {
        return text.len();
    }

    let mut pos = cursor;

    if let Some(current) = char_at(text, pos)
        && !is_vim_blank(current)
    {
        loop {
            let next = next_char_boundary(text, pos);
            if next >= text.len() {
                return text.len();
            }

            let Some(next_char) = char_at(text, next) else {
                return text.len();
            };

            if is_vim_blank(next_char) {
                pos = next;
                break;
            }

            pos = next;
        }
    }

    while pos < text.len() {
        let Some(ch) = char_at(text, pos) else {
            return text.len();
        };

        if !is_vim_blank(ch) {
            return pos;
        }

        let next = next_char_boundary(text, pos);
        if next == pos {
            break;
        }
        pos = next;
    }

    text.len()
}

pub(crate) fn end_big_word(text: &str, cursor: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    // Cap at the last char's *start* (a valid boundary); `len - 1` may land
    // inside a trailing multibyte char and later be sliced → panic.
    let last_char_start = prev_char_boundary(text, text.len());
    let mut pos = cursor.min(last_char_start);

    while pos < text.len() {
        let Some(ch) = char_at(text, pos) else {
            return pos;
        };

        if !is_vim_blank(ch) {
            break;
        }

        let next = next_char_boundary(text, pos);
        if next >= text.len() {
            return last_char_start;
        }
        pos = next;
    }

    loop {
        let next = next_char_boundary(text, pos);
        if next >= text.len() {
            return pos;
        }

        let Some(next_char) = char_at(text, next) else {
            return pos;
        };

        if is_vim_blank(next_char) {
            return pos;
        }

        pos = next;
    }
}

pub(crate) fn start_of_current_vim_word(text: &str, cursor: usize) -> usize {
    let Some(current) = char_at(text, cursor) else {
        return cursor.min(text.len());
    };
    if is_vim_blank(current) {
        return cursor.min(text.len());
    }

    let mut pos = cursor.min(text.len());
    while pos > 0 {
        let previous = prev_char_boundary(text, pos);
        let Some(previous_char) = char_at(text, previous) else {
            break;
        };
        if !same_vim_word_class(previous_char, current) {
            break;
        }
        pos = previous;
    }
    pos
}

pub(crate) fn start_of_current_big_word(text: &str, cursor: usize) -> usize {
    let Some(current) = char_at(text, cursor) else {
        return cursor.min(text.len());
    };
    if is_vim_blank(current) {
        return cursor.min(text.len());
    }

    let mut pos = cursor.min(text.len());
    while pos > 0 {
        let previous = prev_char_boundary(text, pos);
        let Some(previous_char) = char_at(text, previous) else {
            break;
        };
        if is_vim_blank(previous_char) {
            break;
        }
        pos = previous;
    }
    pos
}

pub(crate) fn last_non_blank_before(text: &str, cursor: usize) -> Option<usize> {
    if text.is_empty() || cursor == 0 {
        return None;
    }

    let mut pos = prev_char_boundary(text, cursor.min(text.len()));
    loop {
        let ch = char_at(text, pos)?;
        if !is_vim_blank(ch) {
            return Some(pos);
        }
        if pos == 0 {
            return None;
        }
        pos = prev_char_boundary(text, pos);
    }
}

pub(crate) fn end_prev_vim_word(text: &str, cursor: usize) -> usize {
    if text.is_empty() || cursor == 0 {
        return 0;
    }

    let target = match char_at(text, cursor.min(text.len())) {
        Some(ch) if !is_vim_blank(ch) => {
            let current_start = start_of_current_vim_word(text, cursor.min(text.len()));
            if current_start == 0 {
                return 0;
            }
            last_non_blank_before(text, current_start)
        }
        _ => last_non_blank_before(text, cursor.min(text.len())),
    };

    target.unwrap_or(0)
}

pub(crate) fn end_prev_big_word(text: &str, cursor: usize) -> usize {
    if text.is_empty() || cursor == 0 {
        return 0;
    }

    let target = match char_at(text, cursor.min(text.len())) {
        Some(ch) if !is_vim_blank(ch) => {
            let current_start = start_of_current_big_word(text, cursor.min(text.len()));
            if current_start == 0 {
                return 0;
            }
            last_non_blank_before(text, current_start)
        }
        _ => last_non_blank_before(text, cursor.min(text.len())),
    };

    target.unwrap_or(0)
}

pub(crate) fn current_line_start(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1)
}

pub(crate) fn current_line_end_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index)
}

pub(crate) fn current_line_end_motion(text: &str, cursor: usize) -> usize {
    let end = current_line_end_boundary(text, cursor);
    let start = current_line_start(text, cursor);
    if end == start {
        start
    } else {
        prev_char_boundary(text, end)
    }
}

pub(crate) fn first_non_blank_in_line(text: &str, cursor: usize) -> usize {
    let start = current_line_start(text, cursor);
    let end = current_line_end_boundary(text, cursor);
    let mut pos = start;
    while pos < end {
        let Some(ch) = char_at(text, pos) else {
            break;
        };
        if !ch.is_whitespace() {
            return pos;
        }
        pos = next_char_boundary(text, pos);
    }
    start
}

pub(crate) fn offset_for_line(text: &str, line_index: usize) -> usize {
    if line_index == 0 {
        return 0;
    }

    let mut current_line = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            current_line += 1;
            if current_line == line_index {
                return index + 1;
            }
        }
    }

    text.len()
}

pub(crate) fn line_count(text: &str) -> usize {
    text.chars().filter(|ch| *ch == '\n').count() + 1
}

pub(crate) fn find_character(
    text: &str,
    cursor: usize,
    target: char,
    kind: FindKind,
    count: usize,
) -> Option<usize> {
    match kind {
        FindKind::Forward | FindKind::TillForward => {
            let mut start = next_char_boundary(text, cursor);
            let mut found = None;
            for _step in 0..count {
                found = None;
                for (offset, ch) in text[start..].char_indices() {
                    if ch == target {
                        found = Some(start + offset);
                        break;
                    }
                }
                let index = found?;
                start = next_char_boundary(text, index);
            }
            let index = found?;
            if kind == FindKind::TillForward {
                Some(prev_char_boundary(text, index))
            } else {
                Some(index)
            }
        }
        FindKind::Backward | FindKind::TillBackward => {
            let mut end = cursor.min(text.len());
            let mut found = None;
            for _step in 0..count {
                found = None;
                for (offset, ch) in text[..end].char_indices() {
                    if ch == target {
                        found = Some(offset);
                    }
                }
                let index = found?;
                end = index;
            }
            let index = found?;
            if kind == FindKind::TillBackward {
                Some(next_char_boundary(text, index))
            } else {
                Some(index)
            }
        }
    }
}

pub(crate) fn last_line_start(text: &str) -> usize {
    offset_for_line(text, line_count(text).saturating_sub(1))
}

pub(crate) fn go_to_line_from_command(text: &str, requested_line: usize) -> usize {
    if requested_line == 0 {
        return 0;
    }

    let target = requested_line.min(line_count(text));
    offset_for_line(text, target.saturating_sub(1))
}

pub(crate) fn go_to_last_line(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        last_line_start(text)
    }
}

pub(crate) fn go_to_percent_of_buffer(text: &str, percent: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    let percent = percent.clamp(1, 100);
    let total_lines = line_count(text);
    let target_line = if total_lines <= 1 {
        1
    } else {
        ((total_lines - 1) * percent).div_ceil(100) + 1
    };
    go_to_line_from_command(text, target_line)
}

pub(crate) fn matching_delimiter(ch: char) -> Option<(char, char, bool)> {
    match ch {
        '(' => Some(('(', ')', true)),
        ')' => Some(('(', ')', false)),
        '[' => Some(('[', ']', true)),
        ']' => Some(('[', ']', false)),
        '{' => Some(('{', '}', true)),
        '}' => Some(('{', '}', false)),
        '<' => Some(('<', '>', true)),
        '>' => Some(('<', '>', false)),
        _ => None,
    }
}

pub(crate) fn find_matching_delimiter(text: &str, cursor: usize) -> Option<usize> {
    let ch = char_at(text, cursor)?;
    let (open, close, is_open) = matching_delimiter(ch)?;

    if is_open {
        let mut depth = 0usize;
        let mut scan = next_char_boundary(text, cursor);
        while scan < text.len() {
            let current = char_at(text, scan)?;
            if current == open {
                depth += 1;
            } else if current == close {
                if depth == 0 {
                    return Some(scan);
                }
                depth = depth.saturating_sub(1);
            }
            let next = next_char_boundary(text, scan);
            if next == scan {
                break;
            }
            scan = next;
        }
        return None;
    }

    if cursor == 0 {
        return None;
    }

    let mut depth = 0usize;
    let mut scan = prev_char_boundary(text, cursor);
    loop {
        let current = char_at(text, scan)?;
        if current == close {
            depth += 1;
        } else if current == open {
            if depth == 0 {
                return Some(scan);
            }
            depth = depth.saturating_sub(1);
        }

        if scan == 0 {
            break;
        }
        let previous = prev_char_boundary(text, scan);
        if previous == scan {
            break;
        }
        scan = previous;
    }

    None
}

pub(crate) fn operator_key(op: OperatorKind) -> Option<char> {
    match op {
        OperatorKind::Delete => Some('d'),
        OperatorKind::Change => Some('c'),
        OperatorKind::Yank => Some('y'),
    }
}

pub(crate) fn text_object_kind_from_key(code: KeyCode) -> Option<TextObjectKind> {
    match code {
        KeyCode::Char('w') => Some(TextObjectKind::Word),
        KeyCode::Char('W') => Some(TextObjectKind::BigWord),
        KeyCode::Char('"') => Some(TextObjectKind::DoubleQuote),
        KeyCode::Char('\'') => Some(TextObjectKind::SingleQuote),
        KeyCode::Char('`') => Some(TextObjectKind::Backtick),
        KeyCode::Char('(' | ')' | 'b') => Some(TextObjectKind::Paren),
        KeyCode::Char('[' | ']') => Some(TextObjectKind::Bracket),
        KeyCode::Char('{' | '}' | 'B') => Some(TextObjectKind::Brace),
        KeyCode::Char('<' | '>') => Some(TextObjectKind::Angle),
        _ => None,
    }
}

pub(crate) fn motion_is_inclusive(motion: MotionKind) -> bool {
    matches!(
        motion,
        MotionKind::EndWord
            | MotionKind::EndBigWord
            | MotionKind::LineEnd
            | MotionKind::PrevWordEnd
            | MotionKind::PrevBigWordEnd
            | MotionKind::MatchPair
    )
}

pub(crate) fn motion_is_linewise(motion: MotionKind) -> bool {
    matches!(
        motion,
        MotionKind::Down | MotionKind::Up | MotionKind::LastLine | MotionKind::FirstLine
    )
}

pub(crate) fn logical_vertical_offset(text: &str, cursor: usize, direction: isize) -> usize {
    let current_start = current_line_start(text, cursor);
    let current_end = current_line_end_boundary(text, cursor);
    let current_column = cursor.saturating_sub(current_start);

    let (target_start, target_end) = if direction < 0 {
        if current_start == 0 {
            return 0;
        }
        let previous_end = current_start.saturating_sub(1);
        (current_line_start(text, previous_end), previous_end)
    } else {
        if current_end >= text.len() {
            return text.len();
        }
        let next_start = current_end + 1;
        (next_start, current_line_end_boundary(text, next_start))
    };

    let desired = (target_start + current_column).min(target_end);
    snap_to_char_boundary_clamped(text, target_start, target_end, desired)
}

pub(crate) fn visual_vertical_offset(
    text: &str,
    cursor: usize,
    direction: isize,
    width: usize,
) -> usize {
    let layout = build_input_layout(text, cursor, width.max(1));
    if layout.lines.is_empty() {
        return cursor;
    }

    let target_row = if direction < 0 {
        layout.cursor_row.checked_sub(direction.unsigned_abs())
    } else {
        layout
            .cursor_row
            .checked_add(direction as usize)
            .filter(|row| *row < layout.lines.len())
    };

    let Some(target_row) = target_row else {
        return cursor;
    };
    if target_row == layout.cursor_row {
        return cursor;
    }

    input_cursor_for_column(&layout.lines[target_row], layout.cursor_col)
}

pub(crate) fn resolve_motion_offset(
    text: &str,
    cursor: usize,
    motion: MotionKind,
    count: usize,
) -> usize {
    let mut result = cursor;
    match motion {
        MotionKind::LastLine => {
            return if count == 1 {
                go_to_last_line(text)
            } else {
                go_to_line_from_command(text, count)
            };
        }
        MotionKind::FirstLine => return 0,
        _ => {}
    }

    for _ in 0..count {
        let next = match motion {
            MotionKind::Left => prev_char_boundary(text, result),
            MotionKind::Right => next_char_boundary(text, result),
            MotionKind::Down => logical_vertical_offset(text, result, 1),
            MotionKind::Up => logical_vertical_offset(text, result, -1),
            MotionKind::VisualDown => visual_vertical_offset(text, result, 1, input_inner_width()),
            MotionKind::VisualUp => visual_vertical_offset(text, result, -1, input_inner_width()),
            MotionKind::Word => next_vim_word_start(text, result),
            MotionKind::BackWord => prev_vim_word_start(text, result),
            MotionKind::EndWord => end_vim_word(text, result),
            MotionKind::BigWord => next_big_word_start(text, result),
            MotionKind::BackBigWord => prev_big_word_start(text, result),
            MotionKind::EndBigWord => end_big_word(text, result),
            MotionKind::LineStart => current_line_start(text, result),
            MotionKind::FirstNonBlank => first_non_blank_in_line(text, result),
            MotionKind::LineEnd => current_line_end_motion(text, result),
            MotionKind::PrevWordEnd => end_prev_vim_word(text, result),
            MotionKind::PrevBigWordEnd => end_prev_big_word(text, result),
            MotionKind::MatchPair => find_matching_delimiter(text, result).unwrap_or(result),
            MotionKind::LastLine | MotionKind::FirstLine => result,
        };
        if next == result {
            break;
        }
        result = next;
    }

    result
}

pub(crate) fn operator_motion_range(
    text: &str,
    start: usize,
    target: usize,
    motion: MotionKind,
    op: OperatorKind,
    count: usize,
) -> (usize, usize, bool) {
    let mut from = start.min(target);
    let mut to = start.max(target);
    let linewise = motion_is_linewise(motion);

    if op == OperatorKind::Change && matches!(motion, MotionKind::Word | MotionKind::BigWord) {
        let mut word_cursor = start;
        for _ in 0..count.saturating_sub(1) {
            word_cursor = resolve_motion_offset(text, word_cursor, motion, 1);
        }
        let word_end = match motion {
            MotionKind::Word => end_vim_word(text, word_cursor),
            MotionKind::BigWord => end_big_word(text, word_cursor),
            _ => word_cursor,
        };
        to = next_char_boundary(text, word_end);
    } else if linewise {
        from = current_line_start(text, from);
        to = current_line_end_boundary(text, to);
        if to < text.len() {
            to += 1;
        } else if from > 0 && op != OperatorKind::Yank {
            from -= 1;
        }
    } else if motion_is_inclusive(motion) {
        to = next_char_boundary(text, to);
    }

    (from, to.max(from), linewise)
}

pub(crate) fn operator_motion_range_for_absolute_line(
    text: &str,
    start: usize,
    target: usize,
) -> (usize, usize, bool) {
    let mut from = current_line_start(text, start.min(target));
    let mut to = current_line_end_boundary(text, start.max(target));
    if to < text.len() {
        to += 1;
    } else {
        from = from.saturating_sub(1);
    }
    (from, to, true)
}

pub(crate) fn operator_find_range(text: &str, start: usize, target: usize) -> (usize, usize) {
    if target >= start {
        // Forward find (`df`/`dt`): inclusive of the target char.
        (start, next_char_boundary(text, target))
    } else {
        // Backward find (`dF`/`dT`): inclusive of the target char but exclusive
        // of the char under the cursor — backward motions don't consume the
        // origin. Extending to `next_char_boundary(start)` deleted one char too
        // many.
        (target, start)
    }
}

pub(crate) fn snap_out_of_range_start(_text: &str, offset: usize) -> usize {
    offset
}

pub(crate) fn snap_out_of_range_end(_text: &str, offset: usize) -> usize {
    offset
}

pub(crate) fn last_motion_offset(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        prev_char_boundary(text, text.len())
    }
}

pub(crate) fn snap_to_char_boundary_clamped(
    text: &str,
    start: usize,
    end: usize,
    desired: usize,
) -> usize {
    let desired = desired.min(end).max(start);
    if desired == start || desired == end {
        return desired;
    }
    if text.is_char_boundary(desired) {
        desired
    } else {
        prev_char_boundary(text, desired)
    }
}

pub(crate) fn line_index_at_offset(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

pub(crate) fn classify_vim_char(ch: char, big_word: bool) -> u8 {
    if is_vim_blank(ch) {
        0
    } else if big_word || is_vim_word_char(ch) {
        1
    } else {
        2
    }
}

pub(crate) fn find_word_text_object(
    text: &str,
    cursor: usize,
    scope: TextObjectScope,
    big_word: bool,
) -> Option<(usize, usize)> {
    if text.is_empty() {
        return None;
    }

    let mut pos = cursor.min(text.len().saturating_sub(1));
    if char_at(text, pos).is_none() && pos > 0 {
        pos = prev_char_boundary(text, pos);
    }
    let current = char_at(text, pos)?;
    let class = classify_vim_char(current, big_word);

    let mut start = pos;
    let mut end = next_char_boundary(text, pos);

    while start > 0 {
        let previous = prev_char_boundary(text, start);
        let ch = char_at(text, previous)?;
        if classify_vim_char(ch, big_word) != class {
            break;
        }
        start = previous;
    }

    while end < text.len() {
        let ch = char_at(text, end)?;
        if classify_vim_char(ch, big_word) != class {
            break;
        }
        end = next_char_boundary(text, end);
    }

    if scope == TextObjectScope::Around && class != 0 {
        let mut trailing = end;
        while trailing < text.len() {
            let Some(ch) = char_at(text, trailing) else {
                break;
            };
            if !is_vim_blank(ch) {
                break;
            }
            trailing = next_char_boundary(text, trailing);
        }
        if trailing > end {
            end = trailing;
        } else {
            while start > 0 {
                let previous = prev_char_boundary(text, start);
                let Some(ch) = char_at(text, previous) else {
                    break;
                };
                if !is_vim_blank(ch) {
                    break;
                }
                start = previous;
            }
        }
    }

    Some((start, end))
}

pub(crate) fn find_quote_text_object(
    text: &str,
    cursor: usize,
    quote: char,
    scope: TextObjectScope,
) -> Option<(usize, usize)> {
    let line_start = current_line_start(text, cursor);
    let line_end = current_line_end_boundary(text, cursor);
    let line = &text[line_start..line_end];
    let pos_in_line = cursor.saturating_sub(line_start);
    let positions = line
        .char_indices()
        .filter_map(|(index, ch)| (ch == quote).then_some(index))
        .collect::<Vec<_>>();

    let mut iter = positions.chunks_exact(2);
    for pair in &mut iter {
        let start = pair[0];
        let end = pair[1];
        if start <= pos_in_line && pos_in_line <= end {
            return Some(match scope {
                TextObjectScope::Inner => (line_start + start + quote.len_utf8(), line_start + end),
                TextObjectScope::Around => {
                    (line_start + start, line_start + end + quote.len_utf8())
                }
            });
        }
    }
    None
}

pub(crate) fn find_bracket_text_object(
    text: &str,
    cursor: usize,
    open: char,
    close: char,
    scope: TextObjectScope,
) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut start = None;
    let mut scan = cursor.min(text.len());

    loop {
        if scan >= text.len() && scan > 0 {
            scan = prev_char_boundary(text, scan);
        }
        let ch = char_at(text, scan)?;
        if ch == close && scan != cursor {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                start = Some(scan);
                break;
            }
            depth = depth.saturating_sub(1);
        }
        if scan == 0 {
            break;
        }
        scan = prev_char_boundary(text, scan);
    }

    let start = start?;
    depth = 0;
    let mut scan = next_char_boundary(text, start);
    let mut end = None;
    while scan < text.len() {
        let ch = char_at(text, scan)?;
        if ch == open {
            depth += 1;
        } else if ch == close {
            if depth == 0 {
                end = Some(scan);
                break;
            }
            depth = depth.saturating_sub(1);
        }
        scan = next_char_boundary(text, scan);
    }

    let end = end?;
    Some(match scope {
        TextObjectScope::Inner => (next_char_boundary(text, start), end),
        TextObjectScope::Around => (start, next_char_boundary(text, end)),
    })
}

pub(crate) fn find_text_object(
    text: &str,
    cursor: usize,
    kind: TextObjectKind,
    scope: TextObjectScope,
) -> Option<(usize, usize)> {
    match kind {
        TextObjectKind::Word => find_word_text_object(text, cursor, scope, false),
        TextObjectKind::BigWord => find_word_text_object(text, cursor, scope, true),
        TextObjectKind::DoubleQuote => find_quote_text_object(text, cursor, '"', scope),
        TextObjectKind::SingleQuote => find_quote_text_object(text, cursor, '\'', scope),
        TextObjectKind::Backtick => find_quote_text_object(text, cursor, '`', scope),
        TextObjectKind::Paren => find_bracket_text_object(text, cursor, '(', ')', scope),
        TextObjectKind::Bracket => find_bracket_text_object(text, cursor, '[', ']', scope),
        TextObjectKind::Brace => find_bracket_text_object(text, cursor, '{', '}', scope),
        TextObjectKind::Angle => find_bracket_text_object(text, cursor, '<', '>', scope),
    }
}

#[cfg(test)]
mod operator_find_tests {
    use super::operator_find_range;

    #[test]
    fn forward_find_is_inclusive_of_target() {
        // "abcXef", cursor at 0 (a), df to 'X' at index 3 → delete [0, 4).
        let text = "abcXef";
        assert_eq!(operator_find_range(text, 0, 3), (0, 4));
    }

    #[test]
    fn backward_find_excludes_char_under_cursor() {
        // "abXcde", cursor at 4 (d), dF to 'X' at index 2 → delete [2, 4):
        // includes X, excludes the char under the cursor. Previously the range
        // extended to index 5, deleting one extra char.
        let text = "abXcde";
        assert_eq!(operator_find_range(text, 4, 2), (2, 4));
    }
}

#[cfg(test)]
mod multibyte_tests {
    use super::{
        TextObjectScope, end_big_word, end_vim_word, find_word_text_object, next_char_boundary,
        prev_char_boundary,
    };

    #[test]
    fn char_boundary_helpers_tolerate_mid_char_offsets() {
        // "aé": 'a' = byte 0, 'é' = bytes 1..3. Offset 2 is inside 'é'.
        let text = "aé";
        assert_eq!(prev_char_boundary(text, 2), 1);
        assert_eq!(prev_char_boundary(text, text.len()), 1);
        assert_eq!(next_char_boundary(text, 2), text.len());
        assert_eq!(next_char_boundary(text, 1), text.len());
    }

    #[test]
    fn end_word_motions_return_char_boundary_for_trailing_multibyte() {
        // Previously returned `len - 1` (mid-'é'), which the caller sliced → panic.
        let text = "aé";
        let end = end_vim_word(text, 0);
        assert!(
            text.is_char_boundary(end),
            "end_vim_word must be a boundary"
        );
        let end = end_big_word(text, 0);
        assert!(
            text.is_char_boundary(end),
            "end_big_word must be a boundary"
        );
        // No panic when starting at/after the trailing multibyte char.
        let _ = end_vim_word(text, text.len());
        let _ = end_big_word(text, text.len());
    }

    #[test]
    fn word_text_object_handles_cursor_at_trailing_multibyte() {
        // `diw`/`ciw`/`yiw` at the buffer end with a trailing multibyte char.
        let text = "aé";
        for cursor in [0, 1, 2, text.len()] {
            let range = find_word_text_object(text, cursor, TextObjectScope::Inner, false);
            if let Some((start, end)) = range {
                assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
                let _ = &text[start..end]; // must not panic
            }
        }
    }
}
