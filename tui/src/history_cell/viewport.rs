use anyhow::Result;
use ratatui::{
    prelude::{Modifier, Rect, Style},
    text::Line,
};

use crate::clipboard::copy_text_to_clipboard;
use crate::render::styled_wrap::{push_styled_char, wrap_styled_lines};
use crate::render::text_utils::{StyledLine, display_width, styled_line_display_width};
use crate::state::TuiState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptSelectionPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

impl TranscriptSelectionPoint {
    pub(crate) fn ordered(
        first: TranscriptSelectionPoint,
        second: TranscriptSelectionPoint,
    ) -> (TranscriptSelectionPoint, TranscriptSelectionPoint) {
        if first.row < second.row || (first.row == second.row && first.column <= second.column) {
            (first, second)
        } else {
            (second, first)
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptSelectionState {
    pub(crate) area: Rect,
    pub(crate) anchor: TranscriptSelectionPoint,
    pub(crate) focus: TranscriptSelectionPoint,
}

impl TranscriptSelectionState {
    pub(crate) fn matches_area(&self, area: Rect) -> bool {
        // Only the width affects the line wrapping that the selection's row
        // coordinates depend on. Height changes (e.g. the spinner / status
        // panel growing or shrinking while the user is mid-drag) shift the
        // transcript area but leave row indices meaningful, so they should
        // not invalidate the selection.
        self.area.width == area.width
    }

    pub(crate) fn normalized_bounds(&self) -> (TranscriptSelectionPoint, TranscriptSelectionPoint) {
        TranscriptSelectionPoint::ordered(self.anchor, self.focus)
    }

    pub(crate) fn update_focus(&mut self, focus: TranscriptSelectionPoint) {
        self.focus = focus;
    }

    pub(crate) fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptViewportState {
    pub(crate) area: Rect,
    pub(crate) lines: Vec<StyledLine>,
    pub(crate) all_lines: Vec<StyledLine>,
    pub(crate) all_lines_start: usize,
    pub(crate) all_line_count: usize,
    pub(crate) selection_lines: Vec<StyledLine>,
    pub(crate) selection_lines_start: usize,
    pub(crate) visible_row_start: usize,
    pub(crate) current_scroll: usize,
    pub(crate) max_scroll: usize,
    pub(crate) selection: Option<TranscriptSelectionState>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptViewLines {
    pub(crate) all_lines: Vec<StyledLine>,
    pub(crate) all_lines_start: usize,
    pub(crate) all_line_count: usize,
    pub(crate) selection_lines: Vec<StyledLine>,
    pub(crate) selection_lines_start: usize,
    pub(crate) visible_lines: Vec<StyledLine>,
    pub(crate) visible_row_start: usize,
    pub(crate) actual_scroll: usize,
    pub(crate) max_scroll: usize,
}

impl Default for TranscriptViewportState {
    fn default() -> Self {
        Self {
            area: Rect::ZERO,
            lines: Vec::new(),
            all_lines: Vec::new(),
            all_lines_start: 0,
            all_line_count: 0,
            selection_lines: Vec::new(),
            selection_lines_start: 0,
            visible_row_start: 0,
            current_scroll: 0,
            max_scroll: 0,
            selection: None,
        }
    }
}

impl TranscriptViewportState {
    pub(crate) fn sync(
        &mut self,
        area: Rect,
        lines: Vec<StyledLine>,
        all_lines: Vec<StyledLine>,
        visible_row_start: usize,
        current_scroll: usize,
        max_scroll: usize,
    ) {
        let all_line_count = all_lines.len();
        self.sync_with_window(
            area,
            lines,
            all_lines,
            0,
            all_line_count,
            Vec::new(),
            0,
            visible_row_start,
            current_scroll,
            max_scroll,
        );
    }

    pub(crate) fn sync_with_window(
        &mut self,
        area: Rect,
        lines: Vec<StyledLine>,
        all_lines: Vec<StyledLine>,
        all_lines_start: usize,
        all_line_count: usize,
        selection_lines: Vec<StyledLine>,
        selection_lines_start: usize,
        visible_row_start: usize,
        current_scroll: usize,
        max_scroll: usize,
    ) {
        self.area = area;
        self.lines = lines;
        self.all_lines = all_lines;
        self.all_lines_start = all_lines_start;
        self.all_line_count =
            all_line_count.max(all_lines_start.saturating_add(self.all_lines.len()));
        self.selection_lines = selection_lines;
        self.selection_lines_start = selection_lines_start;
        self.visible_row_start = visible_row_start;
        self.current_scroll = current_scroll.min(max_scroll);
        self.max_scroll = max_scroll;
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.matches_area(area))
        {
            self.selection = None;
        } else if let Some(selection) = self.selection.as_mut() {
            if self.all_line_count == 0 {
                self.selection = None;
            } else {
                let max_row = self.all_line_count.saturating_sub(1);
                selection.anchor.row = selection.anchor.row.min(max_row);
                selection.focus.row = selection.focus.row.min(max_row);
            }
        }
    }

    pub(crate) fn set_selection_lines(&mut self, lines: Vec<StyledLine>, start: usize) {
        self.selection_lines = lines;
        self.selection_lines_start = start;
    }

    pub(crate) fn render_lines(&self) -> Vec<StyledLine> {
        match &self.selection {
            Some(selection) => self
                .lines
                .iter()
                .enumerate()
                .map(|(row, line)| {
                    apply_selection_to_line(line, self.visible_row_start + row, selection)
                })
                .collect(),
            None => self.lines.clone(),
        }
    }

    pub(crate) fn selection_point_from_mouse(
        &self,
        column: u16,
        row: u16,
    ) -> Option<TranscriptSelectionPoint> {
        let area = self.area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x.saturating_add(area.width)
            || row < area.y
            || row >= area.y.saturating_add(area.height)
        {
            return None;
        }

        let visual_row = row.saturating_sub(area.y) as usize;
        let line = self.lines.get(visual_row)?;
        let local_column = column.saturating_sub(area.x) as usize;
        let max_column = styled_line_display_width(line).saturating_sub(1);
        Some(TranscriptSelectionPoint {
            row: self.visible_row_start + visual_row,
            column: local_column.min(max_column),
        })
    }

    pub(crate) fn selection_point_from_mouse_clamped(
        &self,
        column: u16,
        row: u16,
    ) -> Option<TranscriptSelectionPoint> {
        let area = self.area;
        if area.width == 0 || area.height == 0 || self.lines.is_empty() {
            return None;
        }
        let max_x = area.x.saturating_add(area.width.saturating_sub(1));
        let max_y = area.y.saturating_add(area.height.saturating_sub(1));
        let clamped_column = column.clamp(area.x, max_x);
        let clamped_row = row.clamp(area.y, max_y);
        self.selection_point_from_mouse(clamped_column, clamped_row)
    }

    pub(crate) fn begin_selection(&mut self, point: TranscriptSelectionPoint) -> bool {
        if self.all_line_count == 0 || point.row >= self.all_line_count {
            return false;
        }
        self.selection = Some(TranscriptSelectionState {
            area: self.area,
            anchor: point,
            focus: point,
        });
        true
    }

    pub(crate) fn update_selection(&mut self, point: TranscriptSelectionPoint) -> bool {
        let Some(selection) = self.selection.as_mut() else {
            return false;
        };
        selection.update_focus(point);
        true
    }

    pub(crate) fn clear_selection(&mut self) -> bool {
        self.selection.take().is_some()
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let (start, end) = selection.normalized_bounds();
        let mut rows = Vec::new();
        let mut materialized_segment_started = false;
        for row in start.row..=end.row {
            let Some(line) = self.line_for_row(row) else {
                // Reflow and windowed rendering can leave a temporary gap in
                // the selected row range. Copy the first materialized
                // contiguous segment instead of discarding the whole copy.
                if materialized_segment_started {
                    break;
                }
                continue;
            };
            materialized_segment_started = true;
            rows.push(selected_text_for_line(line, row, start, end));
        }
        let selected = rows.join("\n");
        (!selected.is_empty()).then_some(selected)
    }

    fn line_for_row(&self, row: usize) -> Option<&StyledLine> {
        if let Some(line_index) = row.checked_sub(self.all_lines_start)
            && let Some(line) = self.all_lines.get(line_index)
        {
            return Some(line);
        }
        if let Some(line_index) = row.checked_sub(self.selection_lines_start)
            && let Some(line) = self.selection_lines.get(line_index)
        {
            return Some(line);
        }
        None
    }

    pub(crate) fn first_visible_selection_point(&self) -> Option<TranscriptSelectionPoint> {
        self.lines.first()?;
        Some(TranscriptSelectionPoint {
            row: self.visible_row_start,
            column: 0,
        })
    }

    pub(crate) fn last_visible_selection_point(&self) -> Option<TranscriptSelectionPoint> {
        let row_offset = self.lines.len().checked_sub(1)?;
        let line = self.lines.get(row_offset)?;
        Some(TranscriptSelectionPoint {
            row: self.visible_row_start + row_offset,
            column: styled_line_display_width(line).saturating_sub(1),
        })
    }

    pub(crate) fn has_expanded_selection(&self) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|selection| !selection.is_collapsed())
    }

    pub(crate) fn current_scroll(&self) -> usize {
        self.current_scroll.min(self.max_scroll)
    }

    pub(crate) fn max_scroll(&self) -> usize {
        self.max_scroll
    }

    pub(crate) fn set_scroll(&mut self, scroll: usize) {
        self.current_scroll = scroll.min(self.max_scroll);
        if self.area.height == 0 || self.all_line_count == 0 {
            self.visible_row_start = 0;
            self.lines.clear();
            return;
        }
        let end = self.all_line_count.saturating_sub(self.current_scroll());
        let start = end.saturating_sub(self.area.height as usize);
        self.visible_row_start = start;
        let Some(local_start) = start.checked_sub(self.all_lines_start) else {
            self.lines.clear();
            return;
        };
        let local_end = local_start.saturating_add(end.saturating_sub(start));
        if local_end <= self.all_lines.len() {
            self.lines = self.all_lines[local_start..local_end].to_vec();
        } else {
            self.lines.clear();
        }
    }

    pub(crate) fn is_at_top(&self) -> bool {
        self.current_scroll() >= self.max_scroll()
    }

    pub(crate) fn is_at_bottom(&self) -> bool {
        self.current_scroll() == 0
    }
}

pub(crate) fn visible_transcript_lines(
    lines: &[StyledLine],
    width: usize,
    limit: usize,
    requested_scroll: usize,
) -> TranscriptViewLines {
    if limit == 0 || lines.is_empty() {
        return TranscriptViewLines::default();
    }

    let visual_lines = wrap_styled_lines(lines, width);
    let max_scroll = visual_lines.len().saturating_sub(limit);
    let scroll = requested_scroll.min(max_scroll);
    let end = visual_lines.len().saturating_sub(scroll);
    let start = end.saturating_sub(limit);
    TranscriptViewLines {
        all_lines: visual_lines.clone(),
        all_lines_start: 0,
        all_line_count: visual_lines.len(),
        selection_lines: Vec::new(),
        selection_lines_start: 0,
        visible_lines: visual_lines[start..end].to_vec(),
        visible_row_start: start,
        actual_scroll: scroll,
        max_scroll,
    }
}

pub(crate) fn visual_line_window_from_sections(
    sections: &[&[StyledLine]],
    start: usize,
    end: usize,
) -> Vec<StyledLine> {
    let mut visible_lines = Vec::with_capacity(end.saturating_sub(start));
    let mut section_start = 0usize;
    for section in sections {
        let section_end = section_start.saturating_add(section.len());
        if start < section_end && end > section_start {
            let local_start = start.saturating_sub(section_start);
            let local_end = end.saturating_sub(section_start).min(section.len());
            if local_start < local_end {
                visible_lines.extend(section[local_start..local_end].iter().cloned());
            }
        }
        section_start = section_end;
    }
    visible_lines
}

pub(crate) fn visible_transcript_lines_from_visual_sections(
    sections: &[&[StyledLine]],
    limit: usize,
    requested_scroll: usize,
    selection: Option<&TranscriptSelectionState>,
) -> TranscriptViewLines {
    let total_line_count = sections
        .iter()
        .map(|section| section.len())
        .fold(0usize, usize::saturating_add);
    if limit == 0 || total_line_count == 0 {
        return TranscriptViewLines::default();
    }

    let max_scroll = total_line_count.saturating_sub(limit);
    let actual_scroll = requested_scroll.min(max_scroll);
    let end = total_line_count.saturating_sub(actual_scroll);
    let start = end.saturating_sub(limit);
    let visible_lines = visual_line_window_from_sections(sections, start, end);
    let (all_lines, all_lines_start) = if selection.is_some() {
        let window_start = start.saturating_sub(1);
        let window_end = end.saturating_add(1).min(total_line_count);
        (
            visual_line_window_from_sections(sections, window_start, window_end),
            window_start,
        )
    } else {
        (visible_lines.clone(), start)
    };
    let (selection_lines, selection_lines_start) =
        selection_line_window_from_sections(sections, total_line_count, selection)
            .unwrap_or_default();

    TranscriptViewLines {
        all_lines,
        all_lines_start,
        all_line_count: total_line_count,
        selection_lines,
        selection_lines_start,
        visible_lines,
        visible_row_start: start,
        actual_scroll,
        max_scroll,
    }
}

pub(crate) fn selection_line_window_from_sections(
    sections: &[&[StyledLine]],
    total_line_count: usize,
    selection: Option<&TranscriptSelectionState>,
) -> Option<(Vec<StyledLine>, usize)> {
    let selection = selection?;
    if total_line_count == 0 {
        return None;
    }
    let (start, end) = selection.normalized_bounds();
    let start_row = start.row.min(total_line_count.saturating_sub(1));
    let end_row = end.row.min(total_line_count.saturating_sub(1));
    if start_row > end_row {
        return None;
    }
    let end_exclusive = end_row.saturating_add(1).min(total_line_count);
    Some((
        visual_line_window_from_sections(sections, start_row, end_exclusive),
        start_row,
    ))
}

impl TuiState {
    pub(crate) fn clear_latest_message_focus(&mut self) {
        self.focus_latest_message_start = false;
    }

    pub(crate) fn transcript_bottom_pin_is_sticky(&self) -> bool {
        self.retained_visible_transcript_cells > 0
    }

    #[cfg(test)]
    pub(crate) fn mark_transcript_bottom_pin_sticky(&mut self) {
        self.retained_visible_transcript_cells = 1;
    }

    pub(crate) fn clear_transcript_bottom_pin_sticky(&mut self) {
        self.retained_visible_transcript_cells = 0;
    }

    pub(crate) fn has_transcript_selection(&self) -> bool {
        self.transcript_ui.viewport.selection.is_some()
    }

    pub(crate) fn clear_transcript_selection(&mut self) {
        self.transcript_ui.viewport.clear_selection();
    }

    pub(crate) fn copy_selected_transcript_to_clipboard(&mut self) -> Result<usize> {
        self.refresh_transcript_selection_lines();
        let selected = self
            .transcript_ui
            .viewport
            .selected_text()
            .ok_or_else(|| anyhow::anyhow!("No transcript text is selected."))?;
        copy_text_to_clipboard(&selected)?;
        Ok(selected.chars().count())
    }
}

fn selection_style(base: Style) -> Style {
    base.remove_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED)
}

fn apply_selection_to_line(
    line: &StyledLine,
    row: usize,
    selection: &TranscriptSelectionState,
) -> StyledLine {
    let (start, end) = selection.normalized_bounds();
    let mut spans = Vec::new();
    let mut column = 0usize;

    for span in &line.spans {
        for ch in span.content.chars() {
            let width = display_width(ch);
            let char_start = column;
            let char_end = column + width.saturating_sub(1);
            let style = if transcript_selection_contains_cell(row, char_start, char_end, start, end)
            {
                selection_style(span.style)
            } else {
                span.style
            };
            push_styled_char(&mut spans, ch, style);
            column += width;
        }
    }

    if spans.is_empty() {
        Line::default()
    } else {
        Line::from(spans)
    }
}

fn selected_text_for_line(
    line: &StyledLine,
    row: usize,
    start: TranscriptSelectionPoint,
    end: TranscriptSelectionPoint,
) -> String {
    let mut text = String::new();
    let mut column = 0usize;

    for span in &line.spans {
        for ch in span.content.chars() {
            let width = display_width(ch);
            let char_start = column;
            let char_end = column + width.saturating_sub(1);
            if transcript_selection_contains_cell(row, char_start, char_end, start, end) {
                text.push(ch);
            }
            column += width;
        }
    }

    text
}

fn transcript_selection_contains_cell(
    row: usize,
    char_start: usize,
    char_end: usize,
    start: TranscriptSelectionPoint,
    end: TranscriptSelectionPoint,
) -> bool {
    if row < start.row || row > end.row {
        return false;
    }

    if start.row == end.row {
        return char_end >= start.column && char_start <= end.column;
    }

    if row == start.row {
        return char_end >= start.column;
    }

    if row == end.row {
        return char_start <= end.column;
    }

    true
}

#[cfg(test)]
mod tests {
    use ratatui::text::{Line, Span};

    use super::*;
    use crate::tui_theme::subtle_style;

    fn full_line_selection(row: usize) -> (TranscriptSelectionPoint, TranscriptSelectionPoint) {
        (
            TranscriptSelectionPoint { row, column: 0 },
            TranscriptSelectionPoint {
                row,
                column: usize::MAX,
            },
        )
    }

    #[test]
    fn copy_preserves_tree_branch_prefix_verbatim() {
        let line: StyledLine = Line::from(vec![
            Span::styled("  │ ", subtle_style()),
            Span::raw("Running npm install"),
        ]);
        let (start, end) = full_line_selection(0);
        let text = selected_text_for_line(&line, 0, start, end);
        assert_eq!(text, "  │ Running npm install");
    }

    #[test]
    fn copy_preserves_tool_result_prefix_verbatim() {
        let line: StyledLine = Line::from(vec![
            Span::styled("  └ ", subtle_style()),
            Span::raw("Done in 2.3s"),
        ]);
        let (start, end) = full_line_selection(0);
        let text = selected_text_for_line(&line, 0, start, end);
        assert_eq!(text, "  └ Done in 2.3s");
    }

    #[test]
    fn copy_preserves_assistant_bullet_verbatim() {
        let line: StyledLine = Line::from(vec![
            Span::styled("●", subtle_style()),
            Span::raw(" "),
            Span::raw("Here is the answer"),
        ]);
        let (start, end) = full_line_selection(0);
        let text = selected_text_for_line(&line, 0, start, end);
        assert_eq!(text, "● Here is the answer");
    }

    #[test]
    fn copy_preserves_plain_content() {
        let line: StyledLine = Line::from("plain content without prefix");
        let (start, end) = full_line_selection(0);
        let text = selected_text_for_line(&line, 0, start, end);
        assert_eq!(text, "plain content without prefix");
    }

    #[test]
    fn selected_text_returns_first_materialized_segment_across_cache_gap() {
        let mut viewport = TranscriptViewportState {
            all_lines: vec![Line::from("row zero"), Line::from("row one")],
            all_lines_start: 0,
            all_line_count: 5,
            selection_lines: vec![Line::from("row three"), Line::from("row four")],
            selection_lines_start: 3,
            ..TranscriptViewportState::default()
        };
        viewport.selection = Some(TranscriptSelectionState {
            area: Rect::new(0, 0, 40, 5),
            anchor: TranscriptSelectionPoint { row: 0, column: 0 },
            focus: TranscriptSelectionPoint {
                row: 4,
                column: usize::MAX,
            },
        });

        assert_eq!(
            viewport.selected_text().as_deref(),
            Some("row zero\nrow one")
        );
    }

    #[test]
    fn selected_text_skips_leading_gap_before_materialized_segment() {
        let mut viewport = TranscriptViewportState {
            all_lines: vec![Line::from("row two"), Line::from("row three")],
            all_lines_start: 2,
            all_line_count: 4,
            ..TranscriptViewportState::default()
        };
        viewport.selection = Some(TranscriptSelectionState {
            area: Rect::new(0, 0, 40, 4),
            anchor: TranscriptSelectionPoint { row: 0, column: 0 },
            focus: TranscriptSelectionPoint {
                row: 3,
                column: usize::MAX,
            },
        });

        assert_eq!(
            viewport.selected_text().as_deref(),
            Some("row two\nrow three")
        );
    }
}
