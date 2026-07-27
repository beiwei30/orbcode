use crate::bottom_pane::vim::{
    FindKind, LastFind, MotionKind, OperatorKind, RecordedChange, TextObjectKind, TextObjectScope,
    current_line_end_boundary, find_character, find_text_object, go_to_line_from_command,
    last_motion_offset, line_index_at_offset, offset_for_line, operator_find_range,
    operator_motion_range, operator_motion_range_for_absolute_line, resolve_motion_offset,
    snap_out_of_range_end, snap_out_of_range_start,
};
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn execute_line_op(&mut self, op: OperatorKind, count: usize, record_change: bool) {
        let current_line_index = line_index_at_offset(&self.input, self.input_cursor);
        let lines = self
            .input
            .split('\n')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let lines_to_affect = count.min(lines.len().saturating_sub(current_line_index));
        if lines_to_affect == 0 {
            return;
        }
        let line_start = offset_for_line(&self.input, current_line_index);
        let mut line_end = line_start;
        for _ in 0..lines_to_affect {
            line_end = current_line_end_boundary(&self.input, line_end);
            if line_end < self.input.len() {
                line_end += 1;
            } else {
                break;
            }
        }
        let mut from = line_start;
        let to = line_end.min(self.input.len());
        let linewise = true;
        if op != OperatorKind::Yank
            && to == self.input.len()
            && from > 0
            && self.input.as_bytes()[from - 1] == b'\n'
        {
            from -= 1;
        }

        if op == OperatorKind::Change {
            let mut content = self.input[from..to].to_string();
            if !content.ends_with('\n') {
                content.push('\n');
            }
            self.set_register(content, true);
            self.push_undo_state();

            if lines.len() == 1 {
                self.input.clear();
                self.input_cursor = 0;
            } else {
                let mut new_lines = Vec::new();
                new_lines.extend_from_slice(&lines[..current_line_index]);
                new_lines.push(String::new());
                new_lines.extend_from_slice(&lines[current_line_index + lines_to_affect..]);
                self.input = new_lines.join("\n");
                self.input_cursor = offset_for_line(&self.input, current_line_index);
            }

            self.vim_state.pending_insert_change =
                record_change.then_some(RecordedChange::LineOp { op, count });
            self.enter_insert_mode();
            return;
        }

        self.apply_operator_range(
            op,
            from,
            to,
            linewise,
            record_change.then_some(RecordedChange::LineOp { op, count }),
        );
    }

    pub(crate) fn execute_operator_motion(
        &mut self,
        op: OperatorKind,
        motion: MotionKind,
        count: usize,
        record_change: bool,
    ) {
        let start = self.input_cursor;
        let target = resolve_motion_offset(&self.input, self.input_cursor, motion, count);
        if target == start {
            return;
        }

        let (mut from, mut to, linewise) =
            operator_motion_range(&self.input, start, target, motion, op, count);
        from = snap_out_of_range_start(&self.input, from);
        to = snap_out_of_range_end(&self.input, to);

        self.apply_operator_range(
            op,
            from,
            to,
            linewise,
            record_change.then_some(RecordedChange::OperatorMotion { op, motion, count }),
        );
    }

    pub(crate) fn execute_operator_find(
        &mut self,
        op: OperatorKind,
        kind: FindKind,
        target: char,
        count: usize,
        record_change: bool,
    ) {
        let Some(found) = find_character(&self.input, self.input_cursor, target, kind, count)
        else {
            return;
        };
        let (from, to) = operator_find_range(&self.input, self.input_cursor, found);
        self.last_find = Some(LastFind { kind, target });
        self.apply_operator_range(
            op,
            from,
            to,
            false,
            record_change.then_some(RecordedChange::OperatorFind {
                op,
                kind,
                target,
                count,
            }),
        );
    }

    pub(crate) fn execute_operator_text_object(
        &mut self,
        op: OperatorKind,
        scope: TextObjectScope,
        kind: TextObjectKind,
        count: usize,
        record_change: bool,
    ) {
        let Some((start, end)) = find_text_object(&self.input, self.input_cursor, kind, scope)
        else {
            return;
        };
        let final_start = start;
        let mut final_end = end;
        for _ in 1..count {
            let Some((_, next_end)) = find_text_object(
                &self.input,
                final_end.min(self.input.len().saturating_sub(1)),
                kind,
                scope,
            ) else {
                break;
            };
            final_end = next_end;
        }

        self.apply_operator_range(
            op,
            final_start,
            final_end,
            false,
            record_change.then_some(RecordedChange::OperatorTextObject {
                op,
                scope,
                kind,
                count,
            }),
        );
    }

    pub(crate) fn execute_operator_g(
        &mut self,
        op: OperatorKind,
        count: usize,
        record_change: bool,
    ) {
        let motion = MotionKind::LastLine;
        self.execute_operator_motion(op, motion, count, record_change);
    }

    pub(crate) fn execute_operator_gg(
        &mut self,
        op: OperatorKind,
        count: usize,
        record_change: bool,
    ) {
        let target = if count == 1 { 1 } else { count };
        let current = self.input_cursor;
        let target_offset = go_to_line_from_command(&self.input, target);
        let (from, to, linewise) =
            operator_motion_range_for_absolute_line(&self.input, current, target_offset);
        self.apply_operator_range(
            op,
            from,
            to,
            linewise,
            record_change.then_some(RecordedChange::OperatorMotion {
                op,
                motion: MotionKind::FirstLine,
                count: target,
            }),
        );
    }

    pub(crate) fn apply_operator_range(
        &mut self,
        op: OperatorKind,
        from: usize,
        to: usize,
        linewise: bool,
        record: Option<RecordedChange>,
    ) {
        if from >= to {
            return;
        }

        let mut content = self.input[from..to].to_string();
        if linewise && !content.ends_with('\n') {
            content.push('\n');
        }
        self.set_register(content, linewise);

        match op {
            OperatorKind::Yank => {
                self.input_cursor = from;
            }
            OperatorKind::Delete => {
                self.push_undo_state();
                self.input.replace_range(from..to, "");
                self.input_cursor = from.min(last_motion_offset(&self.input));
                if let Some(change) = record {
                    self.vim_state.last_change = Some(change);
                }
            }
            OperatorKind::Change => {
                self.push_undo_state();
                self.input.replace_range(from..to, "");
                self.input_cursor = from.min(self.input.len());
                self.vim_state.pending_insert_change = record;
                self.enter_insert_mode();
            }
        }
    }
}
