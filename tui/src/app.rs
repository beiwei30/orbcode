use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use crossterm::{SynchronizedUpdate, execute};
use orbcode_app_server_client::AppClient;
use orbcode_config::PermissionMode;
use orbcode_protocol::{MessageRole, ProviderId, StreamEvent, TokenUsage, TranscriptMessage};
use tokio::sync::mpsc;
use tokio::time::{self, Duration, MissedTickBehavior};

const STATUSLINE_CMD_TIMEOUT: Duration = Duration::from_secs(5);
const PAGER_DEFERRED_FIXTURE_TEXT: &str = "pager deferred fixture committed while ctrl-o open";
const FINAL_ANSWER_FIXTURE_HEAD: &str = "final answer fixture head";
const FINAL_ANSWER_FIXTURE_TAIL: &str = "final answer fixture tail";

use crate::background_agent_panel::POLL_INTERVAL as BACKGROUND_AGENT_POLL_INTERVAL;
use crate::chat::stream_events::{handle_stream_event_batch, mark_redraw};
use crate::commands::async_local::handle_local_command_event_batch;
use crate::render::request_status::SPINNER_TICK_MS;
use crate::render_metrics::{RenderEventCounts, RenderMetricsRecorder};
use crate::state::TuiState;
use crate::task_panel::POLL_FALLBACK;
use crate::tui_runtime::terminal_session::{
    TranscriptPagerTerminalMode, handle_terminal_event, prepare_draw_transaction,
    prepare_terminal_for_cli_output, restore_terminal, setup_terminal, spawn_terminal_event_reader,
    sync_transcript_pager_terminal_mode,
};

pub async fn run_tui(client: Arc<AppClient>, requested_session: Option<String>) -> Result<String> {
    let bootstrap = client.bootstrap(requested_session.as_deref()).await?;
    let mut dynamic_specs = crate::dynamic_slash_commands::load_dynamic_slash_commands(
        &bootstrap.home_dir,
        &bootstrap.cwd,
    )
    .await;
    dynamic_specs.extend(crate::dynamic_slash_commands::load_workflow_commands(&client).await);
    dynamic_specs.extend(crate::dynamic_slash_commands::load_mcp_prompt_commands(&client).await);
    crate::slash_commands::register_dynamic_slash_commands(dynamic_specs);
    crate::keybindings::load_keybindings_global(
        orbcode_config::load_keybindings(&bootstrap.home_dir),
        bootstrap.home_dir.clone(),
    );
    #[cfg(test)]
    let state_client = Some(Arc::clone(&client));
    #[cfg(not(test))]
    let state_client = Arc::clone(&client);
    let mut state = TuiState::new(state_client, bootstrap);
    state.queue_existing_history_flush();
    if std::env::var_os("ORBCODE_TUI_MANUAL_SCROLLBACK_FIXTURE").is_some() {
        install_manual_scrollback_fixture(&mut state);
    }
    if std::env::var_os("ORBCODE_TUI_PTY_SMOKE_HISTORY_SUMMARY").is_some() {
        install_pty_smoke_history_summary(&mut state);
    }
    if std::env::var_os("ORBCODE_TUI_RESIZE_STREAMING_FIXTURE").is_some() {
        install_resize_streaming_fixture(&mut state);
    }
    if std::env::var_os("ORBCODE_TUI_FINAL_ANSWER_FIXTURE").is_some() {
        install_final_answer_fixture(&mut state);
    }
    if let Ok(result) = client.permission_mode().await
        && let Some(mode) = PermissionMode::parse(&result.mode)
    {
        state.status.permission_mode = mode;
    }
    if let Ok(allow) = client.allow_all().await {
        state.status.allow_all = allow;
    }
    if let Ok(overview) = client.status_overview(&state.session_id).await {
        state.status.sandbox_mode = overview.sandbox_mode;
        state.status.bg_job_count = overview.background_job_count;
        state.status.effort = overview.effort_level;
    }
    state.status.git_branch = detect_git_branch(&state.cwd);
    let mut render_metrics = RenderMetricsRecorder::from_env()?;
    let mut terminal = setup_terminal()?;
    // Test-only smoke hook used by CLI PTY E2E tests to prove first-frame
    // rendering and terminal restoration without brittle key injection.
    if std::env::var_os("ORBCODE_TUI_PTY_SMOKE_EXIT_AFTER_FIRST_FRAME").is_some() {
        prepare_draw_transaction(&mut terminal, &mut state, false)?;
        execute!(terminal.backend_mut(), state.cursor_style())?;
        if let Some(metrics) = render_metrics.as_mut() {
            let reasons = ["pty_smoke_first_frame"];
            let draw_metrics = terminal.draw_with_metrics(|frame| state.draw(frame))?;
            let size = terminal.size()?;
            let context = state.render_metrics_context(
                &terminal,
                size,
                &reasons,
                RenderEventCounts::default(),
            );
            metrics.record_frame(&draw_metrics, context)?;
        } else {
            terminal.draw(|frame| state.draw(frame))?;
        }
        let cleanup_result = prepare_terminal_for_cli_output(&mut terminal);
        let restore_result = restore_terminal(&mut terminal);
        cleanup_result?;
        restore_result?;
        return Ok(state.session_id.clone());
    }
    let (terminal_shutdown, terminal_reader_paused, terminal_reader, mut terminal_events) =
        spawn_terminal_event_reader();
    let mut turn_events: Option<mpsc::UnboundedReceiver<StreamEvent>> = None;
    let (background_task_event_tx, mut background_task_event_rx) =
        mpsc::unbounded_channel::<StreamEvent>();
    let (local_command_tx, mut local_command_rx) = mpsc::unbounded_channel();
    let mut animation_tick = time::interval(Duration::from_millis(SPINNER_TICK_MS));
    animation_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut task_panel_poll = time::interval(POLL_FALLBACK);
    task_panel_poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut background_agent_poll = time::interval(BACKGROUND_AGENT_POLL_INTERVAL);
    background_agent_poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let (statusline_cmd_tx, mut statusline_cmd_rx) = mpsc::unbounded_channel::<Option<String>>();
    let mut statusline_cmd_tick = time::interval(state.statusline_refresh_interval);
    statusline_cmd_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut statusline_cmd_in_flight = false;
    let mut transcript_pager_terminal_mode = TranscriptPagerTerminalMode::default();
    let pager_deferred_fixture_enabled =
        std::env::var_os("ORBCODE_TUI_PAGER_DEFERRED_FIXTURE").is_some();
    let mut pager_deferred_fixture_injected = false;
    let final_answer_fixture_enabled =
        std::env::var_os("ORBCODE_TUI_FINAL_ANSWER_FIXTURE").is_some();
    let mut final_answer_fixture_active_drawn = false;
    let mut final_answer_fixture_completed = false;
    if let Some(ref cmd) = state.statusline_command {
        statusline_cmd_in_flight = true;
        let cmd = cmd.clone();
        let cwd = state.cwd.clone();
        let tx = statusline_cmd_tx.clone();
        // Detached statusline command; completion is delivered through
        // statusline_cmd_rx and guarded by statusline_cmd_in_flight.
        let _statusline_cmd_handle = tokio::spawn(async move {
            let _ = tx.send(run_statusline_command(&cmd, &cwd).await);
        });
    }
    let run_result: Result<()> = async {
        // Two redraw flags so streaming output can be deferred while the
        // user is scrolling or selecting text (tmux copy-mode style).
        // `needs_user_redraw` tracks redraws caused directly by terminal
        // input (keys, mouse) and always paints. `needs_stream_redraw`
        // tracks background sources (stream tokens, animation, polls) and
        // is held while `state.rendering_paused()` is true.
        let mut needs_user_redraw = true;
        let mut needs_stream_redraw = false;
        let mut redraw_reasons = vec!["initial"];
        let mut render_event_counts = RenderEventCounts::default();
        // Debounce terminal resizes: after the size stops changing for this long,
        // run one full source-of-truth history rebuild (codex-style) so rows
        // above the viewport — including the intro banner — are restored intact.
        // While a turn streams, the rebuild is deferred to this settle so we do
        // not purge/flash on every SIGWINCH frame mid-turn. Tunable via
        // `ORBCODE_TUI_RESIZE_SETTLE_MS` for terminal-specific drag cadence.
        let resize_settle: Duration = Duration::from_millis(
            std::env::var("ORBCODE_TUI_RESIZE_SETTLE_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(150),
        );
        let mut resize_settle_deadline: Option<time::Instant> = None;
        'main: loop {
            if !state.rendering_paused()
                && state.task_panel.needs_refresh(std::time::Instant::now())
                && state.task_panel.refresh(&client).await
            {
                mark_redraw(
                    &mut needs_stream_redraw,
                    &mut redraw_reasons,
                    "task_panel_refresh",
                );
            }
            if !state.rendering_paused()
                && state
                    .background_agent_panel
                    .needs_refresh(std::time::Instant::now())
                && state.background_agent_panel.refresh(&client).await
            {
                mark_redraw(
                    &mut needs_stream_redraw,
                    &mut redraw_reasons,
                    "background_agent_panel_refresh",
                );
            }
            let transcript_pager_open = matches!(
                state.overlay,
                Some(crate::overlays::OverlayState::TranscriptPager(_))
            );
            if sync_transcript_pager_terminal_mode(
                &mut terminal,
                transcript_pager_open,
                &mut transcript_pager_terminal_mode,
            )? {
                mark_redraw(
                    &mut needs_user_redraw,
                    &mut redraw_reasons,
                    "transcript_pager_terminal_mode",
                );
            }
            if pager_deferred_fixture_enabled
                && transcript_pager_terminal_mode.is_active()
                && !pager_deferred_fixture_injected
            {
                state.push_message_and_flush_history(TranscriptMessage::new(
                    MessageRole::Assistant,
                    PAGER_DEFERRED_FIXTURE_TEXT.to_string(),
                ));
                pager_deferred_fixture_injected = true;
                mark_redraw(
                    &mut needs_user_redraw,
                    &mut redraw_reasons,
                    "pager_deferred_fixture",
                );
            }
            if final_answer_fixture_enabled
                && final_answer_fixture_active_drawn
                && !final_answer_fixture_completed
            {
                let _ = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
                    message: TranscriptMessage::new(
                        MessageRole::Assistant,
                        final_answer_fixture_text(),
                    ),
                    provider: ProviderId::Anthropic,
                    fallback_from: None,
                    usage: TokenUsage::default(),
                });
                final_answer_fixture_completed = true;
                state.set_status_line("Final answer fixture completed for tmux validation");
                mark_redraw(
                    &mut needs_stream_redraw,
                    &mut redraw_reasons,
                    "final_answer_fixture_complete",
                );
            }

            if !state.rendering_paused() {
                if state.should_refresh_background_jobs_overlay() {
                    state.refresh_background_jobs_overlay(&client).await;
                    mark_redraw(
                        &mut needs_stream_redraw,
                        &mut redraw_reasons,
                        "background_jobs_refresh",
                    );
                }
                subscribe_pending_transcript_task_cards(
                    &mut state,
                    &client,
                    &background_task_event_tx,
                );
            }
            // Wrap state finalization, viewport update, history flush, and
            // draw in a synchronized terminal update so users never see
            // clear/scroll/shrink intermediate frames. Uses
            // prepare_draw_transaction which is the SAME function tests call,
            // ensuring ordering parity between production and tests.
            let mut resize_observed_this_frame = false;
            std::io::stdout().sync_update(|_| {
                let txn = prepare_draw_transaction(
                    &mut terminal,
                    &mut state,
                    transcript_pager_terminal_mode.is_active(),
                )?;
                resize_observed_this_frame = txn.resize_observed;
                if txn.history_flushed {
                    mark_redraw(
                        &mut needs_user_redraw,
                        &mut redraw_reasons,
                        "history_flush",
                    );
                }
                if txn.viewport_mutated && !txn.history_flushed {
                    mark_redraw(
                        &mut needs_user_redraw,
                        &mut redraw_reasons,
                        "viewport_change",
                    );
                }
                let should_draw = txn.terminal_mutated()
                    || needs_user_redraw
                    || (needs_stream_redraw && !state.rendering_paused());
                if should_draw {
                    needs_user_redraw = false;
                    needs_stream_redraw = false;
                    let mut frame_reasons = std::mem::take(&mut redraw_reasons);
                    if frame_reasons.is_empty() {
                        frame_reasons.push("unspecified");
                    }
                    let frame_event_counts = std::mem::take(&mut render_event_counts);
                    execute!(terminal.backend_mut(), state.cursor_style())?;
                    if let Some(metrics) = render_metrics.as_mut() {
                        let draw_metrics = terminal.draw_with_metrics(|frame| state.draw(frame))?;
                        let size = terminal.size()?;
                        let context = state.render_metrics_context(
                            &terminal,
                            size,
                            &frame_reasons,
                            frame_event_counts,
                        );
                        metrics.record_frame(&draw_metrics, context)?;
                    } else {
                        terminal.draw(|frame| state.draw(frame))?;
                    }
                }
                anyhow::Ok(())
            })??;
            if resize_observed_this_frame {
                resize_settle_deadline = Some(time::Instant::now() + resize_settle);
            }
            if final_answer_fixture_enabled && !final_answer_fixture_active_drawn {
                final_answer_fixture_active_drawn = true;
                mark_redraw(
                    &mut needs_stream_redraw,
                    &mut redraw_reasons,
                    "final_answer_fixture_active_frame",
                );
            }

            let stream_paused = state.rendering_paused();
            tokio::select! {
                _ = async {
                    match resize_settle_deadline {
                        Some(deadline) => time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => {
                    resize_settle_deadline = None;
                    // Every observed resize defers its source-backed rebuild to
                    // this settle deadline; reflow_pending marks that work.
                    if state.resize_reflow_pending() {
                        state.rebuild_committed_history_from_source();
                        mark_redraw(&mut needs_user_redraw, &mut redraw_reasons, "resize_settle_reflow");
                    }
                }
                _ = animation_tick.tick(), if state.needs_periodic_tick() && !stream_paused => {
                    state.on_tick();
                    mark_redraw(&mut needs_stream_redraw, &mut redraw_reasons, "animation_tick");
                }
                _ = task_panel_poll.tick(), if !stream_paused => {
                    state.task_panel.mark_dirty();
                    if state.task_panel.tick(std::time::Instant::now()) {
                        mark_redraw(
                            &mut needs_stream_redraw,
                            &mut redraw_reasons,
                            "task_panel_hide",
                        );
                    }
                }
                _ = background_agent_poll.tick(), if !stream_paused => {
                    let now = std::time::Instant::now();
                    state.background_agent_panel.mark_dirty();
                    if state.background_agent_panel.tick(now) {
                        mark_redraw(
                            &mut needs_stream_redraw,
                            &mut redraw_reasons,
                            "background_agent_panel_hide",
                        );
                    }
                    if state.transcript_task_cards.tick(now) {
                        mark_redraw(
                            &mut needs_stream_redraw,
                            &mut redraw_reasons,
                            "transcript_task_cards_hide",
                        );
                    }
                }
                _ = statusline_cmd_tick.tick(), if state.statusline_command.is_some() && !statusline_cmd_in_flight && !stream_paused => {
                    statusline_cmd_in_flight = true;
                    let Some(cmd) = state.statusline_command.clone() else {
                        statusline_cmd_in_flight = false;
                        continue;
                    };
                    let cwd = state.cwd.clone();
                    let tx = statusline_cmd_tx.clone();
                    // Detached statusline command; completion is delivered
                    // through statusline_cmd_rx and guarded by in-flight state.
                    let _statusline_cmd_handle = tokio::spawn(async move {
                        let _ = tx.send(run_statusline_command(&cmd, &cwd).await);
                    });
                }
                maybe_cmd_output = statusline_cmd_rx.recv() => {
                    if let Some(output) = maybe_cmd_output {
                        statusline_cmd_in_flight = false;
                        if state.status.custom_command_output != output {
                            state.status.custom_command_output = output;
                            mark_redraw(
                                &mut needs_stream_redraw,
                                &mut redraw_reasons,
                                "statusline_cmd",
                            );
                        }
                    }
                }
                maybe_terminal_event = terminal_events.recv() => {
                    match maybe_terminal_event {
                        Some(terminal_event) => {
                            render_event_counts.terminal_events += 1;
                            if !handle_terminal_event(
                                &mut state, &client,
                                &mut terminal,
                                &terminal_reader_paused,
                                terminal_event,
                                &mut turn_events,
                                &local_command_tx,
                                &mut needs_user_redraw,
                                &mut redraw_reasons,
                            ).await? {
                                break 'main;
                            }
                            while let Ok(terminal_event) = terminal_events.try_recv() {
                                render_event_counts.terminal_events += 1;
                                if !handle_terminal_event(
                                    &mut state, &client,
                                    &mut terminal,
                                    &terminal_reader_paused,
                                    terminal_event,
                                    &mut turn_events,
                                    &local_command_tx,
                                    &mut needs_user_redraw,
                                    &mut redraw_reasons,
                                ).await? {
                                    break 'main;
                                }
                            }
                        }
                        None => break,
                    }
                }
                maybe_stream_event = async {
                    match &mut turn_events {
                        Some(receiver) => receiver.recv().await,
                        None => None,
                    }
                }, if turn_events.is_some() && !stream_paused => {
                    handle_stream_event_batch(
                        &mut state,
                        &mut turn_events,
                        maybe_stream_event,
                        &mut render_event_counts,
                        &mut needs_stream_redraw,
                        &mut redraw_reasons,
                    );
                    state
                        .submit_queued_followups_if_idle(&client, &mut turn_events)
                        .await?;
                }
                maybe_background_task_event = background_task_event_rx.recv(), if !stream_paused => {
                    handle_background_task_event_batch(
                        &mut state,
                        maybe_background_task_event,
                        &mut background_task_event_rx,
                        &mut render_event_counts,
                        &mut needs_stream_redraw,
                        &mut redraw_reasons,
                    );
                }
                maybe_local_command_event = local_command_rx.recv(), if !stream_paused => {
                    if let Some(event) = maybe_local_command_event {
                        handle_local_command_event_batch(
                            &mut state, &client,
                            &mut turn_events,
                            event,
                            &mut local_command_rx,
                            &mut render_event_counts,
                            &mut needs_stream_redraw,
                            &mut redraw_reasons,
                        ).await?;
                    }
                }
            }
        }

        Ok(())
    }
    .await;

    if transcript_pager_terminal_mode.is_active() {
        let _ = sync_transcript_pager_terminal_mode(
            &mut terminal,
            false,
            &mut transcript_pager_terminal_mode,
        );
    }
    let session_id = state.session_id.clone();
    terminal_shutdown.store(true, Ordering::SeqCst);
    let _ = terminal_reader.await;
    let cleanup_result = prepare_terminal_for_cli_output(&mut terminal);
    let restore_result = restore_terminal(&mut terminal);
    run_result?;
    cleanup_result?;
    restore_result?;
    Ok(session_id)
}

fn subscribe_pending_transcript_task_cards(
    state: &mut TuiState,
    client: &Arc<AppClient>,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) {
    for task_id in state.transcript_task_cards.drain_subscription_requests() {
        let client = Arc::clone(client);
        let tx = tx.clone();
        let _task_stream_handle = tokio::spawn(async move {
            let Ok(mut rx) = client.subscribe_background_task_stream(&task_id).await else {
                return;
            };
            while let Some(event) = rx.recv().await {
                let terminal = background_task_update_is_terminal(&event);
                if tx.send(event).is_err() || terminal {
                    break;
                }
            }
        });
    }
}

fn handle_background_task_event_batch(
    state: &mut TuiState,
    first_event: Option<StreamEvent>,
    rx: &mut mpsc::UnboundedReceiver<StreamEvent>,
    event_counts: &mut RenderEventCounts,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) {
    let Some(first_event) = first_event else {
        return;
    };
    apply_background_task_event(
        state,
        first_event,
        event_counts,
        needs_redraw,
        redraw_reasons,
    );
    while let Ok(event) = rx.try_recv() {
        apply_background_task_event(state, event, event_counts, needs_redraw, redraw_reasons);
    }
}

fn apply_background_task_event(
    state: &mut TuiState,
    event: StreamEvent,
    event_counts: &mut RenderEventCounts,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) {
    event_counts.stream_events += 1;
    let _ = state.apply_stream_event(event);
    mark_redraw(needs_redraw, redraw_reasons, "background_task_stream");
}

fn background_task_update_is_terminal(event: &StreamEvent) -> bool {
    match event {
        StreamEvent::BackgroundTaskUpdated { task, .. } => !task.status.is_active(),
        _ => false,
    }
}

async fn run_statusline_command(command: &str, cwd: &Path) -> Option<String> {
    let result = tokio::time::timeout(
        STATUSLINE_CMD_TIMEOUT,
        tokio::process::Command::new("sh")
            .args(["-c", command])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let line = stdout.lines().next()?.trim();
            if line.is_empty() {
                None
            } else {
                Some(line.to_string())
            }
        }
        _ => None,
    }
}

fn install_manual_scrollback_fixture(state: &mut TuiState) {
    for index in 0..48 {
        state.push_message_and_flush_history(TranscriptMessage::new(
            MessageRole::User,
            format!("manual scrollback fixture prompt {index:02}: validate native scrollback"),
        ));
        state.push_message_and_flush_history(TranscriptMessage::new(
            MessageRole::Assistant,
            format!(
                "manual scrollback fixture summary {index:02}: resize this tmux pane and confirm \
                 retained summary history reflows while the live prompt remains usable"
            ),
        ));
    }
    state.set_status_line("Manual scrollback fixture loaded for tmux validation");
}

fn install_pty_smoke_history_summary(state: &mut TuiState) {
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::Assistant,
        "PTY smoke finalized summary scrollback line".to_string(),
    ));
}

fn install_resize_streaming_fixture(state: &mut TuiState) {
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::User,
        "resize streaming committed prompt marker".to_string(),
    ));
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::Assistant,
        "resize streaming committed history marker".to_string(),
    ));
    state.request_in_flight = true;
    state.request_started_at = Some(std::time::Instant::now());
    state.active_thinking = Some(crate::prompt_state::ActiveThinkingState {
        text: "active resize smoke thought should remain live only".to_string(),
        is_streaming: true,
        completed_at: None,
    });
}

fn install_final_answer_fixture(state: &mut TuiState) {
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::User,
        "produce the final answer native scrollback fixture".to_string(),
    ));
    state.request_in_flight = true;
    state.request_started_at = Some(std::time::Instant::now());
    state.active_thinking = Some(crate::prompt_state::ActiveThinkingState {
        text: "final answer fixture thinking should stay out of committed scrollback".to_string(),
        is_streaming: false,
        completed_at: Some(std::time::Instant::now()),
    });
    state.pending_assistant = final_answer_fixture_text();
}

fn final_answer_fixture_text() -> String {
    std::iter::once(FINAL_ANSWER_FIXTURE_HEAD.to_string())
        .chain((1..=24).map(|index| format!("final answer fixture body line {index:02}")))
        .chain(std::iter::once(FINAL_ANSWER_FIXTURE_TAIL.to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn detect_git_branch(cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    if branch.is_empty() || branch == "HEAD" {
        None
    } else {
        Some(branch.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_statusline_command_captures_first_line() {
        let dir = tempfile::tempdir().expect("temp dir");
        let result = run_statusline_command("echo hello", dir.path()).await;
        assert_eq!(result.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn run_statusline_command_takes_first_line_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let result = run_statusline_command("printf 'first\\nsecond\\nthird'", dir.path()).await;
        assert_eq!(result.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn run_statusline_command_returns_none_on_failure() {
        let dir = tempfile::tempdir().expect("temp dir");
        let result = run_statusline_command("exit 1", dir.path()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn run_statusline_command_returns_none_on_empty_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let result = run_statusline_command("echo ''", dir.path()).await;
        assert_eq!(result, None);
    }
}
