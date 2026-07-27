use crate::bottom_pane::input_layout::normalize_paste_text;
use crate::bottom_pane::vim::{
    char_at, current_line_end_boundary, current_line_start, is_vim_blank, is_vim_word_char,
    line_index_at_offset, next_char_boundary, offset_for_line, prev_char_boundary,
};
use crate::editor_mode::EditorMode;
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn insert_char(&mut self, character: char) {
        let mut encoded = [0_u8; 4];
        self.insert_text(character.encode_utf8(&mut encoded));
    }

    pub(crate) fn insert_text(&mut self, text: &str) {
        let was_at_end = self.input_cursor == self.input.len();
        self.input.insert_str(self.input_cursor, text);
        self.input_cursor += text.len();
        self.input_tail_pinned = was_at_end && self.input.contains('\n');
        self.desired_column = None;
        self.slash_command_selected = 0;
        if self.editor_mode == EditorMode::Insert {
            self.vim_state.inserted_text.push_str(text);
        }
    }

    pub(crate) fn insert_paste_text(&mut self, text: &str) {
        let text = normalize_paste_text(text);
        self.insert_text(&text);
        self.input_tail_pinned = text.contains('\n');
    }

    pub(crate) fn delete_prev_char(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        if self.input_cursor == 0 {
            return;
        }
        let start = prev_char_boundary(&self.input, self.input_cursor);
        self.input.replace_range(start..self.input_cursor, "");
        self.input_cursor = start;
        self.slash_command_selected = 0;
        if self.editor_mode == EditorMode::Insert && !self.vim_state.inserted_text.is_empty() {
            self.vim_state.inserted_text.pop();
        }
    }

    pub(crate) fn delete_next_char(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        if self.input_cursor >= self.input.len() {
            return;
        }
        let end = next_char_boundary(&self.input, self.input_cursor);
        self.input.replace_range(self.input_cursor..end, "");
        self.slash_command_selected = 0;
    }

    pub(crate) fn move_cursor_left(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        self.input_cursor = prev_char_boundary(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_right(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        self.input_cursor = next_char_boundary(&self.input, self.input_cursor);
    }

    /// True when everything between the current line's start and the cursor is
    /// whitespace — i.e. the caret sits in a leading-indent position where Tab
    /// should insert spaces rather than being repurposed (e.g. to queue a
    /// streaming follow-up).
    pub(crate) fn cursor_in_line_indent(&self) -> bool {
        let cursor = self.input_cursor.min(self.input.len());
        let start = current_line_start(&self.input, cursor);
        self.input[start..cursor]
            .chars()
            .all(|character| character == ' ' || character == '\t')
    }

    pub(crate) fn move_cursor_to_current_line_start(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        self.input_cursor = current_line_start(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_to_current_line_end(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        self.input_cursor = current_line_end_boundary(&self.input, self.input_cursor);
    }

    pub(crate) fn kill_to_line_end(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        let end = current_line_end_boundary(&self.input, self.input_cursor);
        if end > self.input_cursor {
            self.input.replace_range(self.input_cursor..end, "");
        }
    }

    pub(crate) fn kill_to_line_start(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        let start = current_line_start(&self.input, self.input_cursor);
        if start < self.input_cursor {
            self.input.replace_range(start..self.input_cursor, "");
            self.input_cursor = start;
        }
    }

    pub(crate) fn indent_selected_lines(&mut self) {
        let Some(sel) = self.input_selection.take() else {
            return;
        };
        let (start, end) = sel.normalized_range();
        let first_line = line_index_at_offset(&self.input, start);
        let last_line = line_index_at_offset(&self.input, end.saturating_sub(1).max(start));

        let mut lines: Vec<String> = self.input.split('\n').map(ToOwned::to_owned).collect();
        let end_line = last_line.min(lines.len() - 1);
        for line in &mut lines[first_line..=end_line] {
            line.insert_str(0, "    ");
        }
        self.input = lines.join("\n");
        let line_count = lines.len();
        let new_start = offset_for_line(&self.input, first_line);
        let new_end = if last_line + 1 < line_count {
            offset_for_line(&self.input, last_line + 1)
        } else {
            self.input.len()
        };
        self.input_cursor = new_start;
        self.input_selection = Some(crate::prompt_state::InputSelectionState {
            anchor: new_start,
            focus: new_end,
        });
        self.desired_column = None;
    }

    pub(crate) fn dedent_selected_lines(&mut self) {
        let Some(sel) = self.input_selection.take() else {
            return;
        };
        let (start, end) = sel.normalized_range();
        let first_line = line_index_at_offset(&self.input, start);
        let last_line = line_index_at_offset(&self.input, end.saturating_sub(1).max(start));

        let mut lines: Vec<String> = self.input.split('\n').map(ToOwned::to_owned).collect();
        let end_line = last_line.min(lines.len() - 1);
        for line in &mut lines[first_line..=end_line] {
            let spaces = line.bytes().take(4).take_while(|&b| b == b' ').count();
            if spaces > 0 {
                line.drain(..spaces);
            }
        }
        self.input = lines.join("\n");
        let line_count = lines.len();
        let new_start = offset_for_line(&self.input, first_line);
        let new_end = if last_line + 1 < line_count {
            offset_for_line(&self.input, last_line + 1)
        } else {
            self.input.len()
        };
        self.input_cursor = new_start;
        self.input_selection = Some(crate::prompt_state::InputSelectionState {
            anchor: new_start,
            focus: new_end,
        });
        self.desired_column = None;
    }

    pub(crate) fn delete_current_line(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        let start = current_line_start(&self.input, self.input_cursor);
        let end = current_line_end_boundary(&self.input, self.input_cursor);

        if start == 0 && end == self.input.len() {
            self.input.clear();
            self.input_cursor = 0;
        } else if end < self.input.len() {
            self.input.replace_range(start..end + 1, "");
            self.input_cursor = start.min(self.input.len());
        } else {
            self.input.replace_range(start.saturating_sub(1)..end, "");
            self.input_cursor = self.input.len().min(start.saturating_sub(1));
        }
    }

    pub(crate) fn delete_prev_word(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        if self.input_cursor == 0 {
            return;
        }
        let target = readline_prev_word_boundary(&self.input, self.input_cursor);
        self.input.replace_range(target..self.input_cursor, "");
        self.input_cursor = target;
    }

    pub(crate) fn move_cursor_word_left(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        self.input_cursor = readline_prev_word_boundary(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_word_right(&mut self) {
        self.input_tail_pinned = false;
        self.desired_column = None;
        self.input_cursor = readline_next_word_boundary(&self.input, self.input_cursor);
    }
}

fn readline_prev_word_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut pos = prev_char_boundary(text, cursor);
    while pos > 0 {
        if let Some(ch) = char_at(text, pos)
            && !is_vim_blank(ch)
            && ch != '\n'
        {
            break;
        }
        pos = prev_char_boundary(text, pos);
    }
    while pos > 0 {
        let Some(current) = char_at(text, pos) else {
            break;
        };
        let prev = prev_char_boundary(text, pos);
        match char_at(text, prev) {
            Some(ch) if is_vim_word_char(ch) && is_vim_word_char(current) => {
                pos = prev;
            }
            Some(ch)
                if !is_vim_word_char(ch)
                    && !is_vim_blank(ch)
                    && ch != '\n'
                    && !is_vim_word_char(current)
                    && !is_vim_blank(current) =>
            {
                pos = prev;
            }
            _ => break,
        }
    }
    pos
}

fn readline_next_word_boundary(text: &str, cursor: usize) -> usize {
    let len = text.len();
    if cursor >= len {
        return len;
    }
    let mut pos = cursor;
    if let Some(ch) = char_at(text, pos) {
        if is_vim_word_char(ch) {
            while pos < len {
                let next = next_char_boundary(text, pos);
                match char_at(text, next) {
                    Some(c) if is_vim_word_char(c) => pos = next,
                    _ => {
                        pos = next;
                        break;
                    }
                }
            }
        } else if !is_vim_blank(ch) && ch != '\n' {
            while pos < len {
                let next = next_char_boundary(text, pos);
                match char_at(text, next) {
                    Some(c) if !is_vim_word_char(c) && !is_vim_blank(c) && c != '\n' => {
                        pos = next;
                    }
                    _ => {
                        pos = next;
                        break;
                    }
                }
            }
        } else {
            pos = next_char_boundary(text, pos);
        }
    }
    while pos < len {
        match char_at(text, pos) {
            Some(ch) if is_vim_blank(ch) && ch != '\n' => pos = next_char_boundary(text, pos),
            _ => break,
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    use orbcode_config::{ContextWindowOptions, MaxOutputTokenOptions, TokenWarningOptions};
    use ratatui::prelude::Rect;

    use crate::background_agent_panel::BackgroundAgentPanelState;
    use crate::bottom_pane::slash_suggestions::SlashSuggestionLinesCache;
    use crate::bottom_pane::vim::VimRuntimeState;
    use crate::history_cell::state::TranscriptUiState;
    use crate::state::{RequestTokenDirection, StatusLineState, TuiState};
    use crate::task_panel::TaskPanelState;
    use crate::tool_cell::live_state::LiveToolCells;
    use crate::transcript_task_cards::TranscriptTaskCardsState;

    fn state(input: &str, cursor: usize) -> TuiState {
        TuiState {
            client: None,
            session_id: String::new(),
            cwd: PathBuf::new(),
            messages: Vec::new(),
            transcript_ui: TranscriptUiState::default(),
            input: input.to_string(),
            input_cursor: cursor,
            input_tail_pinned: false,
            input_area: Rect::ZERO,
            input_selection: None,
            desired_column: None,
            prompt_history: Vec::new(),
            prompt_history_index: None,
            slash_command_selected: 0,
            steered_followups: std::collections::VecDeque::new(),
            queued_followups: std::collections::VecDeque::new(),
            pending_assistant: String::new(),
            compact_started_at: None,
            deferred_assistant_message: None,
            active_thinking: None,
            live_tool_cells: LiveToolCells::default(),
            in_progress_tool_use_ids: HashSet::new(),
            pending_hook_progress: Vec::new(),
            hook_progress_by_message_id: HashMap::new(),
            history_flushed_message_count: 0,
            retained_visible_transcript_cells: 0,
            focus_latest_message_start: false,
            pending_history_flush: false,
            overlay: None,
            recent_denied_permissions: Vec::new(),
            status_line: String::new(),
            status_line_set_at: None,
            ui_version: String::new(),
            cwd_display: String::new(),
            model_display_name: String::new(),
            context_window_options: ContextWindowOptions::default(),
            max_output_token_options: MaxOutputTokenOptions::default(),
            token_warning_options: TokenWarningOptions::default(),
            default_provider_label: String::new(),
            show_update_notice: false,
            expanded_tool_details: false,
            request_in_flight: false,
            spinner_frame: 0,
            spinner_verb_index: 0,
            request_count: 0,
            request_started_at: None,
            streamed_response_chars: 0,
            request_token_direction: RequestTokenDirection::Up,
            current_turn_total_tokens: 0,
            last_provider: None,
            last_usage: None,
            editor_mode: EditorMode::Standard,
            normal_pending: None,
            last_find: None,
            normal_count: None,
            vim_state: VimRuntimeState::default(),
            external_editor_request: None,
            slash_suggestion_lines_cache: SlashSuggestionLinesCache::default(),
            mcp_slash_suggestions: Default::default(),
            mcp_slash_suggestion_revision: 0,
            mcp_slash_suggestion_refresh_key: None,
            task_panel: TaskPanelState::new(Some("test-session"), true),
            background_agent_panel: BackgroundAgentPanelState::new(),
            transcript_task_cards: TranscriptTaskCardsState::new(),
            status: StatusLineState::default(),
            statusline_command: None,
            statusline_refresh_interval: std::time::Duration::from_secs(30),
            clear_session_info: None,
        }
    }

    // --- Ctrl+A: move to current line start ---

    #[test]
    fn ctrl_a_single_line() {
        let mut s = state("hello world", 6);
        s.move_cursor_to_current_line_start();
        assert_eq!(s.input_cursor, 0);
    }

    #[test]
    fn ctrl_a_multiline_middle() {
        // "first\nsecond\nthird" — cursor in "second" at byte 8 ('c')
        let mut s = state("first\nsecond\nthird", 8);
        s.move_cursor_to_current_line_start();
        assert_eq!(s.input_cursor, 6); // start of "second"
    }

    #[test]
    fn ctrl_a_at_line_start() {
        let mut s = state("first\nsecond", 6);
        s.move_cursor_to_current_line_start();
        assert_eq!(s.input_cursor, 6); // already at start
    }

    // --- Ctrl+E: move to current line end ---

    #[test]
    fn ctrl_e_single_line() {
        let mut s = state("hello world", 3);
        s.move_cursor_to_current_line_end();
        assert_eq!(s.input_cursor, 11);
    }

    #[test]
    fn ctrl_e_multiline_first_line() {
        let mut s = state("first\nsecond\nthird", 2);
        s.move_cursor_to_current_line_end();
        assert_eq!(s.input_cursor, 5); // end of "first", before '\n'
    }

    #[test]
    fn ctrl_e_multiline_middle() {
        let mut s = state("first\nsecond\nthird", 8);
        s.move_cursor_to_current_line_end();
        assert_eq!(s.input_cursor, 12); // end of "second", before '\n'
    }

    // --- Ctrl+K: kill to end of line ---

    #[test]
    fn ctrl_k_deletes_to_line_end() {
        let mut s = state("hello world", 5);
        s.kill_to_line_end();
        assert_eq!(s.input, "hello");
        assert_eq!(s.input_cursor, 5);
    }

    #[test]
    fn ctrl_k_multiline_kills_within_line() {
        let mut s = state("first\nsecond\nthird", 8);
        s.kill_to_line_end();
        assert_eq!(s.input, "first\nse\nthird");
        assert_eq!(s.input_cursor, 8);
    }

    #[test]
    fn ctrl_k_at_line_end_noop() {
        let mut s = state("hello\nworld", 5);
        s.kill_to_line_end();
        assert_eq!(s.input, "hello\nworld");
        assert_eq!(s.input_cursor, 5);
    }

    // --- Ctrl+U: kill to start of line ---

    #[test]
    fn ctrl_u_deletes_to_line_start() {
        let mut s = state("hello world", 5);
        s.kill_to_line_start();
        assert_eq!(s.input, " world");
        assert_eq!(s.input_cursor, 0);
    }

    #[test]
    fn ctrl_u_multiline_kills_within_line() {
        let mut s = state("first\nsecond\nthird", 10);
        s.kill_to_line_start();
        assert_eq!(s.input, "first\nnd\nthird");
        assert_eq!(s.input_cursor, 6);
    }

    #[test]
    fn ctrl_u_at_line_start_noop() {
        let mut s = state("hello\nworld", 6);
        s.kill_to_line_start();
        assert_eq!(s.input, "hello\nworld");
        assert_eq!(s.input_cursor, 6);
    }

    // --- Ctrl+W: delete previous word ---

    #[test]
    fn ctrl_w_deletes_word() {
        let mut s = state("hello world", 11);
        s.delete_prev_word();
        assert_eq!(s.input, "hello ");
        assert_eq!(s.input_cursor, 6);
    }

    #[test]
    fn ctrl_w_deletes_word_with_trailing_spaces() {
        let mut s = state("hello   world", 8);
        s.delete_prev_word();
        assert_eq!(s.input, "world");
        assert_eq!(s.input_cursor, 0);
    }

    #[test]
    fn ctrl_w_at_start_noop() {
        let mut s = state("hello", 0);
        s.delete_prev_word();
        assert_eq!(s.input, "hello");
        assert_eq!(s.input_cursor, 0);
    }

    #[test]
    fn ctrl_w_deletes_punctuation_group() {
        let mut s = state("foo::bar", 5);
        s.delete_prev_word();
        assert_eq!(s.input, "foobar");
        assert_eq!(s.input_cursor, 3);
    }

    // --- Ctrl+B/F: character movement ---

    #[test]
    fn ctrl_b_moves_left() {
        let mut s = state("hello", 3);
        s.move_cursor_left();
        assert_eq!(s.input_cursor, 2);
    }

    #[test]
    fn ctrl_b_at_start_stays() {
        let mut s = state("hello", 0);
        s.move_cursor_left();
        assert_eq!(s.input_cursor, 0);
    }

    #[test]
    fn ctrl_f_moves_right() {
        let mut s = state("hello", 2);
        s.move_cursor_right();
        assert_eq!(s.input_cursor, 3);
    }

    #[test]
    fn ctrl_f_at_end_stays() {
        let mut s = state("hello", 5);
        s.move_cursor_right();
        assert_eq!(s.input_cursor, 5);
    }

    // --- Ctrl+Left/Right: word jump ---

    #[test]
    fn word_left_jumps_to_word_start() {
        let mut s = state("hello world", 11);
        s.move_cursor_word_left();
        assert_eq!(s.input_cursor, 6);
    }

    #[test]
    fn word_left_from_middle_of_word() {
        let mut s = state("hello world", 8);
        s.move_cursor_word_left();
        assert_eq!(s.input_cursor, 6);
    }

    #[test]
    fn word_right_jumps_past_word() {
        let mut s = state("hello world", 0);
        s.move_cursor_word_right();
        assert_eq!(s.input_cursor, 6);
    }

    #[test]
    fn word_right_from_space() {
        let mut s = state("hello  world", 5);
        s.move_cursor_word_right();
        assert_eq!(s.input_cursor, 7);
    }

    #[test]
    fn word_right_at_end() {
        let mut s = state("hello", 5);
        s.move_cursor_word_right();
        assert_eq!(s.input_cursor, 5);
    }

    #[test]
    fn word_left_at_start() {
        let mut s = state("hello", 0);
        s.move_cursor_word_left();
        assert_eq!(s.input_cursor, 0);
    }

    // --- Word boundary free functions ---

    #[test]
    fn prev_word_boundary_skips_whitespace() {
        assert_eq!(readline_prev_word_boundary("foo   bar", 9), 6);
    }

    #[test]
    fn next_word_boundary_skips_whitespace() {
        assert_eq!(readline_next_word_boundary("foo   bar", 0), 6);
    }

    #[test]
    fn prev_word_boundary_punctuation() {
        assert_eq!(readline_prev_word_boundary("foo.bar", 4), 3);
    }

    #[test]
    fn next_word_boundary_punctuation() {
        assert_eq!(readline_next_word_boundary("foo.bar", 3), 4);
    }

    #[test]
    fn prev_word_boundary_empty() {
        assert_eq!(readline_prev_word_boundary("", 0), 0);
    }

    #[test]
    fn next_word_boundary_empty() {
        assert_eq!(readline_next_word_boundary("", 0), 0);
    }

    // --- cursor_in_line_indent ---

    #[test]
    fn cursor_in_line_indent_at_line_start() {
        // New line begun mid-edit: caret at the fresh line's start is an indent
        // context, so Tab must indent rather than queue a follow-up.
        assert!(state("first line\n", 11).cursor_in_line_indent());
    }

    #[test]
    fn cursor_in_line_indent_after_leading_whitespace() {
        assert!(state("first\n    ", 9).cursor_in_line_indent());
    }

    #[test]
    fn cursor_in_line_indent_false_after_text() {
        // Caret past non-whitespace on the line: Tab keeps its follow-up role.
        assert!(!state("hello", 5).cursor_in_line_indent());
        assert!(!state("first\n  bbb", 11).cursor_in_line_indent());
    }

    // --- delete_current_line ---

    #[test]
    fn delete_current_line_single_line() {
        let mut s = state("hello world", 5);
        s.delete_current_line();
        assert_eq!(s.input, "");
        assert_eq!(s.input_cursor, 0);
    }

    #[test]
    fn delete_current_line_first_of_many() {
        let mut s = state("first\nsecond\nthird", 3);
        s.delete_current_line();
        assert_eq!(s.input, "second\nthird");
        assert_eq!(s.input_cursor, 0);
    }

    #[test]
    fn delete_current_line_middle() {
        let mut s = state("first\nsecond\nthird", 8);
        s.delete_current_line();
        assert_eq!(s.input, "first\nthird");
        assert_eq!(s.input_cursor, 6);
    }

    #[test]
    fn delete_current_line_last() {
        let mut s = state("first\nsecond\nthird", 14);
        s.delete_current_line();
        assert_eq!(s.input, "first\nsecond");
        assert_eq!(s.input_cursor, 12);
    }

    // --- indent_selected_lines ---

    #[test]
    fn indent_selected_lines_two_lines() {
        let mut s = state("aaa\nbbb\nccc", 0);
        s.input_selection = Some(crate::prompt_state::InputSelectionState {
            anchor: 0,
            focus: 8,
        });
        s.indent_selected_lines();
        assert_eq!(s.input, "    aaa\n    bbb\nccc");
        assert_eq!(s.input_cursor, 0);
        assert!(s.input_selection.is_some());
    }

    #[test]
    fn indent_selected_lines_all_lines() {
        let mut s = state("aaa\nbbb", 0);
        s.input_selection = Some(crate::prompt_state::InputSelectionState {
            anchor: 0,
            focus: 7,
        });
        s.indent_selected_lines();
        assert_eq!(s.input, "    aaa\n    bbb");
    }

    // --- dedent_selected_lines ---

    #[test]
    fn dedent_selected_lines_removes_spaces() {
        let mut s = state("    aaa\n    bbb\nccc", 0);
        s.input_selection = Some(crate::prompt_state::InputSelectionState {
            anchor: 0,
            focus: 16,
        });
        s.dedent_selected_lines();
        assert_eq!(s.input, "aaa\nbbb\nccc");
    }

    #[test]
    fn dedent_selected_lines_partial_spaces() {
        let mut s = state("  aaa\n      bbb", 0);
        s.input_selection = Some(crate::prompt_state::InputSelectionState {
            anchor: 0,
            focus: 15,
        });
        s.dedent_selected_lines();
        assert_eq!(s.input, "aaa\n  bbb");
    }

    // --- column memory ---

    #[test]
    fn column_memory_preserved_across_short_line() {
        // "long line\nhi\nlong line" — cursor at column 8 of first line
        let mut s = state("long line\nhi\nlong line", 8);
        // Move down to "hi" — cursor should clamp to end of "hi" (col 2)
        s.move_cursor_logical_vertical(1);
        assert_eq!(s.input_cursor, 12); // end of "hi"
        // Move down again — should restore to column 8
        s.move_cursor_logical_vertical(1);
        assert_eq!(s.input_cursor, 21); // offset 13 + 8 = 21
    }

    #[test]
    fn column_memory_cleared_on_horizontal_move() {
        let mut s = state("long line\nhi\nlong line", 8);
        s.move_cursor_logical_vertical(1);
        assert_eq!(s.input_cursor, 12);
        // Horizontal move clears desired_column
        s.move_cursor_left();
        assert_eq!(s.input_cursor, 11);
        assert!(s.desired_column.is_none());
        // Subsequent vertical move uses new position (column 1)
        s.move_cursor_logical_vertical(1);
        assert_eq!(s.input_cursor, 14); // offset 13 + 1 = 14
    }
}
