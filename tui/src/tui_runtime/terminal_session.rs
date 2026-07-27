use std::fmt;
use std::io::{self, Write, stdout};
use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use crossterm::{
    Command,
    cursor::{MoveDown, MoveTo, MoveToColumn, RestorePosition, SavePosition, SetCursorStyle},
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind,
        poll as poll_terminal_event, read as read_terminal_event,
    },
    execute, queue,
    style::{Attribute, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{
        Clear, ClearType, DisableLineWrap, EnableLineWrap, EnterAlternateScreen,
        LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::Size,
    style::{Color, Modifier, Style},
};
use tokio::sync::mpsc;
use tokio::time::Duration;

use orbcode_app_server_client::{
    AppClient, McpResourceSlashSuggestion, McpServerSlashSuggestion, McpSlashSuggestionCatalog,
    McpToolSlashSuggestion,
};
use orbcode_protocol::StreamEvent;

use crate::chat::stream_events::mark_redraw;
use crate::commands::async_local::LocalCommandEnvelope;
use crate::custom_terminal::Terminal;
use crate::external_editor::open_file_in_editor;
use crate::render::styled_wrap::wrap_styled_lines;
use crate::render::text_utils::{StyledLine, styled_line_display_width};
use crate::state::TuiState;
use ratatui::layout::{Position, Rect};

pub(crate) fn spawn_terminal_event_reader() -> (
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    tokio::task::JoinHandle<()>,
    mpsc::UnboundedReceiver<io::Result<Event>>,
) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let shutdown_reader = Arc::clone(&shutdown);
    let paused_reader = Arc::clone(&paused);
    let reader = tokio::task::spawn_blocking(move || {
        while !shutdown_reader.load(Ordering::SeqCst) {
            if paused_reader.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(16));
                continue;
            }
            match poll_terminal_event(Duration::from_millis(16)) {
                Ok(true) => loop {
                    if sender.send(read_terminal_event()).is_err() {
                        return;
                    }
                    match poll_terminal_event(Duration::ZERO) {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            break;
                        }
                    }
                },
                Ok(false) => {}
                Err(error) => {
                    if sender.send(Err(error)).is_err() {
                        return;
                    }
                }
            }
        }
    });
    (shutdown, paused, reader, receiver)
}

pub(crate) async fn handle_terminal_event(
    state: &mut TuiState,
    app_server: &AppClient,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    terminal_reader_paused: &AtomicBool,
    terminal_event: io::Result<Event>,
    turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    local_command_tx: &mpsc::UnboundedSender<LocalCommandEnvelope>,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) -> Result<bool> {
    match terminal_event {
        Ok(Event::Key(key_event)) if key_event.kind == KeyEventKind::Press => {
            if !state
                .handle_key(app_server, key_event, turn_events, local_command_tx)
                .await?
            {
                return Ok(false);
            }
            if let Some(request) = state.take_external_editor_request() {
                let result = open_file_in_editor(terminal, terminal_reader_paused, &request.path);
                let needs_keybinding_reload = state.report_external_editor_result(request, result);
                if needs_keybinding_reload {
                    crate::keybindings::reload_keybindings_global(app_server);
                }
            }
            refresh_mcp_slash_suggestions_if_needed(
                state,
                app_server,
                needs_redraw,
                redraw_reasons,
            )
            .await;
            mark_redraw(needs_redraw, redraw_reasons, "key_event");
        }
        Ok(Event::Paste(text)) => {
            state.clear_input_selection();
            state.insert_paste_text(&text);
            state.prompt_history_index = None;
            refresh_mcp_slash_suggestions_if_needed(
                state,
                app_server,
                needs_redraw,
                redraw_reasons,
            )
            .await;
            mark_redraw(needs_redraw, redraw_reasons, "paste_event");
        }
        Ok(Event::Mouse(mouse_event)) => {
            if state.handle_mouse(mouse_event) {
                mark_redraw(needs_redraw, redraw_reasons, "mouse_event");
            }
        }
        Ok(Event::Resize(_, _)) => {
            mark_redraw(needs_redraw, redraw_reasons, "resize_event");
        }
        Ok(_) => {
            mark_redraw(needs_redraw, redraw_reasons, "terminal_event");
        }
        Err(error) => {
            state.set_status_line(format!("Terminal error: {error}"));
            mark_redraw(needs_redraw, redraw_reasons, "terminal_error");
        }
    }

    Ok(true)
}

async fn refresh_mcp_slash_suggestions_if_needed(
    state: &mut TuiState,
    app_server: &AppClient,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) {
    let key = state.mcp_slash_suggestion_refresh_key();
    if key.is_none() || key == state.mcp_slash_suggestion_refresh_key {
        return;
    }
    state.mcp_slash_suggestion_refresh_key = key;
    let catalog = match app_server.mcp_slash_suggestions().await {
        Ok(value) => parse_mcp_slash_suggestion_catalog(&value),
        Err(_) => McpSlashSuggestionCatalog::default(),
    };
    state.update_mcp_slash_suggestions(catalog);
    mark_redraw(needs_redraw, redraw_reasons, "mcp_slash_suggestions");
}

fn parse_mcp_slash_suggestion_catalog(value: &serde_json::Value) -> McpSlashSuggestionCatalog {
    let servers = value["servers"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(McpServerSlashSuggestion {
                        id: s["id"].as_str()?.to_string(),
                        summary: s["summary"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let tools = value["tools"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    Some(McpToolSlashSuggestion {
                        server_id: t["server_id"].as_str()?.to_string(),
                        name: t["name"].as_str()?.to_string(),
                        provider_name: t["provider_name"].as_str().unwrap_or("").to_string(),
                        description: t["description"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let resources = value["resources"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    Some(McpResourceSlashSuggestion {
                        server_id: r["server_id"].as_str()?.to_string(),
                        uri: r["uri"].as_str()?.to_string(),
                        name: r["name"].as_str().unwrap_or("").to_string(),
                        description: r["description"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    McpSlashSuggestionCatalog {
        servers,
        tools,
        resources,
    }
}

pub(crate) fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnableBracketedPaste)?;
    let cursor_y = crossterm::cursor::position().map(|(_, y)| y).unwrap_or(0);
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::with_options(backend)?;
    let size = terminal.size()?;
    terminal.set_viewport_area(Rect::new(0, cursor_y, size.width, 1));
    terminal.clear()?;
    Ok(terminal)
}

pub(crate) fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    // Best-effort, finally-style cleanup (Low #1): attempt EVERY restore step
    // even if an earlier one errors, so a single failure cannot strand the
    // terminal in raw mode, with a hidden cursor, or bracketed paste enabled. The
    // first error is returned after all steps have been attempted.
    //
    // The scroll region is intentionally NOT reset here: `ResetScrollRegion`
    // (`ESC[r` / DECSTBM) homes the cursor to (0,0), and `restore_terminal` runs
    // AFTER `prepare_terminal_for_cli_output` has already positioned the cursor
    // at the cleared live-viewport row. Homing it here would make the CLI resume
    // hint print at the top of the screen, stranding the transcript below it. The
    // scroll region is already reset defensively inside
    // `prepare_terminal_for_cli_output`, BEFORE its clear repositions the cursor.
    let results: [Result<()>; 4] = [
        execute!(terminal.backend_mut(), SetCursorStyle::DefaultUserShape).map_err(Into::into),
        execute!(terminal.backend_mut(), DisableBracketedPaste).map_err(Into::into),
        terminal.show_cursor().map_err(Into::into),
        disable_raw_mode().map_err(Into::into),
    ];
    results
        .into_iter()
        .find_map(Result::err)
        .map_or(Ok(()), Err)
}

pub(crate) fn prepare_terminal_for_cli_output<B>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    B: Backend + Write,
{
    let size = terminal.size()?;
    if size.height == 0 {
        return Ok(());
    }

    // Reset any lingering scroll region BEFORE the clear. The stable-streaming
    // upward-growth path (`scroll_visible_history_for_viewport_growth`) sets a
    // top-anchored scroll region while moving displaced history into native
    // scrollback; if one is still active here it could confine the `ESC[0J`
    // clear and the shell's subsequent output. `ResetScrollRegion` (`ESC[r`)
    // homes the cursor, so it MUST run before `clear_after_position` repositions
    // the cursor to the live-viewport top — otherwise the cursor (and the CLI
    // resume hint printed after teardown) would be left at the top of the screen,
    // stranding the transcript below it.
    queue!(terminal.backend_mut(), ResetScrollRegion)?;
    let y = terminal.viewport_area.y.min(size.height.saturating_sub(1));
    terminal.clear_after_position(Position { x: 0, y })?;
    Write::flush(terminal.backend_mut())
}

#[cfg(test)]
pub(crate) fn initial_top_viewport_area(size: Size) -> Rect {
    Rect::new(0, 0, size.width, 1)
}

#[derive(Debug, Default)]
pub(crate) struct TranscriptPagerTerminalMode {
    saved_inline_viewport: Option<Rect>,
}

impl TranscriptPagerTerminalMode {
    pub(crate) fn is_active(&self) -> bool {
        self.saved_inline_viewport.is_some()
    }
}

pub(crate) fn sync_transcript_pager_terminal_mode<B>(
    terminal: &mut Terminal<B>,
    pager_open: bool,
    mode: &mut TranscriptPagerTerminalMode,
) -> Result<bool>
where
    B: Backend + Write,
{
    match (pager_open, mode.saved_inline_viewport) {
        (true, None) => {
            let saved = terminal.viewport_area;
            execute!(
                terminal.backend_mut(),
                EnterAlternateScreen,
                Clear(ClearType::All),
                MoveTo(0, 0)
            )?;
            let size = terminal.size()?;
            terminal.set_viewport_area(Rect::new(0, 0, size.width, size.height.max(1)));
            terminal.invalidate_viewport();
            mode.saved_inline_viewport = Some(saved);
            Ok(true)
        }
        (false, Some(saved)) => {
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            let size = terminal.size()?;
            let (restored, _) = resized_inline_viewport(saved, size, saved.height.max(1));
            terminal.set_viewport_area(restored);
            terminal.clear()?;
            terminal.invalidate_viewport();
            mode.saved_inline_viewport = None;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) struct DrawTransactionResult {
    pub viewport_mutated: bool,
    pub history_flushed: bool,
    /// A terminal size change was observed this frame. The main loop uses this
    /// to (re)arm the resize-settle deadline; when it fires, a full
    /// source-of-truth history rebuild runs (see
    /// `TuiState::rebuild_committed_history_from_source`).
    pub resize_observed: bool,
}

impl DrawTransactionResult {
    pub fn terminal_mutated(&self) -> bool {
        self.viewport_mutated || self.history_flushed
    }
}

pub(crate) fn prepare_draw_transaction<B>(
    terminal: &mut Terminal<B>,
    state: &mut TuiState,
    pager_active: bool,
) -> Result<DrawTransactionResult>
where
    B: Backend + Write,
{
    let size = terminal.size()?;
    if crate::terminal_trace::enabled() {
        let (pending_assistant_rendered_lines, live_assistant_tail_lines) =
            state.pending_assistant_history_debug_counts(size.width as usize);
        let emission = &state.transcript_ui.emission;
        crate::terminal_trace::record_event(
            "prepare_draw_transaction_start",
            serde_json::json!({
                "terminal_size": crate::terminal_trace::size(size),
                "viewport_area": crate::terminal_trace::rect(terminal.viewport_area),
                "last_known_cursor_pos": {
                    "x": terminal.last_known_cursor_pos.x,
                    "y": terminal.last_known_cursor_pos.y,
                },
                "pager_active": pager_active,
                "request_in_flight": state.request_in_flight,
                "pending_history_flush": state.pending_history_flush,
                "history_flushed_message_count": state.history_flushed_message_count,
                "message_count": state.messages.len(),
                "pending_assistant_len": state.pending_assistant.chars().count(),
                "emission": {
                    "emitted_cell_count": emission.emitted_cell_count,
                    "pending_flush_cell_count": emission.pending_flush_cell_count,
                    "pending_lines": emission.pending_lines.len(),
                    "reflow_pending": emission.reflow_pending,
                    "needs_scrollback_clear": emission.needs_scrollback_clear,
                    "assistant_stream_width": emission.assistant_stream_width,
                    "assistant_stream_emitted_lines": emission.assistant_stream_emitted_line_count,
                    "assistant_stream_pending_lines": emission.assistant_stream_pending_line_count,
                    "assistant_stream_completed_source_len": emission
                        .assistant_stream_completed_source
                        .as_ref()
                        .map(|source| source.chars().count()),
                    "assistant_stream_completed_message_id": emission
                        .assistant_stream_completed_message_id
                        .as_deref(),
                    "assistant_stream_completed_cell_count": emission
                        .assistant_stream_completed_cell_count,
                    "assistant_stream_rendered_lines": pending_assistant_rendered_lines,
                    "assistant_stream_live_tail_lines": live_assistant_tail_lines,
                },
            }),
        );
    }
    state.finalize_deferred_assistant_message(size.width as usize, size.height);
    state.prune_completed_live_tool_activity();
    let last_size = terminal.last_known_screen_size();
    let width_changed = terminal.viewport_area.width != size.width;
    let height_changed = size.height != last_size.height;
    let resize_observed = !pager_active && (width_changed || height_changed);
    let height_shrank_past_viewport = terminal.viewport_area.bottom() > size.height;

    // Resize repair uses a codex-style full source-of-truth rebuild instead of
    // the legacy in-place partial repaint of rows above the viewport (which
    // dropped the earliest rows — the intro banner — once history exceeded the
    // viewport-top space, and fought tmux's own reflow with absolute-positioned
    // writes). Every resize (idle or streaming) defers the rebuild to a settle
    // deadline owned by the main loop, so a drag that fires many SIGWINCH frames
    // rebuilds once when it stops rather than flashing on every frame. The
    // viewport still repositions each frame below; rows above it may show a
    // transient gap until the settle rebuild re-emits the whole transcript.
    let resize_repaired = if resize_observed && state.transcript_ui.emission.emitted_cell_count > 0
    {
        state.mark_resize_reflow_pending();
        true
    } else {
        false
    };

    // Do not prepare incremental history while the transcript pager owns the
    // terminal (Medium #1). The physical flush is skipped while the pager is open
    // (see `flush_pending_history_to_scrollback` below, gated on `!pager_active`),
    // so preparing here would queue assistant prefix/tail lines that never get
    // emitted, desyncing the queued vs physically-emitted counters. A completion
    // arriving while the pager is open would then see a "started" stream (from
    // the queued-only count) and, on pager close, flush the prefix, the full
    // final cell, and the tail — duplicating output. The pager renders from a
    // freshly-refreshed transcript source of its own, so it needs nothing here.
    if !pager_active {
        state.prepare_pending_history_emission(size.width as usize, size.height);
    }
    let viewport_mutated = if pager_active {
        false
    } else {
        let desired_height = state.desired_viewport_height(size.width, size.height);
        // While a resize is unsettled the rows above the viewport are
        // terminal/tmux-owned until the debounced source rebuild fires. Growing
        // the viewport upward into them would clear them (see
        // `update_inline_viewport`'s `clear_after_position`) and draw the live
        // tail on top, erasing history before the settle rebuild. So cap upward
        // growth during pending-resize frames (a narrow, resize-scoped cap — not
        // the removed global streaming cap); the settle rebuild restores the full
        // live-tail height once the size is stable.
        let resize_settle_pending = resize_observed || state.resize_reflow_pending();
        // In the startup window (before the first flush), non-resize idle upward
        // growth RESERVES terminal-owned rows into native scrollback (pushes
        // history up) instead of capping or clearing it. Gated off on resize
        // frames so the H1 resize-settle cap keeps owning those.
        let reserve_terminal_owned_rows = !resize_settle_pending
            && should_reserve_terminal_owned_rows_for_viewport_growth(
                state,
                height_shrank_past_viewport,
            );
        let viewport_height = if state.rendering_paused() {
            terminal.viewport_area.height
        } else if terminal.viewport_area.height > 1
            && (resize_settle_pending
                || (state.idle_transient_panel_visible_for_width(size.width as usize)
                    && !reserve_terminal_owned_rows))
        {
            viewport_height_without_upward_growth(terminal.viewport_area, size, desired_height)
        } else {
            desired_height
        };
        let pin_idle_viewport_to_bottom = should_pin_idle_viewport_to_bottom(
            state,
            terminal.viewport_area,
            size,
            viewport_height,
        );
        let pin_active_viewport_to_bottom = should_pin_active_viewport_to_bottom(
            state,
            terminal.viewport_area,
            size,
            viewport_height,
        );
        if crate::terminal_trace::enabled() {
            crate::terminal_trace::record_event(
                "viewport_height_decision",
                serde_json::json!({
                    "terminal_size": crate::terminal_trace::size(size),
                    "viewport_area": crate::terminal_trace::rect(terminal.viewport_area),
                    "last_known_cursor_pos": {
                        "x": terminal.last_known_cursor_pos.x,
                        "y": terminal.last_known_cursor_pos.y,
                    },
                    "desired_height": desired_height,
                    "viewport_height": viewport_height,
                    "visible_history_rows": terminal.visible_history_rows(),
                    "upward_growth_limited": viewport_height < desired_height,
                    "request_in_flight": state.request_in_flight,
                    "rendering_paused": state.rendering_paused(),
                    "height_shrank_past_viewport": height_shrank_past_viewport,
                    "resize_repaired": resize_repaired,
                    "pin_idle_viewport_to_bottom": pin_idle_viewport_to_bottom,
                    "pin_active_viewport_to_bottom": pin_active_viewport_to_bottom,
                }),
            );
        }
        // Resize frames leave rows above the viewport terminal-owned until the
        // settled source rebuild; they never replay a partial source tail.
        // Stable streaming growth instead scrolls the existing physical rows,
        // preserving them without reconstructing or repainting history.
        //
        // Non-resize reconciliation (High #4): bottom-pinning drops a short
        // viewport to the screen bottom. When a tall live viewport has just
        // collapsed to a shorter committed answer WITHOUT a resize (e.g. streamed
        // content was discarded and replaced at completion, then a new active
        // cell starts), the committed history no longer reaches the pinned
        // position, so pinning strands a blank band between the history and the
        // active viewport — a gap that no resize-settle rebuild will ever repair.
        // Suppress the pin in exactly that case so the active viewport stays
        // anchored right below the history (any slack falls below the input,
        // where it reads as ordinary trailing space). Idle viewports still pin to
        // the bottom (input belongs at the screen bottom), and resize frames
        // still pin (their transient gap is reconciled by the settle rebuild).
        let visible_history_rows = terminal.visible_history_rows();
        // Absolute screen row where the visible history ends. The history block
        // sits directly above the viewport, so its bottom row equals the current
        // (pre-pin) viewport top. This is a POSITION, not the row COUNT
        // `visible_history_rows`: after a native-append startup leaves content
        // above the history (`history_top > 0`), the count and the position
        // diverge by `history_top`, so comparing the pinned position against the
        // count would suppress a valid adjacent pin (or let a real gap pass).
        let visible_history_bottom = terminal.viewport_area.top();
        // Suppress a bottom-pin (idle OR active) on a non-resize frame when it
        // would drop the viewport below the committed history, stranding a blank
        // band above it — the H4 collapse case, and equally the post-first-flush
        // idle case where a short committed transcript (banner + a startup local
        // command) would otherwise leave a gap between it and the bottom-pinned
        // input. Resize frames keep pinning (their transient gap is reconciled by
        // the settle rebuild).
        let suppress_pin_gap = (pin_idle_viewport_to_bottom || pin_active_viewport_to_bottom)
            && !resize_observed
            && !state.resize_reflow_pending()
            // Only meaningful when there is visible history above the viewport for
            // the pin to strand. With no visible history (e.g. it has scrolled off
            // into native scrollback) pinning the input footer to the bottom is
            // correct and strands nothing.
            && visible_history_rows > 0;
        let pinned_area = (pin_idle_viewport_to_bottom || pin_active_viewport_to_bottom)
            .then(|| bottom_pinned_inline_viewport(size, viewport_height))
            .filter(|area| {
                !suppress_pin_gap
                    || !bottom_pin_strands_gap(
                        area.y,
                        visible_history_bottom,
                        state.request_in_flight,
                    )
            });
        let preserve_streaming_history =
            !resize_observed && !state.resize_reflow_pending() && state.request_in_flight;
        let viewport_changed = update_inline_viewport(
            terminal,
            viewport_height,
            pinned_area,
            preserve_streaming_history,
            reserve_terminal_owned_rows,
        )?;
        resize_repaired || viewport_changed
    };
    let history_flushed = !pager_active
        && flush_pending_history_to_scrollback(terminal, state, size.width as usize, size.height)?;
    if crate::terminal_trace::enabled() {
        let (pending_assistant_rendered_lines, live_assistant_tail_lines) =
            state.pending_assistant_history_debug_counts(size.width as usize);
        let emission = &state.transcript_ui.emission;
        crate::terminal_trace::record_event(
            "prepare_draw_transaction_end",
            serde_json::json!({
                "terminal_size": crate::terminal_trace::size(size),
                "viewport_area": crate::terminal_trace::rect(terminal.viewport_area),
                "last_known_cursor_pos": {
                    "x": terminal.last_known_cursor_pos.x,
                    "y": terminal.last_known_cursor_pos.y,
                },
                "pager_active": pager_active,
                "viewport_mutated": viewport_mutated,
                "history_flushed": history_flushed,
                "pending_history_flush": state.pending_history_flush,
                "history_flushed_message_count": state.history_flushed_message_count,
                "message_count": state.messages.len(),
                "pending_assistant_len": state.pending_assistant.chars().count(),
                "emission": {
                    "emitted_cell_count": emission.emitted_cell_count,
                    "pending_flush_cell_count": emission.pending_flush_cell_count,
                    "pending_lines": emission.pending_lines.len(),
                    "reflow_pending": emission.reflow_pending,
                    "needs_scrollback_clear": emission.needs_scrollback_clear,
                    "assistant_stream_width": emission.assistant_stream_width,
                    "assistant_stream_emitted_lines": emission.assistant_stream_emitted_line_count,
                    "assistant_stream_pending_lines": emission.assistant_stream_pending_line_count,
                    "assistant_stream_completed_source_len": emission
                        .assistant_stream_completed_source
                        .as_ref()
                        .map(|source| source.chars().count()),
                    "assistant_stream_completed_message_id": emission
                        .assistant_stream_completed_message_id
                        .as_deref(),
                    "assistant_stream_completed_cell_count": emission
                        .assistant_stream_completed_cell_count,
                    "assistant_stream_rendered_lines": pending_assistant_rendered_lines,
                    "assistant_stream_live_tail_lines": live_assistant_tail_lines,
                },
            }),
        );
    }
    Ok(DrawTransactionResult {
        viewport_mutated,
        history_flushed,
        resize_observed,
    })
}

pub(crate) fn flush_pending_history_to_scrollback<B>(
    terminal: &mut Terminal<B>,
    state: &mut TuiState,
    transcript_width: usize,
    terminal_height: u16,
) -> Result<bool>
where
    B: Backend + Write,
{
    let mut terminal_changed = false;
    if state.transcript_ui.emission.needs_scrollback_clear {
        state.transcript_ui.emission.needs_scrollback_clear = false;
        reset_inline_scrollback_for_reflow(terminal)?;
        terminal_changed = true;
    }

    // The native-append (pre-TUI-scrollback-preserving) path must still fire on
    // the first *user* flush even after intro cells have been emitted, so gate on
    // the emitted-cell count alone rather than also requiring the flushed-message
    // count to be zero. Otherwise the very first user message falls to the
    // clearing insert and the pre-TUI shell scrollback is lost.
    let first_history_flush = state.transcript_ui.emission.emitted_cell_count == 0;
    if !state.prepare_pending_history_emission(transcript_width, terminal_height) {
        return Ok(terminal_changed);
    }

    let screen_size = terminal.size()?;
    if !can_insert_history_above_viewport(terminal.viewport_area, screen_size.height) {
        state.defer_pending_history_flush();
        return Ok(terminal_changed);
    }

    let lines = state.take_pending_history_lines_for_emission(transcript_width, terminal_height);
    if lines.is_empty() {
        return Ok(terminal_changed);
    }

    if crate::terminal_trace::enabled() {
        crate::terminal_trace::record_event(
            "flush_history_inserting",
            serde_json::json!({
                "viewport_area_before": crate::terminal_trace::rect(terminal.viewport_area),
                "line_count": lines.len(),
            }),
        );
    }
    let append_to_native_scrollback = should_append_first_history_flush_to_native_scrollback(
        terminal,
        state,
        screen_size,
        first_history_flush,
    );
    insert_history_lines_with_native_append(
        terminal,
        &lines,
        transcript_width,
        append_to_native_scrollback,
    )?;
    state.commit_history_flush();
    Ok(true)
}

fn should_append_first_history_flush_to_native_scrollback<B>(
    terminal: &Terminal<B>,
    state: &TuiState,
    screen_size: Size,
    first_history_flush: bool,
) -> bool
where
    B: Backend + Write,
{
    first_history_flush
        && terminal.has_drawn_once()
        && terminal.visible_history_rows() == 0
        && terminal.viewport_area.bottom() < screen_size.height
        && terminal.last_known_cursor_pos.y >= terminal.viewport_area.bottom()
        && state.messages.len() <= 1
}

fn should_pin_idle_viewport_to_bottom(
    state: &TuiState,
    area: Rect,
    size: Size,
    viewport_height: u16,
) -> bool {
    !state.request_in_flight
        && !state.rendering_paused()
        // Before the first history flush, do NOT bottom-pin the idle viewport:
        // during the startup window the transient panels reserve terminal-owned
        // rows (see `should_reserve_terminal_owned_rows_for_viewport_growth`), and
        // re-anchoring to the screen bottom here would defeat that and keep
        // `viewport_area.bottom() == screen.height`, which prevents the
        // shrink-on-submit from routing the first flush through the
        // scrollback-preserving native append.
        && state.history_flushed_message_count > 0
        && !state.pending_history_flush
        && state.transcript_ui.emission.pending_lines.is_empty()
        && area.bottom() < size.height
        && viewport_height <= area.height
}

fn should_pin_active_viewport_to_bottom(
    state: &TuiState,
    area: Rect,
    size: Size,
    viewport_height: u16,
) -> bool {
    state.request_in_flight
        && !state.rendering_paused()
        && !state.pending_history_flush
        && state.transcript_ui.emission.pending_lines.is_empty()
        && area.bottom() < size.height
        && viewport_height <= max_inline_viewport_height(size.height)
}

fn bottom_pinned_inline_viewport(size: Size, height: u16) -> Rect {
    let height = height.min(max_inline_viewport_height(size.height)).max(1);
    Rect::new(0, size.height.saturating_sub(height), size.width, height)
}

/// The largest viewport height that keeps the viewport top at its current row
/// (no upward growth over the rows above it). `resized_inline_viewport` reports
/// how much a taller viewport would overflow the screen bottom (`scroll_up`);
/// subtracting it yields a height that pins the bottom without moving the top
/// up. Used for idle transient panels, which should not displace transcript
/// history merely because suggestions or other temporary UI appeared.
fn viewport_height_without_upward_growth(area: Rect, size: Size, desired_height: u16) -> u16 {
    let desired_height = desired_height
        .min(max_inline_viewport_height(size.height))
        .max(1);
    let (_, scroll_up) = resized_inline_viewport(area, size, desired_height);
    desired_height.saturating_sub(scroll_up).max(1)
}

/// Whether idle upward viewport growth in the STARTUP WINDOW (before the first
/// history flush) should reserve terminal-owned rows — scrolling the pre-TUI
/// shell output + banner up into native scrollback — instead of capping the
/// growth or clearing those rows. Only fires while nothing has been committed to
/// scrollback yet (`history_flushed_message_count == 0`) and the viewport is idle
/// and not shrinking, so it never touches the codex resize-purge model or the
/// post-flush paths. Height shrinks are excluded (there is nothing to reserve).
fn should_reserve_terminal_owned_rows_for_viewport_growth(
    state: &TuiState,
    height_shrank_past_viewport: bool,
) -> bool {
    state.history_flushed_message_count == 0
        && !state.request_in_flight
        && !state.rendering_paused()
        && !state.pending_history_flush
        && state.transcript_ui.emission.pending_lines.is_empty()
        && !height_shrank_past_viewport
}

#[cfg(test)]
pub(crate) fn update_inline_viewport_generic<B>(
    terminal: &mut Terminal<B>,
    height: u16,
) -> Result<bool>
where
    B: Backend + Write,
{
    update_inline_viewport(terminal, height, None, false, false)
}

/// Reposition the inline viewport. Stable-size streaming growth first scrolls
/// app-owned visible history out through a top-anchored scroll region so the
/// enlarged live viewport cannot clear it. Resize frames deliberately skip that
/// step: terminal/tmux owns those rows until the settled source rebuild.
/// `reserve_terminal_owned_rows` (startup-window idle growth) instead
/// full-screen-scrolls terminal-owned rows into native scrollback so the growth
/// pushes pre-TUI history up rather than clearing it.
fn update_inline_viewport<B>(
    terminal: &mut Terminal<B>,
    height: u16,
    forced_area: Option<Rect>,
    preserve_visible_history_on_upward_growth: bool,
    reserve_terminal_owned_rows: bool,
) -> Result<bool>
where
    B: Backend + Write,
{
    let size = terminal.size()?;
    let previous_area = terminal.viewport_area;
    let (area, scroll_up) = forced_area
        .map(|area| (area, 0))
        .unwrap_or_else(|| resized_inline_viewport(terminal.viewport_area, size, height));
    let history_scroll_rows =
        if preserve_visible_history_on_upward_growth && terminal.visible_history_rows() > 0 {
            previous_area.y.saturating_sub(area.y)
        } else {
            0
        };
    if crate::terminal_trace::enabled() {
        crate::terminal_trace::record_event(
            "update_inline_viewport",
            serde_json::json!({
                "terminal_size": crate::terminal_trace::size(size),
                "requested_height": height,
                "max_inline_height": max_inline_viewport_height(size.height),
                "height_capped": height > max_inline_viewport_height(size.height),
                "previous_area": crate::terminal_trace::rect(previous_area),
                "next_area": crate::terminal_trace::rect(area),
                "scroll_up": scroll_up,
                "history_scroll_rows": history_scroll_rows,
                "reserve_terminal_owned_rows": reserve_terminal_owned_rows,
                "forced_bottom_pin": forced_area.is_some(),
            }),
        );
    }
    let mut mutated = false;
    if history_scroll_rows > 0 {
        scroll_visible_history_for_viewport_growth(terminal, history_scroll_rows)?;
        terminal.invalidate_viewport();
        mutated = true;
    }
    if scroll_up > 0 {
        if reserve_terminal_owned_rows {
            // Startup-window idle upward growth: clear the old inline drawing
            // first (so the previous banner/input is not duplicated into
            // scrollback when we scroll), then full-screen-scroll `scroll_up` rows
            // up — pushing the pre-TUI history into native scrollback (preserved,
            // scrollable) and opening blank rows at the bottom for the grown
            // viewport. `clear_after_position` is AfterCursor, so it never touches
            // rows above `previous_area.y`.
            terminal.clear_after_position(Position::new(0, previous_area.y))?;
            reserve_terminal_owned_rows_for_inline_viewport(terminal, scroll_up)?;
        }
        // The geometry moved upward, so repaint the viewport at its new
        // position. On resize frames the terminal/tmux already handled the
        // physical rows; stable streaming growth preserved history above.
        terminal.invalidate_viewport();
        mutated = true;
    }
    if area != previous_area {
        let clear_y = previous_area.y.min(area.y);
        terminal.set_viewport_area(area);
        terminal.clear_after_position(Position::new(0, clear_y))?;
        mutated = true;
    }
    Ok(mutated)
}

fn scroll_visible_history_for_viewport_growth<B>(
    terminal: &mut Terminal<B>,
    rows: u16,
) -> io::Result<()>
where
    B: Backend + Write,
{
    let history_bottom = terminal.viewport_area.top();
    if rows == 0 || history_bottom == 0 {
        return Ok(());
    }

    if crate::terminal_trace::enabled() {
        crate::terminal_trace::record_event(
            "scroll_visible_history_for_viewport_growth",
            serde_json::json!({
                "viewport_area": crate::terminal_trace::rect(terminal.viewport_area),
                "rows": rows,
                "visible_history_rows": terminal.visible_history_rows(),
            }),
        );
    }

    let size = terminal.size()?;
    if size.height == 0 {
        return Ok(());
    }
    // `rows` is how far the viewport top moved up, and it never exceeds
    // `history_bottom` (the current viewport top), so a full-screen scroll of
    // `rows` pushes ONLY the oldest visible-history rows off the top — never the
    // live viewport below. A DECSTBM sub-region scroll (`SetScrollRegion`) would
    // DISCARD those rows (both tmux and plain terminals drop rows scrolled off a
    // sub-region top), permanently losing history such as the intro banner. A
    // full-screen scroll (`ESC[r` reset + cursor at the last row + `\r\n`)
    // instead pushes them into native scrollback, where they stay scrollable.
    // The caller invalidates and repaints the viewport at its new position, so
    // the transiently-scrolled viewport rows are fully redrawn.
    let writer = terminal.backend_mut();
    queue!(
        writer,
        ResetScrollRegion,
        MoveTo(0, size.height.saturating_sub(1))
    )?;
    for _ in 0..rows {
        queue!(writer, Print("\r\n"))?;
    }
    Ok(())
}

/// Reserve `rows` blank rows at the bottom of the inline viewport by scrolling the
/// FULL screen up — pushing the top `rows` (pre-TUI shell output / banner) into
/// the terminal's native scrollback where they stay scrollable. Like
/// `scroll_visible_history_for_viewport_growth`, this uses a full-screen scroll
/// (`ESC[r` reset + cursor at the last row + `\r\n`) rather than a DECSTBM
/// sub-region scroll — a sub-region scroll would discard the scrolled-off rows
/// (both tmux and plain terminals drop rows scrolled off a sub-region top). The
/// full-screen scroll reliably reaches native scrollback in both tmux and plain
/// terminals. Used only in the startup window for idle upward growth so the
/// suggestion panel / first flush pushes history UP instead of eating it.
fn reserve_terminal_owned_rows_for_inline_viewport<B>(
    terminal: &mut Terminal<B>,
    rows: u16,
) -> io::Result<()>
where
    B: Backend + Write,
{
    if rows == 0 {
        return Ok(());
    }
    let size = terminal.size()?;
    if size.height == 0 {
        return Ok(());
    }

    if crate::terminal_trace::enabled() {
        crate::terminal_trace::record_event(
            "reserve_terminal_owned_rows_for_inline_viewport",
            serde_json::json!({
                "rows": rows,
                "viewport_area": crate::terminal_trace::rect(terminal.viewport_area),
                "terminal_size": crate::terminal_trace::size(size),
            }),
        );
    }

    let writer = terminal.backend_mut();
    queue!(
        writer,
        ResetScrollRegion,
        MoveTo(0, size.height.saturating_sub(1))
    )?;
    for _ in 0..rows {
        queue!(writer, Print("\r\n"), Clear(ClearType::UntilNewLine))?;
    }
    Ok(())
}

fn reset_inline_scrollback_for_reflow<B>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    B: ratatui::backend::Backend + Write,
{
    if crate::terminal_trace::enabled() {
        crate::terminal_trace::record_event(
            "reset_inline_scrollback_for_reflow",
            serde_json::json!({
                "viewport_area": crate::terminal_trace::rect(terminal.viewport_area),
            }),
        );
    }
    // Codex-style hard reset before re-emitting the transcript from source:
    // reset scroll region + SGR, home, clear the VISIBLE screen (ESC[2J), purge
    // native scrollback (ESC[3J), home. Clearing the visible screen is essential
    // — without it the re-emit lands on top of stale rows, leaving blank bands
    // and partial-render remnants for any row the re-emit doesn't overwrite.
    let writer = terminal.backend_mut();
    queue!(writer, ResetScrollRegion)?;
    queue!(writer, SetAttribute(Attribute::Reset))?;
    queue!(writer, MoveTo(0, 0))?;
    queue!(writer, Clear(ClearType::All))?;
    queue!(writer, Clear(ClearType::Purge))?;
    queue!(writer, MoveTo(0, 0))?;
    Write::flush(writer)?;

    let size = terminal.size()?;
    terminal.set_viewport_area(Rect::new(0, 0, size.width, terminal.viewport_area.height));
    terminal.reset_visible_history();
    terminal.invalidate_viewport();
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryLineWrapPolicy {
    PreWrap,
    #[allow(dead_code)]
    Terminal,
}

#[cfg(test)]
pub(crate) fn insert_history_lines<B>(
    terminal: &mut Terminal<B>,
    lines: &[StyledLine],
    transcript_width: usize,
) -> io::Result<()>
where
    B: ratatui::backend::Backend + Write,
{
    insert_history_lines_with_native_append(terminal, lines, transcript_width, false)
}

fn insert_history_lines_with_native_append<B>(
    terminal: &mut Terminal<B>,
    lines: &[StyledLine],
    transcript_width: usize,
    append_to_native_scrollback: bool,
) -> io::Result<()>
where
    B: ratatui::backend::Backend + Write,
{
    insert_history_lines_with_wrap_policy(
        terminal,
        lines,
        transcript_width,
        HistoryLineWrapPolicy::PreWrap,
        append_to_native_scrollback,
    )
}

pub(crate) fn insert_history_lines_with_wrap_policy<B>(
    terminal: &mut Terminal<B>,
    lines: &[StyledLine],
    transcript_width: usize,
    wrap_policy: HistoryLineWrapPolicy,
    append_to_native_scrollback: bool,
) -> io::Result<()>
where
    B: ratatui::backend::Backend + Write,
{
    let transcript_width = transcript_width.max(1);
    let history_lines = history_lines_for_wrap_policy(lines, transcript_width, wrap_policy);
    if history_lines.is_empty() {
        return Ok(());
    }

    let screen_size = terminal.size()?;
    let row_count = history_physical_row_count(&history_lines, transcript_width);
    let mut area = terminal.viewport_area;
    let cursor = terminal.last_known_cursor_pos;

    if !can_insert_history_above_viewport(area, screen_size.height) {
        return Ok(());
    }

    let trace_enabled = crate::terminal_trace::enabled();
    let area_before = area;
    let writer = terminal.backend_mut();
    if trace_enabled {
        let mut output = Vec::new();
        let result = if append_to_native_scrollback {
            queue_native_scrollback_history_append(
                &mut output,
                &history_lines,
                &mut area,
                screen_size,
                row_count,
                cursor,
                wrap_policy,
            )
        } else {
            queue_standard_history_insert(
                &mut output,
                &history_lines,
                &mut area,
                screen_size,
                row_count,
                cursor,
                wrap_policy,
                transcript_width,
            )
        };
        crate::terminal_trace::record_bytes(
            if append_to_native_scrollback {
                "insert_history_lines_native_append"
            } else {
                "insert_history_lines_standard"
            },
            serde_json::json!({
                "screen_size": crate::terminal_trace::size(screen_size),
                "area_before": crate::terminal_trace::rect(area_before),
                "area_after": crate::terminal_trace::rect(area),
                "row_count": row_count,
                "line_count": history_lines.len(),
                "wrap_policy": format!("{wrap_policy:?}"),
                "preview": history_lines
                    .iter()
                    .take(5)
                    .map(styled_line_plain_text)
                    .collect::<Vec<_>>(),
            }),
            &output,
        );
        writer.write_all(&output)?;
        result?;
    } else if append_to_native_scrollback {
        queue_native_scrollback_history_append(
            writer,
            &history_lines,
            &mut area,
            screen_size,
            row_count,
            cursor,
            wrap_policy,
        )?;
    } else {
        queue_standard_history_insert(
            writer,
            &history_lines,
            &mut area,
            screen_size,
            row_count,
            cursor,
            wrap_policy,
            transcript_width,
        )?;
    }
    Write::flush(writer)?;

    let next_area = Rect {
        width: screen_size.width,
        ..area
    };
    terminal.set_viewport_area(next_area);
    terminal.note_history_rows_inserted(row_count);
    terminal.invalidate_viewport();
    Ok(())
}

fn history_lines_for_wrap_policy(
    lines: &[StyledLine],
    transcript_width: usize,
    wrap_policy: HistoryLineWrapPolicy,
) -> Vec<StyledLine> {
    match wrap_policy {
        HistoryLineWrapPolicy::PreWrap => wrap_styled_lines(lines, transcript_width.max(1)),
        HistoryLineWrapPolicy::Terminal => lines.to_vec(),
    }
}

fn history_physical_row_count(lines: &[StyledLine], transcript_width: usize) -> u16 {
    let width = transcript_width.max(1);
    lines
        .iter()
        .map(|line| styled_line_display_width(line).max(1).div_ceil(width))
        .sum::<usize>()
        .min(u16::MAX as usize) as u16
}

fn can_insert_history_above_viewport(area: Rect, screen_height: u16) -> bool {
    area.top() > 0 || area.bottom() < screen_height
}

fn queue_standard_history_insert(
    writer: &mut impl Write,
    lines: &[StyledLine],
    area: &mut Rect,
    screen_size: Size,
    row_count: u16,
    cursor: Position,
    wrap_policy: HistoryLineWrapPolicy,
    transcript_width: usize,
) -> io::Result<()> {
    if wrap_policy == HistoryLineWrapPolicy::PreWrap {
        queue!(writer, DisableLineWrap)?;
    }
    let result = (|| -> io::Result<()> {
        clear_live_viewport_rows(writer, *area)?;
        let cursor_top = if area.bottom() < screen_size.height {
            let scroll_amount = row_count.min(screen_size.height - area.bottom());
            queue!(writer, SetScrollRegion(area.top() + 1..screen_size.height))?;
            queue!(writer, MoveTo(0, area.top()))?;
            for _ in 0..scroll_amount {
                queue!(writer, Print("\x1bM"))?;
            }
            queue!(writer, ResetScrollRegion)?;

            let cursor_top = area.top().saturating_sub(1);
            area.y = area.y.saturating_add(scroll_amount);
            cursor_top
        } else {
            area.top().saturating_sub(1)
        };

        queue!(writer, SetScrollRegion(1..area.top()))?;
        queue!(writer, MoveTo(0, cursor_top))?;
        for line in lines {
            queue!(writer, Print("\r\n"))?;
            clear_soft_wrap_continuation_rows(writer, line, transcript_width)?;
            queue_styled_line(writer, line)?;
            queue!(
                writer,
                SetAttribute(Attribute::Reset),
                Clear(ClearType::UntilNewLine)
            )?;
        }
        queue!(writer, ResetScrollRegion)?;
        queue!(writer, MoveTo(cursor.x, cursor.y))?;
        Ok(())
    })();
    if wrap_policy == HistoryLineWrapPolicy::PreWrap {
        let restore_result = queue!(writer, EnableLineWrap);
        match (result, restore_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    } else {
        result
    }
}

fn queue_native_scrollback_history_append(
    writer: &mut impl Write,
    lines: &[StyledLine],
    area: &mut Rect,
    screen_size: Size,
    row_count: u16,
    cursor: Position,
    wrap_policy: HistoryLineWrapPolicy,
) -> io::Result<()> {
    if wrap_policy == HistoryLineWrapPolicy::PreWrap {
        queue!(writer, DisableLineWrap)?;
    }
    let result = (|| -> io::Result<()> {
        let viewport_height = area.height.min(screen_size.height).max(1);
        let history_top = area.top().min(screen_size.height.saturating_sub(1));
        clear_tui_rows_from(writer, history_top, screen_size.height)?;
        queue!(writer, ResetScrollRegion)?;
        queue!(writer, MoveTo(0, history_top))?;
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                queue!(writer, Print("\r\n"))?;
            }
            queue_styled_line(writer, line)?;
            queue!(
                writer,
                SetAttribute(Attribute::Reset),
                Clear(ClearType::UntilNewLine)
            )?;
        }

        // Reserve the live viewport immediately after history. Once the cleared
        // terminal-owned rows are exhausted, these full-screen line feeds push
        // both pre-TUI output and the complete history into native scrollback.
        for _ in 0..viewport_height {
            queue!(writer, Print("\r\n"), Clear(ClearType::UntilNewLine))?;
        }

        area.y = history_top
            .saturating_add(row_count)
            .min(screen_size.height.saturating_sub(viewport_height));
        area.height = viewport_height;
        area.width = screen_size.width;
        queue!(
            writer,
            MoveTo(
                cursor.x.min(screen_size.width.saturating_sub(1)),
                cursor.y.min(screen_size.height.saturating_sub(1))
            )
        )?;
        Ok(())
    })();
    if wrap_policy == HistoryLineWrapPolicy::PreWrap {
        let restore_result = queue!(writer, EnableLineWrap);
        match (result, restore_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    } else {
        result
    }
}

fn clear_live_viewport_rows(writer: &mut impl Write, area: Rect) -> io::Result<()> {
    for row in area.top()..area.bottom() {
        queue!(writer, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
    }
    Ok(())
}

/// Clear every terminal row in `[start_row, end_row)` in place before the
/// first-flush append reuses the TUI-owned region.
fn clear_tui_rows_from(writer: &mut impl Write, start_row: u16, end_row: u16) -> io::Result<()> {
    for row in start_row..end_row {
        queue!(writer, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
    }
    Ok(())
}

fn styled_line_plain_text(line: &StyledLine) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn queue_styled_line(writer: &mut impl Write, line: &StyledLine) -> io::Result<()> {
    for span in &line.spans {
        queue_style(writer, span.style)?;
        queue!(writer, Print(span.content.as_ref()))?;
    }
    Ok(())
}

fn clear_soft_wrap_continuation_rows(
    writer: &mut impl Write,
    line: &StyledLine,
    transcript_width: usize,
) -> io::Result<()> {
    let physical_rows = styled_line_display_width(line)
        .max(1)
        .div_ceil(transcript_width.max(1));
    if physical_rows <= 1 {
        return Ok(());
    }

    queue!(writer, SavePosition)?;
    for _ in 1..physical_rows {
        queue!(
            writer,
            MoveDown(1),
            MoveToColumn(0),
            Clear(ClearType::UntilNewLine)
        )?;
    }
    queue!(writer, RestorePosition)?;
    Ok(())
}

fn queue_style(writer: &mut impl Write, style: Style) -> io::Result<()> {
    queue!(writer, SetAttribute(Attribute::Reset))?;
    if let Some(fg) = style.fg {
        queue!(writer, SetForegroundColor(to_crossterm_color(fg)))?;
    }
    if let Some(bg) = style.bg {
        queue!(writer, SetBackgroundColor(to_crossterm_color(bg)))?;
    }
    queue_modifiers(writer, style.add_modifier)
}

fn queue_modifiers(writer: &mut impl Write, modifiers: Modifier) -> io::Result<()> {
    if modifiers.contains(Modifier::BOLD) {
        queue!(writer, SetAttribute(Attribute::Bold))?;
    }
    if modifiers.contains(Modifier::DIM) {
        queue!(writer, SetAttribute(Attribute::Dim))?;
    }
    if modifiers.contains(Modifier::ITALIC) {
        queue!(writer, SetAttribute(Attribute::Italic))?;
    }
    if modifiers.contains(Modifier::UNDERLINED) {
        queue!(writer, SetAttribute(Attribute::Underlined))?;
    }
    if modifiers.contains(Modifier::REVERSED) {
        queue!(writer, SetAttribute(Attribute::Reverse))?;
    }
    if modifiers.contains(Modifier::CROSSED_OUT) {
        queue!(writer, SetAttribute(Attribute::CrossedOut))?;
    }
    Ok(())
}

fn to_crossterm_color(color: Color) -> crossterm::style::Color {
    match color {
        Color::Reset => crossterm::style::Color::Reset,
        Color::Black => crossterm::style::Color::Black,
        Color::Red => crossterm::style::Color::DarkRed,
        Color::Green => crossterm::style::Color::DarkGreen,
        Color::Yellow => crossterm::style::Color::DarkYellow,
        Color::Blue => crossterm::style::Color::DarkBlue,
        Color::Magenta => crossterm::style::Color::DarkMagenta,
        Color::Cyan => crossterm::style::Color::DarkCyan,
        Color::Gray => crossterm::style::Color::Grey,
        Color::DarkGray => crossterm::style::Color::DarkGrey,
        Color::LightRed => crossterm::style::Color::Red,
        Color::LightGreen => crossterm::style::Color::Green,
        Color::LightYellow => crossterm::style::Color::Yellow,
        Color::LightBlue => crossterm::style::Color::Blue,
        Color::LightMagenta => crossterm::style::Color::Magenta,
        Color::LightCyan => crossterm::style::Color::Cyan,
        Color::White => crossterm::style::Color::White,
        Color::Rgb(r, g, b) => crossterm::style::Color::Rgb { r, g, b },
        Color::Indexed(index) => crossterm::style::Color::AnsiValue(index),
    }
}

pub(crate) fn resized_inline_viewport(current: Rect, size: Size, height: u16) -> (Rect, u16) {
    let mut area = current;
    area.width = size.width;
    area.height = height.min(max_inline_viewport_height(size.height)).max(1);
    let mut scroll_up = 0;

    if area.bottom() > size.height {
        scroll_up = area.bottom() - size.height;
        area.y = size.height.saturating_sub(area.height);
    } else if area.y >= size.height {
        area.y = size.height.saturating_sub(area.height);
    }

    (area, scroll_up)
}

fn max_inline_viewport_height(screen_height: u16) -> u16 {
    screen_height.saturating_sub(1).max(1)
}

/// How far below the last tracked committed-history row the bottom-pinned
/// viewport may sit before the pin is suppressed to avoid stranding a blank band
/// above it. Kept just under the `max_blank_gap <= 2` tolerance the render tests
/// assert so a tolerated small gap never trips the reconciliation.
const RECONCILE_HISTORY_GAP_ROWS: u16 = 2;

/// Whether bottom-pinning the viewport to `pinned_top` would strand a blank gap
/// above it, given the committed history ends at `visible_history_bottom`.
///
/// During an active turn (`active_turn`) the live viewport height fluctuates by
/// a row as content streams (thinking text wrapping, spinner lines). A pin that
/// moves the top even ONE row below the history strands a blank; when the next
/// cell commits it is inserted (with its own leading blank) directly below that
/// orphan, baking a stray DOUBLE blank line into scrollback. So an active turn
/// requires STRICT adjacency (0 tolerance). Idle frames keep the small tolerance
/// the render tests allow (a tolerated 1–2 row gap below a short committed
/// transcript never trips reconciliation).
fn bottom_pin_strands_gap(pinned_top: u16, visible_history_bottom: u16, active_turn: bool) -> bool {
    let tolerance = if active_turn {
        0
    } else {
        RECONCILE_HISTORY_GAP_ROWS
    };
    pinned_top > visible_history_bottom.saturating_add(tolerance)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SetScrollRegion(Range<u16>);

impl Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        panic!("use ANSI for SetScrollRegion");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResetScrollRegion;

impl Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        panic!("use ANSI for ResetScrollRegion");
    }
}

#[cfg(test)]
mod pin_gap_tests {
    use super::bottom_pin_strands_gap;

    #[test]
    fn active_turn_requires_strict_adjacency() {
        // History ends at row 17; pinning the viewport top at 18 is adjacent.
        assert!(!bottom_pin_strands_gap(18, 18, true));
        // A one-row downward move (top 19, history bottom 18) strands a blank
        // during an active turn — this is the double-blank-line bug — so the pin
        // must be suppressed.
        assert!(bottom_pin_strands_gap(19, 18, true));
        assert!(bottom_pin_strands_gap(20, 18, true));
    }

    #[test]
    fn active_turn_suppresses_gap_of_any_size() {
        // The gate is an absolute compare of the proposed pinned top vs the
        // history bottom, not a per-row accumulation: an N-row downward move is
        // caught in a single check for every N >= 1 (a large content shrink that
        // would drop the top many rows below history is suppressed too).
        for n in 1..=40u16 {
            assert!(
                bottom_pin_strands_gap(18 + n, 18, true),
                "active turn must suppress a {n}-row gap"
            );
        }
    }

    #[test]
    fn idle_frames_keep_small_tolerance() {
        // Idle frames tolerate the small (<= RECONCILE_HISTORY_GAP_ROWS) gap the
        // render tests allow, so a short committed transcript can still bottom-pin
        // the input.
        assert!(!bottom_pin_strands_gap(18, 18, false));
        assert!(!bottom_pin_strands_gap(19, 18, false));
        assert!(!bottom_pin_strands_gap(20, 18, false));
        // Beyond the tolerance the pin is still suppressed.
        assert!(bottom_pin_strands_gap(21, 18, false));
    }
}
