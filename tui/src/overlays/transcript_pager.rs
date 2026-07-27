use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Wrap};

use crate::history_cell::cells::TranscriptCell;
use crate::history_cell::state::history_lines_for_cell_range;
use crate::render::active_transcript::ActiveTranscriptSnapshot;
use crate::render::styled_wrap::wrap_styled_lines;
use crate::render::text_utils::StyledLine;
use crate::render::transcript_cell::{CellRenderMode, render_committed_transcript_cell_lines};
use crate::state::TuiState;

use super::OverlayState;

const TRANSCRIPT_PAGER_OVERSCAN_ROWS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AnchorKind {
    Tail,
    Head,
    Search,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchDirection {
    Forward,
    Backward,
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptPagerState {
    pub(crate) source_signature: u64,
    pub(crate) width: usize,
    source_cell_count: usize,
    anchor_kind: AnchorKind,
    scroll_from_bottom: usize,
    scroll_from_top: usize,
    viewport_height: usize,
    height_cache: Vec<Option<usize>>,
    pub(crate) rendered_window: Vec<StyledLine>,
    pub(crate) loaded_cell_start: usize,
    pub(crate) loaded_cell_end: usize,
    pub(crate) search_query: String,
    pub(crate) search_active: bool,
    pub(crate) search_status: Option<String>,
    search_match_cell: Option<usize>,
    search_direction: SearchDirection,
    source_cells: Vec<TranscriptCell>,
    live_tail_revision: u64,
    live_tail_lines: Vec<StyledLine>,
    cwd: std::path::PathBuf,
    model_display_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TranscriptPagerAction {
    None,
    Close,
}

impl TranscriptPagerState {
    fn new(
        source_signature: u64,
        source_cells: Vec<TranscriptCell>,
        width: usize,
        viewport_height: usize,
        cwd: std::path::PathBuf,
        model_display_name: String,
    ) -> Self {
        let source_cell_count = source_cells.len();
        Self {
            source_signature,
            width: width.max(1),
            source_cell_count,
            anchor_kind: AnchorKind::Tail,
            scroll_from_bottom: 0,
            scroll_from_top: 0,
            viewport_height: viewport_height.max(1),
            height_cache: vec![None; source_cell_count],
            rendered_window: Vec::new(),
            loaded_cell_start: source_cell_count,
            loaded_cell_end: source_cell_count,
            search_query: String::new(),
            search_active: false,
            search_status: None,
            search_match_cell: None,
            search_direction: SearchDirection::Forward,
            source_cells,
            live_tail_revision: 0,
            live_tail_lines: Vec::new(),
            cwd,
            model_display_name,
        }
    }

    pub(crate) fn sync_live_tail(&mut self, snapshot: &ActiveTranscriptSnapshot) -> bool {
        if self.live_tail_revision == snapshot.revision {
            return false;
        }
        self.live_tail_revision = snapshot.revision;
        self.live_tail_lines = snapshot.lines.clone();
        true
    }

    pub(crate) fn sync_viewport(&mut self, area: Rect) {
        let width = area.width.max(1) as usize;
        let vh = pager_content_height(area).max(1);
        if self.width != width {
            self.width = width;
            self.height_cache = vec![None; self.source_cell_count];
        }
        self.viewport_height = vh;
        self.materialize();
    }

    #[cfg(test)]
    pub(crate) fn source_cells_len(&self) -> usize {
        self.source_cells.len()
    }

    #[cfg(test)]
    pub(crate) fn scroll_from_bottom(&self) -> usize {
        self.scroll_from_bottom
    }

    fn page_step(&self) -> usize {
        self.viewport_height.saturating_sub(1).max(1)
    }

    fn cell_height(&mut self, index: usize) -> usize {
        if let Some(height) = self.height_cache.get(index).and_then(|h| *h) {
            return height;
        }
        let lines = self.render_cell_lines(index);
        let height = wrap_styled_lines(&lines, self.width)
            .len()
            .max(1)
            .saturating_add(1);
        if let Some(slot) = self.height_cache.get_mut(index) {
            *slot = Some(height);
        }
        height
    }

    fn render_cell_lines(&self, index: usize) -> Vec<StyledLine> {
        let Some(cell) = self.source_cells.get(index) else {
            return Vec::new();
        };
        let last_thinking_block =
            last_visible_thinking_block_from_cells(&self.source_cells, self.source_cell_count);
        render_committed_transcript_cell_lines(
            cell,
            &self.cwd,
            CellRenderMode::Detail,
            last_thinking_block.as_ref(),
            self.width,
            &self.model_display_name,
        )
    }

    fn render_wrapped_range(&self, start: usize, end: usize) -> Vec<StyledLine> {
        let cells = (start..end)
            .map(|index| self.render_cell_lines(index))
            .collect::<Vec<_>>();
        wrap_styled_lines(
            &history_lines_for_cell_range(&cells, 0, cells.len()),
            self.width,
        )
    }

    fn materialize_tail(&mut self) {
        let needed_rows = self
            .viewport_height
            .saturating_add(self.scroll_from_bottom)
            .saturating_add(TRANSCRIPT_PAGER_OVERSCAN_ROWS);
        let live_tail = self.render_live_tail();
        let source_needed = needed_rows.saturating_sub(live_tail.len());
        // Fast first estimate from the per-cell `cell_height`s.
        let mut start = self.source_cell_count;
        let mut rows = 0usize;
        while start > 0 && rows < source_needed {
            start -= 1;
            rows = rows.saturating_add(self.cell_height(start));
        }
        // Correct against the ACTUAL merged render. `cell_height` gives every
        // cell at least two rows (content + separator) even when the merged
        // render skips empty (e.g. blank thinking) cells and omits separators,
        // so the estimate overcounts and can stop early — leaving a short/blank
        // window when scrolling up past empty cells beyond the overscan margin.
        // Extend backward until the real render meets the target.
        let mut wrapped = self.render_wrapped_range(start, self.source_cell_count);
        while start > 0 && wrapped.len() < source_needed {
            start -= 1;
            wrapped = self.render_wrapped_range(start, self.source_cell_count);
        }
        self.loaded_cell_start = start;
        self.loaded_cell_end = self.source_cell_count;
        wrapped.extend(live_tail);
        if start == 0 {
            let max_scroll = wrapped.len().saturating_sub(self.viewport_height);
            self.scroll_from_bottom = self.scroll_from_bottom.min(max_scroll);
        }
        let end = wrapped.len().saturating_sub(self.scroll_from_bottom);
        let vis_start = end.saturating_sub(self.viewport_height);
        self.rendered_window = wrapped[vis_start..end].to_vec();
    }

    fn materialize_head(&mut self) {
        let needed_rows = self
            .viewport_height
            .saturating_add(self.scroll_from_top)
            .saturating_add(TRANSCRIPT_PAGER_OVERSCAN_ROWS);
        // Fast first estimate from the per-cell `cell_height`s.
        let mut end = 0usize;
        let mut rows = 0usize;
        while end < self.source_cell_count && rows < needed_rows {
            rows = rows.saturating_add(self.cell_height(end));
            end += 1;
        }
        // Correct against the ACTUAL merged render. `cell_height` overcounts
        // empty (blank thinking) cells and separators, so the estimate can stop
        // early — leaving `wrapped` shorter than `scroll_from_top` and rendering
        // an empty page. Extend forward until the real render meets the target
        // or every cell is loaded.
        let mut wrapped = self.render_wrapped_range(0, end);
        while end < self.source_cell_count && wrapped.len() < needed_rows {
            end += 1;
            wrapped = self.render_wrapped_range(0, end);
        }
        self.loaded_cell_start = 0;
        self.loaded_cell_end = end;
        if end == self.source_cell_count {
            let max_scroll = wrapped.len().saturating_sub(self.viewport_height);
            self.scroll_from_top = self.scroll_from_top.min(max_scroll);
        }
        let start = self.scroll_from_top.min(wrapped.len());
        let vis_end = start
            .saturating_add(self.viewport_height)
            .min(wrapped.len());
        self.rendered_window = wrapped[start..vis_end].to_vec();
    }

    fn materialize_search(&mut self) {
        let Some(match_cell) = self.search_match_cell else {
            self.materialize_tail();
            return;
        };
        let needed_rows = self
            .viewport_height
            .saturating_add(TRANSCRIPT_PAGER_OVERSCAN_ROWS);
        let mut end = match_cell;
        let mut rows = 0usize;
        while end < self.source_cell_count && rows < needed_rows {
            rows = rows.saturating_add(self.cell_height(end));
            end += 1;
        }
        self.loaded_cell_start = match_cell;
        self.loaded_cell_end = end;
        let wrapped = self.render_wrapped_range(match_cell, end);
        let vis_end = self.viewport_height.min(wrapped.len());
        self.rendered_window = wrapped[..vis_end].to_vec();
    }

    fn materialize(&mut self) {
        match self.anchor_kind {
            AnchorKind::Tail => self.materialize_tail(),
            AnchorKind::Head => self.materialize_head(),
            AnchorKind::Search => self.materialize_search(),
        }
    }

    fn render_live_tail(&self) -> Vec<StyledLine> {
        if self.live_tail_lines.is_empty() {
            return Vec::new();
        }
        let mut lines = Vec::new();
        if self.source_cell_count > 0 {
            lines.push(Line::default());
        }
        lines.extend(self.live_tail_lines.clone());
        wrap_styled_lines(&lines, self.width)
    }

    fn search_match(
        &self,
        direction: SearchDirection,
        start: usize,
        include_start: bool,
    ) -> Option<usize> {
        if self.source_cell_count == 0 || self.search_query.is_empty() {
            return None;
        }
        let case_sensitive = self.search_query.chars().any(char::is_uppercase);
        let query = if case_sensitive {
            self.search_query.clone()
        } else {
            self.search_query.to_lowercase()
        };
        for offset in 0..self.source_cell_count {
            if offset == 0 && !include_start {
                continue;
            }
            let index = match direction {
                SearchDirection::Forward => start.saturating_add(offset) % self.source_cell_count,
                SearchDirection::Backward => {
                    (start + self.source_cell_count - (offset % self.source_cell_count))
                        % self.source_cell_count
                }
            };
            let text = plain_text(&self.render_cell_lines(index));
            let candidate = if case_sensitive {
                text
            } else {
                text.to_lowercase()
            };
            if candidate.contains(&query) {
                return Some(index);
            }
        }
        None
    }

    fn current_cell_index(&self) -> usize {
        match self.anchor_kind {
            AnchorKind::Tail => self.source_cell_count.saturating_sub(1),
            AnchorKind::Head => 0,
            AnchorKind::Search => self
                .search_match_cell
                .unwrap_or_else(|| self.source_cell_count.saturating_sub(1)),
        }
    }
}

impl TuiState {
    pub(crate) fn open_transcript_pager(&mut self, width: usize, viewport_height: usize) {
        self.refresh_transcript_ui_state();
        let snapshot = self.active_transcript_snapshot(width);
        let mut pager = TranscriptPagerState::new(
            self.transcript_ui.source_signature,
            self.transcript_ui.cells.clone(),
            width,
            viewport_height,
            self.cwd.clone(),
            self.model_display_name.clone(),
        );
        pager.sync_live_tail(&snapshot);
        pager.materialize();
        self.overlay = Some(OverlayState::TranscriptPager(pager));
    }
}

pub(crate) fn apply_transcript_pager_key(
    pager: &mut TranscriptPagerState,
    key_event: &KeyEvent,
) -> TranscriptPagerAction {
    if pager.search_active {
        return apply_search_input_key(pager, key_event);
    }

    match key_event.code {
        KeyCode::Char('/') => {
            pager.search_active = true;
            pager.search_query.clear();
            pager.search_status = None;
            TranscriptPagerAction::None
        }
        KeyCode::Char('n') => repeat_search(pager, SearchDirection::Forward),
        KeyCode::Char('N') => repeat_search(pager, SearchDirection::Backward),
        _ => apply_nav_key(pager, key_event),
    }
}

fn apply_nav_key(pager: &mut TranscriptPagerState, key_event: &KeyEvent) -> TranscriptPagerAction {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') => TranscriptPagerAction::Close,
        KeyCode::Char('o') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            TranscriptPagerAction::Close
        }
        KeyCode::Up | KeyCode::Char('k') => {
            scroll_by(pager, -1);
            TranscriptPagerAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            scroll_by(pager, 1);
            TranscriptPagerAction::None
        }
        KeyCode::PageUp | KeyCode::Char('b') => {
            scroll_by(pager, -(pager.page_step() as isize));
            TranscriptPagerAction::None
        }
        KeyCode::PageDown | KeyCode::Char('f') | KeyCode::Char(' ') => {
            scroll_by(pager, pager.page_step() as isize);
            TranscriptPagerAction::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            pager.anchor_kind = AnchorKind::Head;
            pager.scroll_from_top = 0;
            TranscriptPagerAction::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            pager.anchor_kind = AnchorKind::Tail;
            pager.scroll_from_bottom = 0;
            TranscriptPagerAction::None
        }
        _ => TranscriptPagerAction::None,
    }
}

fn apply_search_input_key(
    pager: &mut TranscriptPagerState,
    key_event: &KeyEvent,
) -> TranscriptPagerAction {
    match key_event.code {
        KeyCode::Esc => {
            pager.search_active = false;
            TranscriptPagerAction::None
        }
        KeyCode::Enter => {
            pager.search_active = false;
            repeat_search(pager, SearchDirection::Forward)
        }
        KeyCode::Backspace => {
            pager.search_query.pop();
            TranscriptPagerAction::None
        }
        KeyCode::Char(ch)
            if key_event
                .modifiers
                .intersection(KeyModifiers::CONTROL | KeyModifiers::ALT)
                == KeyModifiers::empty() =>
        {
            pager.search_query.push(ch);
            TranscriptPagerAction::None
        }
        _ => TranscriptPagerAction::None,
    }
}

fn repeat_search(
    pager: &mut TranscriptPagerState,
    direction: SearchDirection,
) -> TranscriptPagerAction {
    if pager.search_query.is_empty() {
        pager.search_status = Some("No search query.".to_string());
        return TranscriptPagerAction::None;
    }

    let start = pager
        .search_match_cell
        .unwrap_or_else(|| pager.current_cell_index());
    let include_start = pager.search_match_cell.is_none();
    if let Some(match_cell) = pager.search_match(direction, start, include_start) {
        pager.anchor_kind = AnchorKind::Search;
        pager.search_match_cell = Some(match_cell);
        pager.search_direction = direction;
        pager.search_status = Some(format!(
            "Match in cell {} of {}.",
            match_cell.saturating_add(1),
            pager.source_cell_count
        ));
    } else {
        pager.search_status = Some(format!("No matches for '{}'.", pager.search_query));
    }
    TranscriptPagerAction::None
}

fn scroll_by(pager: &mut TranscriptPagerState, delta: isize) {
    match pager.anchor_kind {
        AnchorKind::Tail => {
            if delta < 0 {
                pager.scroll_from_bottom = pager
                    .scroll_from_bottom
                    .saturating_add(delta.unsigned_abs());
            } else {
                pager.scroll_from_bottom = pager.scroll_from_bottom.saturating_sub(delta as usize);
            }
        }
        AnchorKind::Head => {
            if delta < 0 {
                pager.scroll_from_top = pager.scroll_from_top.saturating_sub(delta.unsigned_abs());
            } else {
                pager.scroll_from_top = pager.scroll_from_top.saturating_add(delta as usize);
            }
        }
        AnchorKind::Search => {
            // Convert the search anchor into a concrete Head offset positioned
            // at the matched cell (its top line), so the delta scrolls relative
            // to the match. Previously this reset to Tail with
            // `scroll_from_bottom = 0`, snapping to the transcript bottom and
            // discarding the search position on the first arrow key.
            //
            // The offset must come from the *actual merged prefix render*, not a
            // sum of per-cell `cell_height`s: `cell_height` gives every cell at
            // least two rows (content + separator) even when the merged render
            // skips empty cells entirely and omits separators between cells that
            // don't need one. Summing it overcounts, so the first arrow key
            // still jumped away from the match whenever an empty (e.g. blank
            // thinking) or no-separator cell preceded it. Rendering the prefix
            // the same way `materialize_head` does yields the exact line offset.
            let match_cell = pager
                .search_match_cell
                .unwrap_or_else(|| pager.source_cell_count.saturating_sub(1));
            let top_line = pager.render_wrapped_range(0, match_cell).len();
            pager.anchor_kind = AnchorKind::Head;
            pager.scroll_from_top = top_line;
            scroll_by(pager, delta);
        }
    }
}

fn plain_text(lines: &[StyledLine]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn last_visible_thinking_block_from_cells(
    cells: &[TranscriptCell],
    _count: usize,
) -> Option<(String, usize)> {
    use orbcode_protocol::{MessageRole, TranscriptBlock};
    for cell in cells.iter().rev() {
        if let TranscriptCell::Message(message) = cell {
            match message.role {
                MessageRole::Assistant => {
                    for (index, block) in message.blocks.iter().enumerate().rev() {
                        if matches!(block, TranscriptBlock::Thinking { .. }) {
                            return Some((message.id.clone(), index));
                        }
                    }
                }
                MessageRole::User => {
                    let has_tool_result = message
                        .blocks
                        .iter()
                        .any(|b| matches!(b, TranscriptBlock::ToolResult { .. }));
                    if !has_tool_result {
                        return None;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

pub(crate) fn pager_content_height(area: Rect) -> usize {
    area.height as usize
}

pub(crate) fn draw_transcript_pager_overlay(
    frame: &mut crate::custom_terminal::Frame,
    pager: &TranscriptPagerState,
    area: Rect,
) {
    frame.render_widget(Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let content_height = pager_content_height(area);
    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: content_height as u16,
    };
    frame.render_widget(
        Paragraph::new(pager.rendered_window.clone()).wrap(Wrap { trim: false }),
        content_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};

    fn message_cell(role: MessageRole, text: &str) -> TranscriptCell {
        TranscriptCell::Message(TranscriptMessage::new(role, text))
    }

    fn empty_thinking_cell() -> TranscriptCell {
        // An assistant message whose only block is whitespace-only thinking
        // renders to zero lines (see render_message_lines_with_hook_progress).
        TranscriptCell::Message(TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: "   ".to_string(),
                signature: None,
            }],
        ))
    }

    #[test]
    fn arrow_after_search_match_keeps_match_visible_past_empty_cell() {
        // Regression: an empty (whitespace-thinking) cell before the match made
        // the `cell_height`-summed offset overshoot, so the first arrow key
        // scrolled past the match. The offset now comes from the merged render.
        let cells = vec![
            message_cell(MessageRole::User, "alpha alpha alpha"),
            empty_thinking_cell(),
            message_cell(MessageRole::User, "TARGETNEEDLE match line"),
        ];
        let mut pager = TranscriptPagerState::new(
            1,
            cells,
            40,
            4,
            std::path::PathBuf::new(),
            "test-model".to_string(),
        );
        pager.anchor_kind = AnchorKind::Search;
        pager.search_match_cell = Some(2);
        pager.search_query = "TARGETNEEDLE".to_string();
        pager.materialize();
        assert!(
            plain_text(&pager.rendered_window).contains("TARGETNEEDLE"),
            "search materialize should show the match"
        );

        // First arrow-up converts the Search anchor to a concrete Head offset.
        scroll_by(&mut pager, -1);
        pager.materialize();
        assert!(
            plain_text(&pager.rendered_window).contains("TARGETNEEDLE"),
            "the match must remain visible after the first arrow key, not snap away: {:?}",
            plain_text(&pager.rendered_window)
        );
    }

    #[test]
    fn materialize_tail_fills_viewport_past_empty_cells() {
        // Regression: `cell_height` counts each empty (blank-thinking) cell as
        // ~2 rows even though it renders to zero lines, so the load estimate
        // stopped early and left the viewport short/blank when several empty
        // cells sit at the tail (beyond the 8-row overscan). The correction
        // pass must keep loading content cells until the real render fills the
        // viewport.
        const VIEWPORT: usize = 6;
        let mut cells = Vec::new();
        for i in 0..12 {
            cells.push(message_cell(
                MessageRole::User,
                &format!("content line {i}"),
            ));
        }
        // Several empty cells at the tail inflate the cell_height estimate.
        for _ in 0..6 {
            cells.push(empty_thinking_cell());
        }
        let cell_count = cells.len();
        let mut pager = TranscriptPagerState::new(
            1,
            cells,
            40,
            VIEWPORT,
            std::path::PathBuf::new(),
            "test-model".to_string(),
        );
        pager.source_cell_count = cell_count;
        pager.anchor_kind = AnchorKind::Tail;
        pager.materialize();
        assert_eq!(
            pager.rendered_window.len(),
            VIEWPORT,
            "the tail window must fill the viewport even when empty cells precede content: {:?}",
            plain_text(&pager.rendered_window)
        );
    }
}
