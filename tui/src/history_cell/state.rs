use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use orbcode_protocol::TranscriptMessage;
use ratatui::text::Line;

use crate::history_cell::cells::{TranscriptCell, transcript_cells_from_messages};
use crate::history_cell::viewport::TranscriptViewportState;
use crate::render::assistant::render_pending_assistant_lines;
use crate::render::styled_wrap::wrap_styled_lines;
use crate::render::text_utils::{StyledLine, compact_blank_lines, is_blank_line};
use crate::state::TuiState;
use crate::streaming::table_holdback::table_holdback_source_start;
use crate::tui_theme::inactive_style;

const ASSISTANT_STREAM_LIVE_TAIL_LINES: usize = 12;

/// Steady-state pacing for incremental streaming commits: reveal at most this
/// many newly-stable lines into scrollback per draw frame so the commit
/// animates smoothly instead of jumping. Orb Code commits once per frame, so the
/// frame cadence (delta- and `animation_tick`-driven) is the clock — no
/// separate commit timer is needed.
const STREAM_COMMIT_SMOOTH_STEP_LINES: usize = 2;
/// When the stable backlog exceeds this (a burst / instant paste), stop pacing
/// and commit the whole backlog in one frame rather than dripping it slowly.
const STREAM_COMMIT_CATCHUP_BACKLOG_LINES: usize = 24;

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptUiState {
    pub(crate) cells: Vec<TranscriptCell>,
    pub(crate) source_message_count: usize,
    pub(crate) source_signature: u64,
    pub(crate) viewport: TranscriptViewportState,
    pub(crate) render_cache: TranscriptRenderCache,
    pub(crate) emission: HistoryEmissionState,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HistoryEmissionState {
    pub(crate) emitted_cell_count: usize,
    pub(crate) pending_flush_cell_count: Option<usize>,
    pub(crate) emission_width: Option<usize>,
    pub(crate) pending_lines: VecDeque<StyledLine>,
    pub(crate) reflow_pending: bool,
    /// Sticky: a resize rebuild ran while a turn was streaming. The settle
    /// rebuild clears `reflow_pending` mid-stream, but scrollback then holds the
    /// transient stream wrapping (raw lines at the resize-time width). This flag
    /// survives until the stream ends so one final source-backed rebuild can
    /// reconcile scrollback to the finalized transcript.
    pub(crate) reflow_ran_during_stream: bool,
    pub(crate) needs_scrollback_clear: bool,
    pub(crate) assistant_stream_width: Option<usize>,
    pub(crate) assistant_stream_emitted_line_count: usize,
    pub(crate) assistant_stream_pending_line_count: Option<usize>,
    pub(crate) assistant_stream_completed_source: Option<String>,
    pub(crate) assistant_stream_completed_message_id: Option<String>,
    pub(crate) assistant_stream_completed_cell_count: Option<usize>,
    /// Memoizes the last `render_pending_assistant_lines(source, width)` so the
    /// full (growing) streamed markdown is not re-rendered on every frame —
    /// only when the source or width actually changes. Interior mutability lets
    /// the `&self` live-render path populate it. Keyed by `(width, source hash)`.
    pending_render_cache: std::cell::RefCell<PendingAssistantRenderCache>,
}

#[derive(Clone, Debug, Default)]
struct PendingAssistantRenderCache {
    key: Option<(usize, u64)>,
    lines: Vec<StyledLine>,
    #[cfg(test)]
    hits: u64,
    #[cfg(test)]
    misses: u64,
}

impl HistoryEmissionState {
    /// Render `source` at `width`, reusing the cached result when the
    /// `(width, source)` pair is unchanged since the last call.
    pub(crate) fn render_pending_assistant_cached(
        &self,
        source: &str,
        transcript_width: usize,
    ) -> Vec<StyledLine> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(source, &mut hasher);
        let key = (transcript_width, std::hash::Hasher::finish(&hasher));
        // Scope the hit-check borrow so it is released before the miss path
        // re-borrows to populate the cache.
        {
            if let Ok(cache) = self.pending_render_cache.try_borrow()
                && cache.key == Some(key)
            {
                let lines = cache.lines.clone();
                drop(cache);
                #[cfg(test)]
                if let Ok(mut cache) = self.pending_render_cache.try_borrow_mut() {
                    cache.hits += 1;
                }
                return lines;
            }
        }
        let rendered = render_pending_assistant_lines(source, transcript_width);
        if let Ok(mut cache) = self.pending_render_cache.try_borrow_mut() {
            #[cfg(test)]
            {
                cache.misses += 1;
            }
            cache.key = Some(key);
            cache.lines = rendered.clone();
        }
        rendered
    }

    #[cfg(test)]
    pub(crate) fn pending_render_cache_stats(&self) -> (u64, u64) {
        let cache = self.pending_render_cache.borrow();
        (cache.hits, cache.misses)
    }

    fn reset_assistant_stream_history(&mut self) {
        self.assistant_stream_width = None;
        self.assistant_stream_emitted_line_count = 0;
        self.assistant_stream_pending_line_count = None;
        self.assistant_stream_completed_source = None;
        self.assistant_stream_completed_message_id = None;
        self.assistant_stream_completed_cell_count = None;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptRenderCache {
    pub(crate) key: Option<TranscriptRenderCacheKey>,
    pub(crate) lines: Vec<StyledLine>,
    pub(crate) visual_lines: Vec<StyledLine>,
    #[cfg(test)]
    pub(crate) hits: u64,
    #[cfg(test)]
    pub(crate) misses: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptRenderCacheKey {
    pub(crate) transcript_width: usize,
    pub(crate) static_cell_count: usize,
    pub(crate) model_signature: u64,
    pub(crate) cwd_signature: u64,
    pub(crate) source_signature: u64,
    pub(crate) active_thinking_visible: bool,
    pub(crate) blink_visible: Option<bool>,
}

impl TranscriptUiState {
    pub(crate) fn from_messages(messages: &[TranscriptMessage], cwd: &Path) -> Self {
        let mut state = Self::default();
        state.refresh_from_messages(messages, cwd);
        state
    }

    pub(crate) fn refresh_from_messages(&mut self, messages: &[TranscriptMessage], cwd: &Path) {
        let source_signature = transcript_source_signature(messages, cwd);
        if self.source_signature == source_signature && self.source_message_count == messages.len()
        {
            return;
        }
        self.cells = transcript_cells_from_messages(messages, cwd);
        self.source_message_count = messages.len();
        self.source_signature = source_signature;
        self.render_cache.clear();
    }
}

impl TuiState {
    pub(crate) fn queue_existing_history_flush(&mut self) {
        if !self.messages.is_empty() {
            self.pending_history_flush = true;
        }
    }

    pub(crate) fn push_message_and_flush_history(&mut self, message: TranscriptMessage) {
        self.clear_latest_message_focus();
        self.messages.push(message);
        self.pending_history_flush = true;
        self.prune_completed_live_tool_activity();
    }

    pub(crate) fn check_width_reflow(&mut self, transcript_width: usize) {
        let emission = &mut self.transcript_ui.emission;
        if emission.emitted_cell_count > 0
            && emission
                .emission_width
                .is_some_and(|w| w != transcript_width)
        {
            // Defer the committed-history rebuild until the resize settles
            // (driven by the main loop's resize-settle deadline). Rebuilding on
            // every frame would flash on a drag and, mid-stream, disrupt the live
            // render. The settle rebuild re-emits the whole transcript (banner
            // included) from source once. Reset the assistant-stream row
            // accounting so the live tail re-renders cleanly at the new width.
            emission.reflow_pending = true;
            emission.assistant_stream_width = Some(transcript_width);
            emission.assistant_stream_emitted_line_count = 0;
            emission.assistant_stream_pending_line_count = None;
        }
    }

    pub(crate) fn finish_stream_reflow_if_needed(&mut self) {
        if self.request_in_flight {
            return;
        }
        // Rebuild once at turn end when a reflow is still deferred, OR when a
        // reflow ran mid-stream (sticky): the latter means scrollback holds the
        // transient stream wrapping, so re-emit from the finalized source to
        // reconcile it. Clearing the sticky flag prevents repeats.
        let emission = &self.transcript_ui.emission;
        if emission.reflow_pending || emission.reflow_ran_during_stream {
            self.rebuild_committed_history_from_source();
            self.transcript_ui.emission.reflow_ran_during_stream = false;
        }
    }

    /// Full source-of-truth scrollback rebuild: purge native scrollback and
    /// re-emit every committed history cell from source at the current size.
    ///
    /// This is the codex-style resize repair — it replaces the fragile in-place
    /// partial repaint of rows above the viewport (which dropped the earliest
    /// rows, e.g. the intro banner, and fought tmux's own reflow). Because it
    /// re-emits the whole transcript from source, the banner is always restored.
    /// Used both mid-stream (on resize-settle) and at turn end.
    pub(crate) fn rebuild_committed_history_from_source(&mut self) {
        let streaming = self.request_in_flight;
        let emission = &mut self.transcript_ui.emission;
        emission.pending_lines.clear();
        emission.emitted_cell_count = 0;
        emission.pending_flush_cell_count = None;
        emission.reset_assistant_stream_history();
        emission.reflow_pending = false;
        emission.needs_scrollback_clear = true;
        // A rebuild during a streaming turn re-emits only the transient stream
        // wrapping present now; remember it so a final source-backed rebuild runs
        // at turn end (see finish_stream_reflow_if_needed).
        if streaming {
            emission.reflow_ran_during_stream = true;
        }
        self.history_flushed_message_count = 0;
        self.pending_history_flush = true;
    }

    /// Defer a full source rebuild until the resize settles (used while a turn is
    /// streaming, where purging every SIGWINCH frame would flash and disrupt the
    /// live render). The viewport still repositions each frame; rows above it may
    /// show a transient gap until the settle rebuild fires.
    pub(crate) fn mark_resize_reflow_pending(&mut self) {
        self.transcript_ui.emission.reflow_pending = true;
    }

    /// Whether a committed-history rebuild is currently deferred.
    pub(crate) fn resize_reflow_pending(&self) -> bool {
        self.transcript_ui.emission.reflow_pending
    }

    pub(crate) fn reset_assistant_stream_history(&mut self) {
        self.transcript_ui.emission.reset_assistant_stream_history();
    }

    pub(crate) fn reset_assistant_stream_history_for_reflow(&mut self) {
        let emission = &mut self.transcript_ui.emission;
        emission.pending_lines.clear();
        emission.emitted_cell_count = 0;
        emission.pending_flush_cell_count = None;
        emission.reset_assistant_stream_history();
        emission.needs_scrollback_clear = true;
        self.transcript_ui.render_cache.clear();
        self.history_flushed_message_count = 0;
        self.pending_history_flush = true;
    }

    pub(crate) fn assistant_stream_history_started(&self) -> bool {
        let emission = &self.transcript_ui.emission;
        emission.assistant_stream_emitted_line_count > 0
            || emission.assistant_stream_pending_line_count.is_some()
            || emission.assistant_stream_completed_source.is_some()
    }

    pub(crate) fn complete_assistant_stream_history_from_source(
        &mut self,
        source: String,
        message_id: String,
    ) {
        let emission = &mut self.transcript_ui.emission;
        emission.assistant_stream_completed_source = Some(source);
        emission.assistant_stream_completed_message_id = Some(message_id);
        self.pending_history_flush = true;
    }

    pub(crate) fn prepare_pending_history_emission(
        &mut self,
        transcript_width: usize,
        _terminal_height: u16,
    ) -> bool {
        self.check_width_reflow(transcript_width);

        let pending_history = self.pending_history_flush
            || !self.transcript_ui.emission.pending_lines.is_empty()
            || self.pending_assistant_history_ready(transcript_width);
        if !pending_history {
            return false;
        }

        self.refresh_transcript_ui_state();
        if self.transcript_ui.emission.pending_lines.is_empty()
            && self
                .transcript_ui
                .emission
                .pending_flush_cell_count
                .is_none()
            && self.transcript_ui.emission.emitted_cell_count != self.history_flushed_message_count
        {
            self.transcript_ui.emission.emitted_cell_count = self.history_flushed_message_count;
        }

        let cells_with_ids =
            self.committed_history_cells_for_emission_with_message_ids(transcript_width);
        let cells: Vec<Vec<StyledLine>> = cells_with_ids
            .iter()
            .map(|(lines, _)| lines.clone())
            .collect();
        let flush_upto = cells.len();
        let completed_stream_message_id = self
            .transcript_ui
            .emission
            .assistant_stream_completed_message_id
            .clone();
        let completed_stream_cell_indices = completed_stream_message_id
            .as_deref()
            .map(|message_id| {
                cells_with_ids
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (_, cell_message_id))| {
                        cell_message_id
                            .as_deref()
                            .is_some_and(|cell_message_id| cell_message_id == message_id)
                            .then_some(index)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let stream_completed_with_emitted_prefix = self
            .transcript_ui
            .emission
            .assistant_stream_completed_source
            .is_some()
            && self
                .transcript_ui
                .emission
                .assistant_stream_emitted_line_count
                > 0;
        let committed_flush_upto =
            if stream_completed_with_emitted_prefix && completed_stream_cell_indices.is_empty() {
                flush_upto.saturating_sub(1)
            } else {
                flush_upto
            };
        {
            let emission = &mut self.transcript_ui.emission;
            if emission.emission_width != Some(transcript_width) {
                if emission.assistant_stream_emitted_line_count > 0 {
                    emission.pending_lines.clear();
                    emission.emitted_cell_count = 0;
                    emission.pending_flush_cell_count = None;
                    emission.reset_assistant_stream_history();
                    emission.needs_scrollback_clear = true;
                    self.history_flushed_message_count = 0;
                    self.pending_history_flush = true;
                    return true;
                }
                emission.emission_width = Some(transcript_width);
                emission.emitted_cell_count = emission.emitted_cell_count.min(committed_flush_upto);
                emission.pending_flush_cell_count = None;
                emission.pending_lines.clear();
            }
        }

        let start = {
            let emission = &self.transcript_ui.emission;
            emission
                .pending_flush_cell_count
                .unwrap_or(emission.emitted_cell_count)
                .min(committed_flush_upto)
        };
        if start < committed_flush_upto {
            let lines = if stream_completed_with_emitted_prefix
                && !completed_stream_cell_indices.is_empty()
            {
                history_lines_for_cell_range_excluding_indices(
                    &cells,
                    start,
                    committed_flush_upto,
                    &completed_stream_cell_indices,
                )
            } else {
                history_lines_for_cell_range(&cells, start, committed_flush_upto)
            };
            let emission = &mut self.transcript_ui.emission;
            if !lines.is_empty() {
                emission.pending_lines.extend(lines);
                emission.pending_flush_cell_count = Some(committed_flush_upto);
                self.history_flushed_message_count = committed_flush_upto;
                self.pending_history_flush = false;
            }
        }

        self.prepare_pending_assistant_history_emission(transcript_width, flush_upto);

        let has_pending_lines = !self.transcript_ui.emission.pending_lines.is_empty();
        if has_pending_lines
            && let Some(flush_upto) = self.transcript_ui.emission.pending_flush_cell_count
        {
            self.history_flushed_message_count = flush_upto;
        }
        if !has_pending_lines {
            self.finalize_completed_assistant_stream_without_pending_lines();
        }
        if !has_pending_lines {
            self.history_flushed_message_count = self
                .transcript_ui
                .emission
                .emitted_cell_count
                .min(committed_flush_upto);
            self.pending_history_flush = false;
        }
        self.prune_completed_live_tool_activity();
        has_pending_lines
    }

    pub(crate) fn commit_history_flush(&mut self) {
        if let Some(flush_upto) = self.transcript_ui.emission.pending_flush_cell_count.take() {
            self.transcript_ui.emission.emitted_cell_count = flush_upto;
            self.history_flushed_message_count = flush_upto;
            self.clear_transcript_bottom_pin_sticky();
            self.transcript_ui.render_cache.clear();
        }
        if let Some(line_count) = self
            .transcript_ui
            .emission
            .assistant_stream_pending_line_count
            .take()
        {
            self.transcript_ui
                .emission
                .assistant_stream_emitted_line_count = line_count;
        }
        if let Some(cell_count) = self
            .transcript_ui
            .emission
            .assistant_stream_completed_cell_count
            .take()
        {
            self.transcript_ui.emission.emitted_cell_count = cell_count;
            self.history_flushed_message_count = cell_count;
            self.transcript_ui.emission.reset_assistant_stream_history();
            self.clear_transcript_bottom_pin_sticky();
            self.transcript_ui.render_cache.clear();
        }
        self.pending_history_flush = false;
        self.prune_completed_live_tool_activity();
    }

    pub(crate) fn defer_pending_history_flush(&mut self) {
        self.history_flushed_message_count = self.transcript_ui.emission.emitted_cell_count;
        self.pending_history_flush = !self.transcript_ui.emission.pending_lines.is_empty();
        self.prune_completed_live_tool_activity();
    }

    pub(crate) fn pending_assistant_live_lines(&self, transcript_width: usize) -> Vec<StyledLine> {
        if self.pending_assistant.is_empty() {
            return Vec::new();
        }
        let rendered = self
            .transcript_ui
            .emission
            .render_pending_assistant_cached(&self.pending_assistant, transcript_width);
        let start = self
            .transcript_ui
            .emission
            .assistant_stream_pending_line_count
            .unwrap_or(
                self.transcript_ui
                    .emission
                    .assistant_stream_emitted_line_count,
            )
            .min(rendered.len());
        rendered.into_iter().skip(start).collect()
    }

    pub(crate) fn pending_assistant_history_debug_counts(
        &self,
        transcript_width: usize,
    ) -> (usize, usize) {
        let rendered = self
            .pending_assistant_history_source()
            .map(|source| render_pending_assistant_lines(source, transcript_width).len())
            .unwrap_or_default();
        let live_tail = if self.pending_assistant.is_empty() {
            0
        } else {
            self.pending_assistant_live_lines(transcript_width).len()
        };
        (rendered, live_tail)
    }

    fn pending_assistant_history_source(&self) -> Option<&str> {
        // While a width reflow is deferred, the old-width assistant lines still
        // sit in physical scrollback until the settle rebuild purges them, but
        // `check_width_reflow` has already reset the emitted-line counter to zero
        // (High/Medium #3). Committing from line zero now would re-append the
        // already-emitted prefix at the new width, duplicating it before settle.
        // Hold all assistant commit until the settle rebuild re-emits from source.
        if self.transcript_ui.emission.reflow_pending {
            return None;
        }
        if let Some(source) = self
            .transcript_ui
            .emission
            .assistant_stream_completed_source
            .as_deref()
        {
            return Some(source);
        }
        if self.streaming_incremental_commit_active() {
            return Some(self.pending_assistant.as_str());
        }
        None
    }

    fn pending_assistant_history_ready(&self, _transcript_width: usize) -> bool {
        self.pending_assistant_history_source().is_some()
    }

    /// Whether the live streaming assistant text should be committed to
    /// scrollback incrementally this frame. This is the default (and only)
    /// behavior: an in-flight turn with non-empty pending text streams into
    /// native scrollback. The `emitted_cell_count > 0` condition is the banner
    /// atomicity gate — a streaming flush must never be the `first_history_flush`
    /// that carries the intro banner, so incremental commit waits until the
    /// first history cell (banner / user message) has already been committed.
    fn streaming_incremental_commit_active(&self) -> bool {
        self.request_in_flight
            && !self.pending_assistant.is_empty()
            && self.transcript_ui.emission.emitted_cell_count > 0
            // A completed thinking block is committed to scrollback ahead of the
            // final answer at turn completion; committing the assistant answer
            // incrementally before that would interleave thinking inside the
            // answer (High #3). Hold incremental commit until the thinking is
            // materialized (i.e. `active_thinking` no longer holds it).
            && !self.has_pending_completed_thinking()
    }

    fn prepare_pending_assistant_history_emission(
        &mut self,
        transcript_width: usize,
        completed_cell_count: usize,
    ) {
        let Some(source) = self.pending_assistant_history_source().map(str::to_owned) else {
            return;
        };
        let rendered = self
            .transcript_ui
            .emission
            .render_pending_assistant_cached(&source, transcript_width);
        if rendered.is_empty() {
            return;
        }

        let emission = &mut self.transcript_ui.emission;
        if emission.assistant_stream_width != Some(transcript_width) {
            if emission.assistant_stream_emitted_line_count > 0 {
                emission.pending_lines.clear();
                emission.emitted_cell_count = 0;
                emission.pending_flush_cell_count = None;
                emission.reset_assistant_stream_history();
                emission.needs_scrollback_clear = true;
                self.history_flushed_message_count = 0;
                self.pending_history_flush = true;
                return;
            }
            emission.assistant_stream_width = Some(transcript_width);
            emission.assistant_stream_pending_line_count = None;
        }

        let completed = emission.assistant_stream_completed_source.is_some();
        let target = if completed {
            rendered.len()
        } else {
            let tail_target = rendered
                .len()
                .saturating_sub(ASSISTANT_STREAM_LIVE_TAIL_LINES);
            // Withhold an in-progress markdown table (and anything after it) in
            // the mutable tail so its rows are never committed to scrollback
            // before the table finalizes and stops reflowing. Committed lines
            // are always sliced from the full-source render (`rendered`); the
            // prefix render is only used to count stable lines before the table.
            match table_holdback_source_start(&source) {
                Some(0) => 0,
                Some(table_start) => {
                    let stable_prefix_len =
                        render_pending_assistant_lines(&source[..table_start], transcript_width)
                            .len();
                    tail_target.min(stable_prefix_len)
                }
                None => tail_target,
            }
        };
        let start = emission
            .assistant_stream_pending_line_count
            .unwrap_or(emission.assistant_stream_emitted_line_count)
            .min(target);
        // Frame-driven pacing (Smooth / CatchUp) while streaming: reveal only a
        // few newly-stable lines per frame at steady state so the commit
        // animates, but commit the whole backlog at once when it is large (a
        // burst) to avoid a slow drip. Completion commits everything unpaced.
        let target = if completed {
            target
        } else {
            let available = target.saturating_sub(start);
            if available > STREAM_COMMIT_CATCHUP_BACKLOG_LINES {
                target
            } else {
                start + available.min(STREAM_COMMIT_SMOOTH_STEP_LINES)
            }
        };
        if start >= target {
            if completed && emission.assistant_stream_emitted_line_count >= rendered.len() {
                emission.assistant_stream_completed_cell_count = Some(completed_cell_count);
            }
            return;
        }

        if start == 0
            && !emission.pending_lines.is_empty()
            && emission
                .pending_lines
                .back()
                .is_some_and(|line| !is_blank_line(line))
        {
            emission.pending_lines.push_back(Line::default());
        }
        emission
            .pending_lines
            .extend(rendered.iter().skip(start).take(target - start).cloned());
        emission.assistant_stream_pending_line_count = Some(target);
        if completed && target == rendered.len() {
            emission.assistant_stream_completed_cell_count = Some(completed_cell_count);
        }
        self.pending_history_flush = false;
    }

    fn finalize_completed_assistant_stream_without_pending_lines(&mut self) {
        let Some(cell_count) = self
            .transcript_ui
            .emission
            .assistant_stream_completed_cell_count
            .take()
        else {
            return;
        };
        self.transcript_ui.emission.emitted_cell_count = cell_count;
        self.history_flushed_message_count = cell_count;
        self.transcript_ui.emission.reset_assistant_stream_history();
        self.clear_transcript_bottom_pin_sticky();
        self.transcript_ui.render_cache.clear();
    }

    pub(crate) fn take_pending_history_lines_for_emission(
        &mut self,
        transcript_width: usize,
        terminal_height: u16,
    ) -> Vec<StyledLine> {
        self.prepare_pending_history_emission(transcript_width, terminal_height);
        self.transcript_ui
            .emission
            .pending_lines
            .drain(..)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn take_history_lines(
        &mut self,
        transcript_width: usize,
        terminal_height: u16,
    ) -> Vec<StyledLine> {
        let lines = self.take_pending_history_lines_for_emission(transcript_width, terminal_height);
        if !lines.is_empty() {
            self.commit_history_flush();
        }
        lines
    }
}

impl TranscriptRenderCache {
    pub(crate) fn is_current(&mut self, key: &TranscriptRenderCacheKey) -> bool {
        if self.key.as_ref() == Some(key) {
            #[cfg(test)]
            {
                self.hits += 1;
            }
            return true;
        }

        #[cfg(test)]
        {
            self.misses += 1;
        }
        false
    }

    pub(crate) fn store(&mut self, key: TranscriptRenderCacheKey, lines: Vec<StyledLine>) {
        let visual_lines = wrap_styled_lines(&lines, key.transcript_width.max(1));
        self.key = Some(key);
        self.lines = lines;
        self.visual_lines = visual_lines;
    }

    pub(crate) fn clear(&mut self) {
        self.key = None;
        self.lines.clear();
        self.visual_lines.clear();
    }
}

fn transcript_source_signature(messages: &[TranscriptMessage], cwd: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    cwd.to_string_lossy().hash(&mut hasher);
    messages.len().hash(&mut hasher);
    for message in messages {
        message.id.hash(&mut hasher);
        message.content.len().hash(&mut hasher);
        message.blocks.len().hash(&mut hasher);
        message.stop_reason.hash(&mut hasher);
        message.usage.is_some().hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn hash_string_value(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn flatten_transcript_cells(cells: &[Vec<StyledLine>]) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    let mut previous_non_empty_cell: Option<&Vec<StyledLine>> = None;
    let non_empty_cell_count = cells.iter().filter(|cell| !cell.is_empty()).count();
    let preserve_leading_blank = cells
        .iter()
        .find(|cell| !cell.is_empty())
        .and_then(|cell| cell.first())
        .is_some_and(is_blank_line);
    let preserve_trailing_blank = non_empty_cell_count == 1
        && cells
            .iter()
            .find(|cell| !cell.is_empty())
            .and_then(|cell| cell.last())
            .is_some_and(is_blank_line);
    for cell in cells {
        if cell.is_empty() {
            continue;
        }
        if previous_non_empty_cell.is_some_and(|previous| cell_separator_needed(previous, cell)) {
            lines.push(Line::default());
        }
        lines.extend(cell.iter().cloned());
        previous_non_empty_cell = Some(cell);
    }
    while lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    let mut compacted = compact_blank_lines(lines);
    if preserve_leading_blank && !compacted.is_empty() {
        compacted.insert(0, Line::default());
    }
    if preserve_trailing_blank && !compacted.is_empty() {
        compacted.push(Line::default());
    }
    compacted
}

pub(crate) fn history_lines_for_cell_range(
    cells: &[Vec<StyledLine>],
    start: usize,
    end: usize,
) -> Vec<StyledLine> {
    history_lines_for_cell_range_filter(cells, start, end, |_| true)
}

pub(crate) fn history_lines_for_cell_range_excluding_indices(
    cells: &[Vec<StyledLine>],
    start: usize,
    end: usize,
    excluded_indices: &[usize],
) -> Vec<StyledLine> {
    history_lines_for_cell_range_filter(cells, start, end, |index| {
        !excluded_indices.contains(&index)
    })
}

fn history_lines_for_cell_range_filter(
    cells: &[Vec<StyledLine>],
    start: usize,
    end: usize,
    include_index: impl Fn(usize) -> bool,
) -> Vec<StyledLine> {
    let start = start.min(cells.len());
    let end = end.min(cells.len());
    let mut lines = Vec::new();
    let mut previous_non_empty_cell = cells[..start]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, cell)| (include_index(index) && !cell.is_empty()).then_some(cell));
    let has_previous_non_empty_cell = previous_non_empty_cell.is_some();
    let mut starts_with_separator = false;
    let preserve_leading_blank = !has_previous_non_empty_cell
        && cells
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
            .filter(|(index, _)| include_index(*index))
            .map(|(_, cell)| cell)
            .find(|cell| !cell.is_empty())
            .and_then(|cell| cell.first())
            .is_some_and(is_blank_line);
    for (index, cell) in cells
        .iter()
        .enumerate()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        if !include_index(index) {
            continue;
        }
        if cell.is_empty() {
            continue;
        }
        if previous_non_empty_cell.is_some_and(|previous| cell_separator_needed(previous, cell)) {
            if lines.is_empty() {
                starts_with_separator = true;
            }
            lines.push(Line::default());
        }
        lines.extend(cell.iter().cloned());
        previous_non_empty_cell = Some(cell);
    }
    while lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    let mut compacted = compact_blank_lines(lines);
    if (starts_with_separator || preserve_leading_blank) && !compacted.is_empty() {
        compacted.insert(0, Line::default());
    }
    compacted
}

fn cell_separator_needed(previous: &[StyledLine], next: &[StyledLine]) -> bool {
    !((cell_is_local_note(previous) && !cell_is_user_prompt(next))
        || (cell_is_assistant_text(previous) && cell_is_collapsed_activity_group(next)))
}

fn cell_is_local_note(cell: &[StyledLine]) -> bool {
    cell.first().is_some_and(|line| {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .starts_with("✻ ")
    })
}

fn cell_is_assistant_text(cell: &[StyledLine]) -> bool {
    let Some(first_line) = cell.first() else {
        return false;
    };
    if first_line.spans.len() < 2 {
        return false;
    }
    if first_line.spans[0].content.as_ref() != "●"
        || first_line.spans[0].style != inactive_style()
        || first_line.spans[1].content.as_ref() != " "
    {
        return false;
    }

    !cell.get(1).is_some_and(|line| {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .starts_with("  └ ")
    })
}

fn cell_is_collapsed_activity_group(cell: &[StyledLine]) -> bool {
    cell.first().is_some_and(|line| {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .contains("(ctrl+o to expand)")
    })
}

fn cell_is_user_prompt(cell: &[StyledLine]) -> bool {
    cell.first().is_some_and(|line| {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .starts_with("› ")
    })
}
