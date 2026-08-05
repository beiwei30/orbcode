use super::support::*;
use crate::prompt_state::DeferredAssistantMessage;
use std::time::Instant;

#[test]
fn empty_prompt_desired_height_is_compact() {
    let mut state = normal_state("", 0);
    let desired = state.desired_viewport_height(80, 24);
    assert!(
        desired <= 12,
        "empty prompt desired height should be compact, not full terminal: {desired}"
    );
}

#[test]
fn empty_prompt_frame_has_no_large_blank_gap() {
    let mut state = normal_state("", 0);
    let screen = draw_at_content_height(&mut state, 80, 24);

    let gap = max_blank_gap(&screen);
    assert!(
        gap <= 2,
        "empty prompt frame should not have more than 2 blank rows between content: gap={gap}\n{screen:#?}"
    );
    assert!(
        screen_has_input_chrome(&screen),
        "empty prompt should show input chrome\n{screen:#?}"
    );
}

#[test]
fn committed_messages_then_empty_prompt_has_no_large_gap() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "world".to_string(),
    ));
    state.pending_history_flush = true;

    let screen = draw_at_content_height(&mut state, 80, 24);
    let gap = max_blank_gap(&screen);
    assert!(
        gap <= 2,
        "after committed messages, frame should not have large blank gap: gap={gap}\n{screen:#?}"
    );
}

#[test]
fn resume_initial_transaction_flushes_loaded_history_to_scrollback() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.messages = (0..40)
        .map(|index| {
            let text = if index == 4 {
                "可以看完整历史".to_string()
            } else {
                format!("resume history line {index:02}")
            };
            TranscriptMessage::new(MessageRole::Assistant, text)
        })
        .collect();
    state.queue_existing_history_flush();

    let txn = prepare_draw_transaction(&mut terminal, &mut state, false)
        .expect("resume initial transaction");
    assert!(txn.history_flushed);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw resumed frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    let scrollback = screen.scrollback_lines();
    let full_terminal = screen.full_contents().join("\n");
    assert!(!scrollback.is_empty(), "{full_terminal}");
    assert!(
        full_terminal.contains("resume history line 00"),
        "{full_terminal}"
    );
    assert!(
        full_terminal.contains("resume history line 39"),
        "{full_terminal}"
    );
    assert!(full_terminal.contains("可以看完整历史"), "{full_terminal}");
    assert!(!full_terminal.contains("可 以 看"), "{full_terminal}");
    assert!(
        input_chrome_is_at_bottom(&terminal.screen_lines()),
        "viewport should end at the live prompt after resume history flush\n{:#?}",
        terminal.screen_lines()
    );
}

fn startup_terminal_with_pretui_rows(
    width: u16,
    height: u16,
    pretui_rows: u16,
) -> (Terminal<RenderFixtureBackend>, TerminalScreenModel) {
    let backend = RenderFixtureBackend::new(width, height);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    let mut screen = TerminalScreenModel::new(width, height);
    for i in 1..=pretui_rows {
        screen.process_bytes(format!("PRETUI-{i:02}\r\n").as_bytes());
    }
    terminal.set_viewport_area(Rect::new(0, pretui_rows, width, 1));
    terminal.last_known_cursor_pos = Position::new(0, pretui_rows);
    (terminal, screen)
}

#[test]
fn startup_slash_command_flush_pushes_pretui_scrollback_up_with_single_blank() {
    // End-to-end (screen-model) regression for startup-window scrollback
    // preservation on a normal terminal. Pre-seed pre-TUI shell output that
    // fills toward the bottom, launch (banner viewport grows upward → reserve),
    // then commit a local slash command (first flush). Assert the OUTCOME, not
    // just emitted bytes: pre-TUI is pushed UP into native scrollback (preserved,
    // not eaten), the banner is committed once, order is preserved, and the
    // committed transcript is contiguous (single-blank spacing, no stranded gap).
    //
    // `TerminalScreenModel` pushes a scrolled-off row into scrollback only on a
    // full-screen scroll (top margin at row 0), so this discriminates the reserve
    // full-screen scroll (preserves) from the old clear / sub-region scroll (eats).
    let width = 90;
    let height = 24;
    let pretui_rows = 18u16;
    let (mut terminal, mut screen) = startup_terminal_with_pretui_rows(width, height, pretui_rows);
    let mut state = normal_state("", 0);

    // Frame 1: draw the banner from the launch cursor, matching `setup_terminal`.
    run_prompt_transition(&mut state, &mut terminal, &mut screen);
    terminal.backend_mut().output.clear();

    // Frame 2: typing `/` opens the startup slash-suggestion panel. The idle
    // viewport grows upward and must reserve the terminal-owned rows it covers.
    state.input = "/".to_string();
    state.input_cursor = state.input.len();
    let suggestions = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    terminal.backend_mut().output.clear();

    let after_suggestions = suggestions.full_terminal_screen;
    let after_suggestions_text = after_suggestions.join("\n");
    for i in 1..=pretui_rows {
        assert!(
            after_suggestions_text.contains(&format!("PRETUI-{i:02}")),
            "slash suggestions must preserve pre-TUI row {i}:\n{after_suggestions:#?}"
        );
    }
    assert!(
        screen
            .scrollback_lines()
            .iter()
            .any(|line| line.contains("PRETUI-")),
        "slash suggestions must push terminal-owned rows into model scrollback:\n{after_suggestions:#?}"
    );
    let last_pretui_after_suggestions = after_suggestions_text
        .rfind(&format!("PRETUI-{pretui_rows:02}"))
        .expect("last pre-TUI row after suggestions");
    let banner_after_suggestions = after_suggestions_text
        .find("Orb Code")
        .expect("banner after suggestions");
    assert!(
        last_pretui_after_suggestions < banner_after_suggestions,
        "slash suggestions must push pre-TUI rows above the banner:\n{after_suggestions:#?}"
    );

    // Frame 3: completing the command closes the suggestions and shrinks the
    // viewport without bottom-pinning it during the startup window.
    state.input = "/allow-all on".to_string();
    state.input_cursor = state.input.len();
    run_prompt_transition(&mut state, &mut terminal, &mut screen);
    terminal.backend_mut().output.clear();

    // Frame 4: submit the local command and flush its output as the first
    // committed history.
    state.input.clear();
    state.input_cursor = 0;
    state.push_local_slash_command_output("/allow-all on", "Allow-all mode enabled.", None);
    let flush = run_prompt_transition(&mut state, &mut terminal, &mut screen);

    let full = flush.full_terminal_screen;
    let text = full.join("\n");

    // 1. Every pre-TUI row survives (pushed into native scrollback, not eaten).
    for i in 1..=pretui_rows {
        assert!(
            text.contains(&format!("PRETUI-{i:02}")),
            "pre-TUI row {i} must be preserved in scrollback, not eaten:\n{full:#?}"
        );
    }
    // 2. Intro banner committed exactly once (not duplicated).
    assert_eq!(
        text.matches("Orb Code").count(),
        1,
        "intro banner must appear exactly once:\n{full:#?}"
    );
    // 3. Pre-TUI stays ABOVE the banner (history pushed up, not overwritten).
    let last_pretui = text
        .rfind(&format!("PRETUI-{pretui_rows:02}"))
        .expect("last pre-TUI row present");
    let banner = text.find("Orb Code").expect("banner present");
    assert!(
        last_pretui < banner,
        "pre-TUI history must remain above the banner:\n{full:#?}"
    );
    // 4. The committed local-command output is present.
    assert!(
        text.contains("Allow-all mode enabled."),
        "committed command output must be present:\n{full:#?}"
    );
    // 5. The banner and local-command history cells have exactly one blank row
    // between them, and no other committed content has a stranded blank band.
    let tip = full
        .iter()
        .position(|line| line.contains("Tip:"))
        .expect("banner tip present");
    let command = full
        .iter()
        .position(|line| line.contains("❯ /allow-all on"))
        .expect("committed slash command present");
    assert_eq!(
        command,
        tip + 2,
        "banner and slash-command cells must have exactly one blank row between them:\n{full:#?}"
    );
    assert!(
        full[tip + 1].trim().is_empty(),
        "the row between banner and slash-command cells must be blank:\n{full:#?}"
    );
    let gap = max_blank_gap(&full);
    assert!(
        gap <= 2,
        "committed transcript must be contiguous (no stranded gap), got max blank gap {gap}:\n{full:#?}"
    );
}

#[test]
fn first_long_user_flush_preserves_all_history_and_pretui_scrollback() {
    let width = 90;
    let height = 24;
    let pretui_rows = 18u16;
    let history_rows = 30u16;
    let (mut terminal, mut screen) = startup_terminal_with_pretui_rows(width, height, pretui_rows);
    let mut state = normal_state("", 0);

    run_prompt_transition(&mut state, &mut terminal, &mut screen);
    terminal.backend_mut().output.clear();

    let user_message = (1..=history_rows)
        .map(|row| format!("FIRST-HISTORY-{row:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    state
        .messages
        .push(TranscriptMessage::new(MessageRole::User, user_message));
    state.pending_history_flush = true;
    state.request_in_flight = true;

    let flush = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let full = flush.full_terminal_screen;
    let text = full.join("\n");

    for row in 1..=pretui_rows {
        let marker = format!("PRETUI-{row:02}");
        assert_eq!(
            text.matches(&marker).count(),
            1,
            "first user flush must preserve pre-TUI row {row}:\n{full:#?}"
        );
    }
    for row in 1..=history_rows {
        let marker = format!("FIRST-HISTORY-{row:02}");
        assert_eq!(
            text.matches(&marker).count(),
            1,
            "first user flush must emit history row {row} exactly once:\n{full:#?}"
        );
    }
    assert_eq!(
        text.matches("Orb Code").count(),
        1,
        "first user flush must preserve the intro banner exactly once:\n{full:#?}"
    );

    let last_pretui = text.rfind("PRETUI-18").expect("last pre-TUI row");
    let banner = text.find("Orb Code").expect("intro banner");
    let first_history = text.find("FIRST-HISTORY-01").expect("first history row");
    let last_history = text.find("FIRST-HISTORY-30").expect("last history row");
    assert!(
        last_pretui < banner && banner < first_history && first_history < last_history,
        "pre-TUI, banner, and complete first history must retain order:\n{full:#?}"
    );
}

#[test]
fn tui_exit_clears_live_viewport_before_resume_hint() {
    let width = 88;
    let backend = RenderFixtureBackend::new(width, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    let mut screen = TerminalScreenModel::new(width, 24);
    let mut state = normal_state("", 0);
    state.messages = (0..40)
        .map(|index| {
            TranscriptMessage::new(
                MessageRole::Assistant,
                format!("resume exit history line {index:02}"),
            )
        })
        .collect();
    state.queue_existing_history_flush();

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("resume initial transaction");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw resumed frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();
    assert!(
        screen_has_input_chrome(&screen.screen_lines()),
        "resume frame should start with visible input chrome\n{:#?}",
        screen.screen_lines()
    );

    prepare_terminal_for_cli_output(&mut terminal).expect("prepare terminal for cli output");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    let hint = "To continue this session, run orbcode resume f86364c2-5192-4b3a-9513-565c786765c4";
    screen.process_bytes(format!("\n{hint}\n").as_bytes());

    let full = screen.full_contents();
    let hint_lines = full
        .iter()
        .filter(|line| line.contains("To continue this session"))
        .collect::<Vec<_>>();
    assert_eq!(
        hint_lines.len(),
        1,
        "resume hint should be written once\n{full:#?}"
    );
    assert_eq!(
        hint_lines[0].as_str(),
        hint,
        "resume hint should be written to a clean line without stale input chrome\n{full:#?}"
    );
    assert!(
        !screen_has_input_chrome(&screen.screen_lines()),
        "exit cleanup should remove the live prompt before CLI output\n{:#?}",
        screen.screen_lines()
    );
}

#[test]
fn tui_exit_after_tall_table_answer_leaves_no_residual_below_hint() {
    let width = 88;
    let backend = RenderFixtureBackend::new(width, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    let mut screen = TerminalScreenModel::new(width, 24);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "top 10 rust files".to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;

    let mut table = String::from(
        "Here are the top 10 .rs files:\n\n| Rank | File | Lines |\n| --- | --- | --- |\n",
    );
    for index in 1..=10 {
        table.push_str(&format!(
            "| {index} | crate/src/module_{index:02}.rs | {} |\n",
            5000 - index * 137
        ));
    }
    state.pending_assistant = table.clone();

    for _ in 0..3 {
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("stream prepare");
        terminal
            .draw(|frame| state.draw(frame))
            .expect("stream draw");
        screen.process_bytes(terminal.backend_mut().output.as_slice());
        terminal.backend_mut().output.clear();
    }

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, table.clone()),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    for _ in 0..2 {
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("commit prepare");
        terminal
            .draw(|frame| state.draw(frame))
            .expect("commit draw");
        screen.process_bytes(terminal.backend_mut().output.as_slice());
        terminal.backend_mut().output.clear();
    }

    // Ctrl-C exit cleanup, then the resume hint printed to stderr.
    prepare_terminal_for_cli_output(&mut terminal).expect("exit cleanup");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();
    let hint = "To continue this session, run orbcode resume abcd";
    screen.process_bytes(format!("\n{hint}\n").as_bytes());

    let full = screen.full_contents();
    let hint_idx = full
        .iter()
        .position(|line| line.contains("To continue this session"))
        .expect("resume hint present");
    let residual_below = full.iter().skip(hint_idx + 1).any(|line| {
        line.contains("Rank") || line.contains("module_") || line.contains("Here are the top")
    });
    assert!(
        !residual_below,
        "no committed answer content may remain below the resume hint on exit:\n{full:#?}"
    );
}

#[test]
fn tall_streaming_shrinks_to_short_without_large_gap() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant =
        "line one\nline two\nline three\nline four\nline five\nline six\nline seven\nline eight"
            .to_string();

    let tall_desired = state.desired_viewport_height(80, 24);

    state.pending_assistant = "short".to_string();
    let short_desired = state.desired_viewport_height(80, 24);

    assert!(
        short_desired < tall_desired || short_desired <= 12,
        "desired height should shrink when content shrinks: tall={tall_desired}, short={short_desired}"
    );

    let screen = draw_at_content_height(&mut state, 80, 24);
    let gap = max_blank_gap(&screen);
    assert!(
        gap <= 2,
        "shrinking from tall to short content should not leave large blank gap: gap={gap}\n{screen:#?}"
    );
}

#[test]
fn sticky_bottom_pin_clears_after_content_shrinks() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    let tall_content: String = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.pending_assistant = tall_content;

    let _ = state.desired_viewport_height(80, 24);
    let tall_desired = state.desired_viewport_height(80, 24);

    state.pending_assistant = "short".to_string();
    state.request_in_flight = false;
    let short_desired = state.desired_viewport_height(80, 24);

    assert!(
        short_desired < tall_desired.min(24),
        "after tall content shrinks and turn ends, desired height should shrink: \
         tall={tall_desired}, short={short_desired}"
    );
}

#[test]
fn final_assistant_commit_frame_has_no_large_gap() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = "streaming answer".to_string();

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "final answer".to_string()),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let screen = draw_at_content_height(&mut state, 80, 24);
    let gap = max_blank_gap(&screen);
    assert!(
        gap <= 2,
        "final assistant commit frame should not have large blank gap: gap={gap}\n{screen:#?}"
    );
    assert!(
        screen_has_input_chrome(&screen),
        "input chrome should be visible after turn completes\n{screen:#?}"
    );
}

#[test]
fn long_streaming_answer_scrollback_has_no_chrome_and_final_is_not_duplicated() {
    let backend = RenderFixtureBackend::new(100, 32);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 8, 100, 24));
    let mut screen = TerminalScreenModel::new(100, 32);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "produce a long report".to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "completed reasoning".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });
    let answer = std::iter::once("codex-tui-regression-title".to_string())
        .chain((1..=36).map(|index| format!("streaming report line {index:02}")))
        .collect::<Vec<_>>()
        .join("\n");
    state.pending_assistant = answer.clone();

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("streaming prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("streaming draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    let live_tail = plain_text_lines(&state.pending_assistant_live_lines(100)).join("\n");
    assert!(
        live_tail.contains("codex-tui-regression-title"),
        "active assistant output should remain live until completion: {live_tail}"
    );
    assert!(
        live_tail.contains("streaming report line 36"),
        "latest streaming line should remain live: {live_tail}"
    );
    let streaming_scrollback = screen.scrollback_lines().join("\n");
    assert!(
        !streaming_scrollback.contains("codex-tui-regression-title"),
        "active assistant output should not be written to native scrollback before completion:\n{streaming_scrollback}"
    );
    for (i, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_scrollback_chrome(line),
            "scrollback row {i} contains chrome during streaming: {line:?}"
        );
    }

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("final prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("final draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    let full_terminal = screen.full_contents().join("\n");
    assert_eq!(
        full_terminal.matches("codex-tui-regression-title").count(),
        1,
        "completed source-backed answer should not duplicate streamed history:\n{full_terminal}"
    );
    assert_eq!(
        full_terminal.matches("streaming report line 36").count(),
        1,
        "completed source-backed answer should not duplicate the live tail:\n{full_terminal}"
    );
    for (i, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_scrollback_chrome(line),
            "scrollback row {i} contains chrome after completion: {line:?}"
        );
    }
    assert_eq!(
        input_chrome_count(&terminal.screen_lines()),
        1,
        "live viewport should keep a single input row after completion\n{:#?}",
        terminal.screen_lines()
    );
}

#[test]
fn incremental_streaming_commits_prefix_to_scrollback_and_keeps_tail_live() {
    let backend = RenderFixtureBackend::new(100, 32);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 8, 100, 24));
    let mut screen = TerminalScreenModel::new(100, 32);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "produce a long report".to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;
    let answer = std::iter::once("stream-commit-title".to_string())
        .chain((1..=36).map(|index| format!("stream commit line {index:02}")))
        .collect::<Vec<_>>()
        .join("\n");
    state.pending_assistant = answer.clone();

    // Frame 1 commits the user message / banner (emitted_cell_count 0 -> 1); the
    // banner gate keeps streaming out of this first flush. Frame 2 then commits
    // the streaming stable prefix incrementally.
    for _ in 0..2 {
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("streaming prepare");
        terminal
            .draw(|frame| state.draw(frame))
            .expect("streaming draw");
        screen.process_bytes(terminal.backend_mut().output.as_slice());
        terminal.backend_mut().output.clear();
    }

    let streaming_scrollback = screen.scrollback_lines().join("\n");
    // With the flag on, the stable prefix is now in native scrollback...
    assert!(
        streaming_scrollback.contains("stream commit line 01"),
        "stable prefix should be committed to native scrollback while streaming:\n{streaming_scrollback}"
    );
    // ...but the growing tail stays live (not in scrollback).
    assert!(
        !streaming_scrollback.contains("stream commit line 36"),
        "the live tail must not be committed to scrollback mid-stream:\n{streaming_scrollback}"
    );
    let live_tail = plain_text_lines(&state.pending_assistant_live_lines(100)).join("\n");
    assert!(
        live_tail.contains("stream commit line 36"),
        "latest streaming line should remain live: {live_tail}"
    );
    for (i, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_scrollback_chrome(line),
            "scrollback row {i} contains chrome during incremental streaming: {line:?}"
        );
    }

    // Completion must not duplicate the incrementally-committed prefix.
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("final prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("final draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    let full_terminal = screen.full_contents().join("\n");
    assert_eq!(
        full_terminal.matches("stream commit line 01").count(),
        1,
        "incrementally-committed prefix must not duplicate at completion:\n{full_terminal}"
    );
    assert_eq!(
        full_terminal.matches("stream commit line 36").count(),
        1,
        "final tail must appear exactly once:\n{full_terminal}"
    );
    for (i, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_scrollback_chrome(line),
            "scrollback row {i} contains chrome after completion: {line:?}"
        );
    }
    assert_eq!(
        input_chrome_count(&terminal.screen_lines()),
        1,
        "live viewport should keep a single input row after completion\n{:#?}",
        terminal.screen_lines()
    );
}

#[test]
fn transcript_pager_active_does_not_prepare_incremental_history() {
    // Regression (Medium #1): while the transcript pager owns the terminal the
    // physical flush is skipped. Preparing incremental history anyway would queue
    // assistant lines that never get emitted, desyncing the queued vs emitted
    // counters — a completion arriving while the pager is open would then look
    // "started" and duplicate the prefix + full final cell + tail on pager close.
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prior committed message".to_string(),
    ));
    state.pending_history_flush = true;
    // Commit a first cell so streaming is past the banner first-flush gate.
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("commit first cell");
    terminal.draw(|frame| state.draw(frame)).expect("draw");
    terminal.backend_mut().output.clear();

    state.request_in_flight = true;
    state.pending_assistant = (0..20)
        .map(|index| format!("streamed line {index:02}\n"))
        .collect();

    let pending_before = state
        .transcript_ui
        .emission
        .assistant_stream_pending_line_count;
    let emitted_before = state
        .transcript_ui
        .emission
        .assistant_stream_emitted_line_count;

    // Pager active: no incremental history may be prepared or queued.
    prepare_draw_transaction(&mut terminal, &mut state, true).expect("pager-active prepare");
    assert_eq!(
        state
            .transcript_ui
            .emission
            .assistant_stream_pending_line_count,
        pending_before,
        "no incremental history may be prepared while the pager owns the terminal"
    );
    assert_eq!(
        state
            .transcript_ui
            .emission
            .assistant_stream_emitted_line_count,
        emitted_before,
        "the pager frame must not advance the emitted-line counter"
    );

    // Pager closed: incremental preparation resumes.
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("pager-closed prepare");
    terminal.draw(|frame| state.draw(frame)).expect("draw");
    assert!(
        state
            .transcript_ui
            .emission
            .assistant_stream_emitted_line_count
            > 0,
        "incremental history should resume once the pager closes"
    );
}

#[test]
fn ordinary_streaming_with_visible_history_can_grow_past_one_row() {
    // A blanket "do not grow upward" cap regressed normal streaming to a
    // single changing row whenever committed process text filled the screen.
    // Ordinary prose has an incrementally-committed stable prefix, so its live
    // viewport must still be allowed to grow.
    let backend = RenderFixtureBackend::new(60, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 23, 60, 1));
    terminal.note_history_rows_inserted(23);
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (1..=8)
        .map(|index| format!("ordinary streaming line {index}\n"))
        .collect();

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare prose growth");

    assert!(
        terminal.viewport_area.height > 1,
        "ordinary streaming must not collapse to one row when visible history exists: {:?}",
        terminal.viewport_area
    );
    let output = terminal.backend_mut().output_string();
    assert!(
        output.contains("\x1b[r"),
        "ordinary streaming growth should reset the scroll region to full screen (ESC[r) so \
         displaced history is pushed into native scrollback: {output:?}"
    );
    assert!(
        !output.contains("\x1b[1;23r"),
        "growth must not use a DECSTBM sub-region scroll, which discards scrolled-off rows \
         instead of sending them to native scrollback: {output:?}"
    );
}

#[test]
fn streaming_growth_waiting_for_resize_settle_preserves_terminal_owned_history() {
    // Regression (High #1): a frame immediately after SIGWINCH can observe the
    // new stable size while the debounced source rebuild is still pending. The
    // rows above the viewport remain terminal/tmux-owned until settle. Live
    // growth must NOT climb into them: doing so would neither scroll them into
    // native scrollback (correct — tmux owns them) nor clear+overdraw them
    // (the bug), which erases history before the settle rebuild. So the viewport
    // top must stay pinned above the terminal-owned rows until settle.
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 8, 80, 4));
    terminal.note_history_rows_inserted(8);
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (1..=16)
        .map(|index| format!("post-resize streaming line {index}\n"))
        .collect();
    state.mark_resize_reflow_pending();

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare post-resize growth");

    assert!(
        terminal.viewport_area.top() >= 8,
        "the live viewport must not grow up into terminal-owned history before settle: top={}",
        terminal.viewport_area.top()
    );
    assert_eq!(
        terminal.visible_history_rows(),
        8,
        "terminal-owned history rows must be preserved until the settle rebuild"
    );
    let output = terminal.backend_mut().output_string();
    assert!(
        !output.contains("\x1b[1;8r"),
        "post-resize frames must not scroll terminal-owned history before settle: {output:?}"
    );
}

#[test]
fn tall_held_live_answer_grows_without_eating_committed_process_text() {
    // A markdown table is held live until finalize because later rows can
    // change all column widths. Its viewport must still grow beyond one row,
    // while the committed process text it displaces moves into native
    // scrollback instead of being cleared.
    let backend = RenderFixtureBackend::new(60, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 22, 60, 2));
    let mut screen = TerminalScreenModel::new(60, 24);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "list the top files".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        (1..=4)
            .map(|n| format!("PROCESS-MARKER line {n}: analyzing the repository"))
            .collect::<Vec<_>>()
            .join("\n"),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;

    let mut committed = false;
    for _ in 0..12 {
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare process text");
        terminal
            .draw(|frame| state.draw(frame))
            .expect("draw process text");
        screen.process_bytes(terminal.backend_mut().output.as_slice());
        terminal.backend_mut().output.clear();
        if !state.pending_history_flush && state.transcript_ui.emission.pending_lines.is_empty() {
            committed = true;
            break;
        }
    }
    assert!(
        committed,
        "process text should commit before the answer streams"
    );
    let initial_viewport_height = terminal.viewport_area.height;

    let mut answer = String::from("| File | Lines |\n| --- | --- |\n");
    for index in 1..=20 {
        answer.push_str(&format!("| file_{index:02}.rs | {} |\n", index * 10));
        state.pending_assistant = answer.clone();
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare growth");
        terminal
            .draw(|frame| state.draw(frame))
            .expect("draw growth");
        screen.process_bytes(terminal.backend_mut().output.as_slice());
        terminal.backend_mut().output.clear();
    }

    let grown_viewport_height = terminal.viewport_area.height;
    assert!(
        grown_viewport_height > initial_viewport_height,
        "the held-live table should grow beyond the initial viewport instead of being clipped: \
         initial={initial_viewport_height}, final={}",
        grown_viewport_height
    );

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare completion");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw completion");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    let full = screen.full_contents().join("\n");
    for n in 1..=4 {
        assert_eq!(
            full.matches(&format!("PROCESS-MARKER line {n}")).count(),
            1,
            "process line {n} must survive the tall held-live answer exactly once:\n{full}"
        );
    }
    for marker in ["file_01.rs", "file_20.rs"] {
        assert_eq!(
            full.matches(marker).count(),
            1,
            "the finalized table should contain {marker} exactly once:\n{full}"
        );
    }
    assert!(
        full.contains('└'),
        "the live table tail should remain visible:\n{full}"
    );
    for (index, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_scrollback_chrome(line),
            "scrollback row {index} contains live viewport chrome: {line:?}"
        );
    }
}

#[test]
fn vt100_final_answer_head_and_tail_commit_once_without_live_chrome_interleave() {
    let backend = VT100Backend::new(100, 48);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 8, 100, 40));
    let mut state = normal_state("", 0);
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::User,
        "produce a vt100 visible report".to_string(),
    ));
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "completed vt100 reasoning".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });
    let answer = std::iter::once("vt100-final-answer-head".to_string())
        .chain((1..=18).map(|index| format!("vt100 final answer body line {index:02}")))
        .chain(std::iter::once("vt100-final-answer-tail".to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    state.pending_assistant = answer.clone();

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("streaming prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("streaming draw");

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("final prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("final draw");

    let screen = terminal.backend_mut().screen_lines();
    let text = screen.join("\n");
    assert_eq!(
        text.matches("vt100-final-answer-head").count(),
        1,
        "final answer head should appear exactly once on the real VT100 screen\n{text}"
    );
    assert_eq!(
        text.matches("vt100-final-answer-tail").count(),
        1,
        "final answer tail should appear exactly once on the real VT100 screen\n{text}"
    );
    let head = text.find("vt100-final-answer-head").expect("head visible");
    let tail = text.find("vt100-final-answer-tail").expect("tail visible");
    assert!(head < tail, "final answer head should precede tail\n{text}");
    let between = &text[head..tail];
    assert!(
        !between.contains("❯")
            && !between.contains("-- NORMAL --")
            && !between.contains("-- INSERT --")
            && !between.contains("Thinking"),
        "live prompt/status/thinking chrome should not interleave final answer head and tail\n{text}"
    );
    let live_prompt_count = screen.iter().filter(|line| line.starts_with("❯")).count();
    assert_eq!(
        live_prompt_count, 1,
        "final answer commit should leave one live prompt on the real VT100 screen\n{text}"
    );
    assert!(
        max_blank_gap(&screen) <= 2,
        "final answer commit should not leave a large visible blank gap\n{text}"
    );
}

#[test]
fn completed_tool_followed_by_next_cell_no_large_gap() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "run a command".to_string(),
    ));
    let mut tool_msg = TranscriptMessage::new(MessageRole::Assistant, String::new());
    tool_msg.blocks.push(TranscriptBlock::ToolUse {
        id: "tool-1".to_string(),
        name: "Bash".to_string(),
        input: serde_json::json!({"command": "echo hello"}).to_string(),
    });
    state.messages.push(tool_msg);
    let mut result_msg = TranscriptMessage::new(MessageRole::User, String::new());
    result_msg.blocks.push(TranscriptBlock::ToolResult {
        tool_use_id: "tool-1".to_string(),
        content: "hello".to_string().into(),
        is_error: false,
        metadata: None,
    });
    state.messages.push(result_msg);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "Done!".to_string(),
    ));
    state.pending_history_flush = true;

    let screen = draw_at_content_height(&mut state, 80, 24);
    let gap = max_blank_gap(&screen);
    assert!(
        gap <= 2,
        "completed tool + next cell should not have large blank gap: gap={gap}\n{screen:#?}"
    );
}

#[test]
fn screen_dividers_frame_input_area() {
    let mut state = normal_state("hello", 5);
    let screen = draw_at_content_height(&mut state, 80, 24);

    assert!(
        screen_has_divider(&screen),
        "screen should have divider lines around input\n{screen:#?}"
    );
    assert!(
        screen_has_input_chrome(&screen),
        "screen should show input chrome\n{screen:#?}"
    );
}

#[test]
fn normal_history_flush_updates_viewport_before_insert() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.push_local_slash_command_output(
        "/allow-all on",
        "Allow-all mode enabled.",
        Some(
            "Tool and network permission prompts are bypassed; configured deny rules still apply."
                .to_string(),
        ),
    );

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    assert!(
        result.viewport_area.height < 20,
        "viewport should shrink from old tall height after transition: {}",
        result.viewport_area.height
    );
    let buffer_gap = max_blank_gap(&result.buffer_screen);
    assert!(
        buffer_gap <= 2,
        "viewport buffer should have no large blank gap: gap={buffer_gap}\n{:#?}",
        result.buffer_screen
    );
    let screen_gap = max_blank_gap(&result.full_terminal_screen);
    assert!(
        screen_gap <= 2,
        "full terminal screen should have no large blank gap: gap={screen_gap}\n{:#?}",
        result.full_terminal_screen
    );
    assert_eq!(
        input_chrome_count(&result.buffer_screen),
        1,
        "input chrome should appear exactly once in viewport\n{:#?}",
        result.buffer_screen
    );
    assert!(
        input_chrome_is_at_bottom(&result.buffer_screen),
        "input chrome should be at bottom of live viewport\n{:#?}",
        result.buffer_screen
    );
}

#[test]
fn slash_command_transition_frame_has_no_large_gap() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.push_local_slash_command_output(
        "/allow-all on",
        "Allow-all mode enabled.",
        Some(
            "Tool and network permission prompts are bypassed; configured deny rules still apply."
                .to_string(),
        ),
    );

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let screen_gap = max_blank_gap(&result.full_terminal_screen);
    assert!(
        screen_gap <= 2,
        "/allow-all: full terminal screen gap should be <= 2: gap={screen_gap}\n{:#?}",
        result.full_terminal_screen
    );
    assert_eq!(
        input_chrome_count(&result.buffer_screen),
        1,
        "input chrome should appear exactly once\n{:#?}",
        result.buffer_screen
    );
    let scrollback = screen.scrollback_lines();
    for (i, line) in scrollback.iter().enumerate() {
        assert!(
            !is_chrome_line(line),
            "scrollback row {i} contains chrome: {line:?}"
        );
    }
}

#[test]
fn tall_active_to_short_brief_transition_has_no_large_gap() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (0..20)
        .map(|i| format!("streaming line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw tall frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "short final answer".to_string()),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    terminal.backend_mut().output.clear();

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let screen_gap = max_blank_gap(&result.full_terminal_screen);
    assert!(
        screen_gap <= 2,
        "tall-to-short: full terminal screen gap should be <= 2: gap={screen_gap}\n{:#?}",
        result.full_terminal_screen
    );
    assert!(
        input_chrome_is_at_bottom(&result.buffer_screen),
        "input chrome should be at bottom after shrink\n{:#?}",
        result.buffer_screen
    );
}

#[test]
fn completed_tool_then_next_active_has_no_chrome_interleave() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "run a command".to_string(),
    ));
    let mut tool_msg = TranscriptMessage::new(MessageRole::Assistant, String::new());
    tool_msg.blocks.push(TranscriptBlock::ToolUse {
        id: "tool-1".to_string(),
        name: "Bash".to_string(),
        input: serde_json::json!({"command": "echo hello"}).to_string(),
    });
    state.messages.push(tool_msg);
    let mut result_msg = TranscriptMessage::new(MessageRole::User, String::new());
    result_msg.blocks.push(TranscriptBlock::ToolResult {
        tool_use_id: "tool-1".to_string(),
        content: "hello".to_string().into(),
        is_error: false,
        metadata: None,
    });
    state.messages.push(result_msg);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "The command succeeded.".to_string(),
    ));
    state.pending_history_flush = true;

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let screen_gap = max_blank_gap(&result.full_terminal_screen);
    assert!(
        screen_gap <= 2,
        "completed tool: full terminal screen gap should be <= 2: gap={screen_gap}\n{:#?}",
        result.full_terminal_screen
    );
    let scrollback = screen.scrollback_lines();
    for (i, line) in scrollback.iter().enumerate() {
        assert!(
            !is_chrome_line(line),
            "scrollback row {i} contains chrome: {line:?}"
        );
    }
}

#[test]
fn app_loop_finalizes_before_desired_height() {
    let mut state = normal_state("", 0);
    state.pending_assistant.clear();
    state.request_in_flight = false;
    state.deferred_assistant_message = Some(DeferredAssistantMessage {
        message: TranscriptMessage::new(MessageRole::Assistant, "short final answer".to_string()),
    });

    let width = 80u16;
    let terminal_height = 24u16;

    let height_before_finalize = state.desired_viewport_height(width, terminal_height);

    state.finalize_deferred_assistant_message(width as usize, terminal_height);
    state.prune_completed_live_tool_activity();
    state.prepare_pending_history_emission(width as usize, terminal_height);

    let height_after_finalize = state.desired_viewport_height(width, terminal_height);

    assert!(
        height_after_finalize > height_before_finalize || state.messages.len() == 1,
        "after finalize commits a deferred message, the committed message should \
         increase transcript height or be present in messages: \
         before={height_before_finalize}, after={height_after_finalize}, \
         messages={}",
        state.messages.len(),
    );
    assert_eq!(
        state.messages.len(),
        1,
        "finalize should have committed the deferred message"
    );
    assert!(
        state.deferred_assistant_message.is_none(),
        "deferred_assistant_message should be consumed after finalize"
    );

    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen_model = TerminalScreenModel::new(80, 24);
    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen_model);
    let gap = max_blank_gap(&result.buffer_screen);
    assert!(
        gap <= 2,
        "after finalize + transition, no large gap: gap={gap}\n{:#?}",
        result.buffer_screen,
    );
}

#[test]
fn app_loop_terminal_mutation_always_draws() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut state = normal_state("", 0);
    state.push_local_slash_command_output(
        "/allow-all on",
        "Allow-all mode enabled.",
        Some(
            "Tool and network permission prompts are bypassed; configured deny rules still apply."
                .to_string(),
        ),
    );

    let size = terminal.size().expect("size");
    state.finalize_deferred_assistant_message(size.width as usize, size.height);
    state.prune_completed_live_tool_activity();
    state.prepare_pending_history_emission(size.width as usize, size.height);
    let desired = state.desired_viewport_height(size.width, size.height);
    let viewport_changed =
        update_inline_viewport_generic(&mut terminal, desired).expect("update viewport");
    let history_flushed = flush_pending_history_to_scrollback(
        &mut terminal,
        &mut state,
        size.width as usize,
        size.height,
    )
    .unwrap_or(false);
    let terminal_mutated = viewport_changed || history_flushed;

    assert!(
        terminal_mutated,
        "viewport change or history flush must be detected as a terminal mutation: \
         viewport_changed={viewport_changed}, history_flushed={history_flushed}"
    );
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw must succeed after mutation");
    let screen = terminal.screen_lines();
    assert!(
        screen_has_input_chrome(&screen),
        "after terminal mutation + draw, input chrome must be visible\n{screen:#?}"
    );
}

#[test]
fn tall_active_commit_transition_has_no_large_gap() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello".to_string(),
    ));
    state.request_in_flight = true;
    state.pending_assistant = (0..18)
        .map(|i| format!("long streaming line {i} with some content"))
        .collect::<Vec<_>>()
        .join("\n");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw tall streaming frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "short answer".to_string()),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    terminal.backend_mut().output.clear();

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let buffer_gap = max_blank_gap(&result.buffer_screen);
    assert!(
        buffer_gap <= 2,
        "tall active -> short commit: viewport buffer gap should be <= 2: gap={buffer_gap}\n{:#?}",
        result.buffer_screen
    );
    let screen_gap = max_blank_gap(&result.full_terminal_screen);
    assert!(
        screen_gap <= 2,
        "tall active -> short commit: full screen gap should be <= 2: gap={screen_gap}\n{:#?}",
        result.full_terminal_screen
    );
    assert_eq!(
        input_chrome_count(&result.buffer_screen),
        1,
        "input chrome should appear exactly once\n{:#?}",
        result.buffer_screen
    );
    assert!(
        input_chrome_is_at_bottom(&result.buffer_screen),
        "input chrome should be at bottom\n{:#?}",
        result.buffer_screen
    );
}

#[test]
fn vt100_tall_active_commit_transition_has_no_large_gap() {
    let backend = VT100Backend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let old_y = terminal.viewport_area.y;
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello".to_string(),
    ));
    state.request_in_flight = true;
    state.pending_assistant = (0..18)
        .map(|i| format!("long streaming line {i} with some content"))
        .collect::<Vec<_>>()
        .join("\n");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw tall streaming frame");

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "short answer".to_string()),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let result = run_prompt_transition_vt100(&mut state, &mut terminal);
    let terminal_gap = max_blank_gap(&result.terminal_screen);
    assert!(
        terminal_gap <= 2,
        "vt100 terminal screen gap should be <= 2 after tall active -> short commit: \
         gap={terminal_gap}\n{}",
        result.terminal_contents
    );
    let buffer_gap = max_blank_gap(&result.buffer_screen);
    assert!(
        buffer_gap <= 2,
        "ratatui buffer gap should also be <= 2 after tall active -> short commit: \
         gap={buffer_gap}\n{:#?}",
        result.buffer_screen
    );
    let active_input_prompt_count = result
        .terminal_screen
        .iter()
        .filter(|line| line.starts_with("❯"))
        .count();
    assert_eq!(
        active_input_prompt_count, 1,
        "vt100 terminal screen should contain exactly one input chrome row\n{}",
        result.terminal_contents
    );
    assert!(
        input_chrome_is_at_bottom(&result.terminal_screen),
        "vt100 input chrome should be at the bottom of the live viewport\n{}",
        result.terminal_contents
    );
    assert!(
        result.viewport_area.y <= old_y + terminal.visible_history_rows() + 2,
        "vt100 transition should not create an empty hole larger than inserted history rows: \
         old_y={}, new_y={}, visible_history_rows={}",
        old_y,
        result.viewport_area.y,
        terminal.visible_history_rows(),
    );
}

#[test]
fn tool_streaming_then_completed_card_has_no_chrome_interleave() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "run a command".to_string(),
    ));
    state.request_in_flight = true;
    state.pending_assistant = "Running bash...".to_string();
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw streaming frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    let mut tool_msg = TranscriptMessage::new(MessageRole::Assistant, String::new());
    tool_msg.blocks.push(TranscriptBlock::ToolUse {
        id: "tool-1".to_string(),
        name: "Bash".to_string(),
        input: serde_json::json!({"command": "echo hello"}).to_string(),
    });
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: tool_msg,
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    let mut result_msg = TranscriptMessage::new(MessageRole::User, String::new());
    result_msg.blocks.push(TranscriptBlock::ToolResult {
        tool_use_id: "tool-1".to_string(),
        content: "hello".to_string().into(),
        is_error: false,
        metadata: None,
    });
    state.messages.push(result_msg);
    state.pending_history_flush = true;
    state.pending_assistant = "The command succeeded.".to_string();
    state.request_in_flight = true;
    terminal.backend_mut().output.clear();

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let scrollback = screen.scrollback_lines();
    let screen_gap = max_blank_gap(&result.full_terminal_screen);
    assert!(
        screen_gap <= 2,
        "tool streaming -> committed: screen gap should be <= 2: gap={screen_gap}\n{:#?}",
        result.full_terminal_screen
    );
    assert_eq!(
        input_chrome_count(&result.buffer_screen),
        1,
        "input chrome should appear exactly once\n{:#?}",
        result.buffer_screen
    );
    for (i, line) in scrollback.iter().enumerate() {
        assert!(
            !is_chrome_line(line),
            "scrollback row {i} contains chrome: {line:?}"
        );
    }
    assert!(
        no_chrome_between_committed_cells(&scrollback),
        "scrollback must not have chrome between committed cells\n{:#?}",
        scrollback
    );
}

#[test]
fn history_insertion_updates_scrollback_ledger() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));

    assert_eq!(terminal.visible_history_rows(), 0);

    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "world".to_string(),
    ));
    state.pending_history_flush = true;

    let size = terminal.size().expect("size");
    state.prepare_pending_history_emission(size.width as usize, size.height);
    flush_pending_history_to_scrollback(
        &mut terminal,
        &mut state,
        size.width as usize,
        size.height,
    )
    .expect("flush");

    assert!(
        terminal.visible_history_rows() > 0,
        "after inserting history, visible_history_rows should be > 0: {}",
        terminal.visible_history_rows()
    );
}

#[test]
fn scrollback_reflow_resets_ledger() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));

    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello".to_string(),
    ));
    state.pending_history_flush = true;

    let size = terminal.size().expect("size");
    state.prepare_pending_history_emission(size.width as usize, size.height);
    flush_pending_history_to_scrollback(
        &mut terminal,
        &mut state,
        size.width as usize,
        size.height,
    )
    .expect("flush");
    assert!(terminal.visible_history_rows() > 0);

    state.transcript_ui.emission.needs_scrollback_clear = true;
    flush_pending_history_to_scrollback(
        &mut terminal,
        &mut state,
        size.width as usize,
        size.height,
    )
    .expect("reflow flush");
    assert_eq!(
        terminal.visible_history_rows(),
        0,
        "after scrollback reflow, ledger should be reset"
    );
}

#[test]
fn viewport_set_clamps_visible_history() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 10, 80, 14));
    terminal.note_history_rows_inserted(8);
    assert_eq!(terminal.visible_history_rows(), 8);

    terminal.set_viewport_area(Rect::new(0, 3, 80, 21));
    assert!(
        terminal.visible_history_rows() <= 3,
        "visible_history_rows should be clamped to viewport top: {}",
        terminal.visible_history_rows()
    );
}

#[test]
fn scrollback_contains_no_input_chrome_after_history_flush() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello world".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "goodbye".to_string(),
    ));
    state.pending_history_flush = true;

    let _ = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let scrollback = screen.scrollback_lines();
    for (i, line) in scrollback.iter().enumerate() {
        assert!(
            !line.starts_with("› ") && !line.starts_with("> ") && !line.starts_with("❯"),
            "scrollback row {i} contains input chrome: {line:?}"
        );
        let t = line.trim();
        let is_divider = !t.is_empty() && t.chars().all(|c| c == '─');
        assert!(!is_divider, "scrollback row {i} contains divider: {line:?}");
        assert!(
            !line.contains("-- NORMAL --") && !line.contains("-- INSERT --"),
            "scrollback row {i} contains footer: {line:?}"
        );
    }
}

#[test]
fn scrollback_max_blank_gap_after_multiple_commits() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);

    for i in 0..5 {
        state.messages.push(TranscriptMessage::new(
            MessageRole::User,
            format!("question {i}"),
        ));
        state.messages.push(TranscriptMessage::new(
            MessageRole::Assistant,
            format!("answer {i}"),
        ));
    }
    state.pending_history_flush = true;

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let full = screen.full_contents();
    let gap = max_blank_gap(&full);
    assert!(
        gap <= 2,
        "scrollback + screen should have max blank gap <= 2: gap={gap}\nfull contents:\n{:#?}",
        full
    );
    let viewport_gap = max_blank_gap(&result.buffer_screen);
    assert!(
        viewport_gap <= 2,
        "viewport should also have max blank gap <= 2: gap={viewport_gap}"
    );
}

#[test]
fn small_pane_scrollback_reachable() {
    let backend = RenderFixtureBackend::new(80, 10);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 2, 80, 8));
    let mut screen = TerminalScreenModel::new(80, 10);
    let mut state = normal_state("hello", 5);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "first prompt".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "first answer with a bit of content to show".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "second prompt".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "second answer".to_string(),
    ));
    state.pending_history_flush = true;

    let _ = run_prompt_transition(&mut state, &mut terminal, &mut screen);
    let full = screen.full_contents();
    let has_first_prompt = full.iter().any(|line| line.contains("first prompt"));
    assert!(
        has_first_prompt,
        "in a small pane, scrollback should contain the first prompt\nfull:\n{:#?}",
        full
    );
}

#[test]
fn standard_insert_preserves_viewport_position() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 10, 80, 14));

    let history_lines: Vec<StyledLine> = vec![
        StyledLine::from("history line 1"),
        StyledLine::from("history line 2"),
        StyledLine::from("history line 3"),
    ];
    let before_height = terminal.viewport_area.height;
    insert_history_lines(&mut terminal, &history_lines, 80).expect("standard insert");

    assert_eq!(
        terminal.viewport_area.height, before_height,
        "Standard insert should preserve viewport height"
    );
    assert!(
        terminal.visible_history_rows() > 0,
        "Standard insert should update the scrollback ledger"
    );
}

#[test]
fn vt100_standard_insert_keeps_prompt_below_history_rows() {
    let backend = VT100Backend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 80, 6));
    let mut state = normal_state("typed input", 11);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial viewport");

    let history_lines: Vec<StyledLine> = vec![
        StyledLine::from("committed history line 1"),
        StyledLine::from("committed history line 2"),
    ];
    insert_history_lines(&mut terminal, &history_lines, 80).expect("standard insert");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw after insert");

    let screen = terminal.backend_mut().screen_lines();
    let first_history = screen
        .iter()
        .position(|line| line.contains("committed history line 1"))
        .expect("history line 1 should be visible on vt100 screen");
    let second_history = screen
        .iter()
        .position(|line| line.contains("committed history line 2"))
        .expect("history line 2 should be visible on vt100 screen");
    let prompt = screen
        .iter()
        .position(|line| line.starts_with("❯"))
        .expect("active prompt should be visible on vt100 screen");

    assert!(
        first_history < second_history && second_history < prompt,
        "vt100 Standard insert should keep committed rows above the live prompt\n{screen:#?}"
    );
    assert_eq!(
        screen.iter().filter(|line| line.starts_with("❯")).count(),
        1,
        "vt100 Standard insert should leave one active prompt\n{screen:#?}"
    );
    assert!(
        max_blank_gap(&screen) <= 2,
        "vt100 Standard insert should not create a large visible gap\n{screen:#?}"
    );
    assert!(terminal.visible_history_rows() > 0);
}

#[test]
fn vt100_terminal_wrap_policy_counts_soft_wrapped_rows() {
    let backend = VT100Backend::new(20, 10);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 20, 5));
    let mut state = normal_state("typed input", 11);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial viewport");

    let long_source_line = "x".repeat(38);
    let history_lines = vec![StyledLine::from(long_source_line.clone())];
    insert_history_lines_with_wrap_policy(
        &mut terminal,
        &history_lines,
        20,
        HistoryLineWrapPolicy::Terminal,
        false,
    )
    .expect("terminal-wrap insert");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw after terminal-wrap insert");

    let screen = terminal.backend_mut().screen_lines();
    assert_eq!(
        terminal.visible_history_rows(),
        2,
        "Terminal wrap policy should record physical soft-wrapped rows\n{screen:#?}"
    );
    assert!(
        screen.iter().any(|line| line == &"x".repeat(20)),
        "first soft-wrapped physical row should be visible\n{screen:#?}"
    );
    assert!(
        screen.iter().any(|line| line.contains(&"x".repeat(18))),
        "second soft-wrapped physical row should be visible\n{screen:#?}"
    );
}

#[test]
fn vt100_soft_wrap_history_insert_preserves_following_committed_row() {
    let backend = VT100Backend::new(12, 8);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 12, 4));
    let mut state = normal_state("typed input", 11);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial viewport");

    let history_lines = vec![
        StyledLine::from("x".repeat(13)),
        StyledLine::from("COMMITTED"),
    ];
    insert_history_lines_with_wrap_policy(
        &mut terminal,
        &history_lines,
        12,
        HistoryLineWrapPolicy::Terminal,
        false,
    )
    .expect("terminal-wrap insert");

    let screen = terminal.backend_mut().screen_lines();
    assert!(
        screen.iter().any(|line| line.contains("COMMITTED")),
        "soft-wrap cleanup must not erase the following committed row\n{screen:#?}"
    );
}

#[test]
fn vt100_terminal_wrap_handles_exact_wide_combining_and_styled_boundaries() {
    let cases = vec![
        (StyledLine::from("x".repeat(12)), 1_u16, "exact width"),
        (StyledLine::from("x".repeat(13)), 2_u16, "width plus one"),
        (StyledLine::from("x".repeat(31)), 3_u16, "multiple wraps"),
        (StyledLine::from("界".repeat(7)), 2_u16, "wide glyphs"),
        (
            StyledLine::from("e\u{301}".repeat(12)),
            1_u16,
            "combining marks",
        ),
        (
            StyledLine::from(vec![
                Span::styled("styled-", Style::default().fg(Color::Red)),
                Span::styled("boundary", Style::default().add_modifier(Modifier::BOLD)),
            ]),
            2_u16,
            "ANSI style transition",
        ),
    ];

    for (line, expected_rows, label) in cases {
        let backend = VT100Backend::new(12, 8);
        let mut terminal = Terminal::with_options(backend).expect("create terminal");
        terminal.set_viewport_area(Rect::new(0, 4, 12, 4));
        insert_history_lines_with_wrap_policy(
            &mut terminal,
            &[line],
            12,
            HistoryLineWrapPolicy::Terminal,
            false,
        )
        .unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(terminal.visible_history_rows(), expected_rows, "{label}");
    }
}

#[test]
fn vt100_clear_after_position_clears_live_rows_and_next_draw_restores_them() {
    let backend = VT100Backend::new(40, 10);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 40, 6));
    let mut state = normal_state("typed input", 11);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial viewport");
    terminal.note_history_rows_inserted(3);

    let before = terminal.backend_mut().screen_lines();
    assert!(
        before.iter().any(|line| line.starts_with("❯")),
        "initial vt100 screen should contain the live prompt\n{before:#?}"
    );

    terminal
        .clear_after_position(Position::new(0, 4))
        .expect("clear after viewport top");
    let cleared = terminal.backend_mut().screen_lines();
    assert!(
        !cleared.iter().any(|line| line.starts_with("❯")),
        "clear_after_position should clear live viewport rows on the real terminal\n{cleared:#?}"
    );
    assert_eq!(
        terminal.visible_history_rows(),
        3,
        "live viewport clear must not reset visible history rows"
    );

    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw after clear");
    let redrawn = terminal.backend_mut().screen_lines();
    assert!(
        redrawn.iter().any(|line| line.starts_with("❯")),
        "reset diff buffer should force the next draw to restore live rows\n{redrawn:#?}"
    );
}

#[test]
fn vt100_resize_while_streaming_thinking_clears_stale_live_rows() {
    let backend = VT100Backend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 80, 18));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "summarize the resize behavior".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "prior committed history".to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "active resize thought should remain live only".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial frame");
    assert_eq!(
        state.transcript_ui.emission.emission_width,
        Some(80),
        "initial history flush should establish the emitted width"
    );

    terminal.backend_mut().resize(60, 24);
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare 60-col resize");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw 60-col resize");

    terminal.backend_mut().resize(40, 24);
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare 40-col resize");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw 40-col resize");

    let screen = terminal.backend_mut().screen_lines();
    let thinking_row_count = screen
        .iter()
        .filter(|line| line.contains("thinking)"))
        .count();
    let thinking_preview_count = screen
        .iter()
        .filter(|line| line.contains("active resize thought"))
        .count();
    let request_status_count = screen
        .iter()
        .filter(|line| line.contains("Thinking..."))
        .count();

    assert_eq!(
        thinking_row_count, 1,
        "repeated active resizes should leave exactly one live thinking summary\n{screen:#?}"
    );
    assert_eq!(
        thinking_preview_count, 1,
        "repeated active resizes should leave exactly one live thinking preview\n{screen:#?}"
    );
    assert!(
        request_status_count <= 1,
        "repeated active resizes should not stack request status rows\n{screen:#?}"
    );
    let live_input_count = screen.iter().filter(|line| line.starts_with("❯")).count();
    assert_eq!(
        live_input_count, 1,
        "repeated active resizes should leave exactly one live input prompt\n{screen:#?}"
    );
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "committed-history reflow should remain deferred during active streaming"
    );
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "active streaming resize should not request the full committed-history purge"
    );
}

#[test]
fn vt100_pure_height_resizes_do_not_leave_large_blank_gap() {
    let backend = VT100Backend::new(80, 30);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 8, 80, 22));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "height resize prompt marker".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "height resize committed marker".to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "active height resize thought should remain live only".to_string(),
        is_streaming: true,
        completed_at: None,
    });
    state.pending_assistant = (0..20)
        .map(|index| format!("height resize streaming line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial frame");

    for height in [24, 18, 28, 14, 30] {
        terminal.backend_mut().resize(80, height);
        prepare_draw_transaction(&mut terminal, &mut state, false)
            .expect("prepare height resize frame");
        terminal
            .draw(|frame| state.draw(frame))
            .expect("draw height resize frame");

        let screen = terminal.backend_mut().screen_lines();
        // Per-frame the viewport is deliberately NOT allowed to climb into the
        // rows above it while a resize is still unsettled — those are
        // terminal/tmux-owned (High #1). Once earlier shrinks have scrolled real
        // history off into native scrollback, a later grow can therefore expose a
        // transient blank gap above the viewport rather than overdrawing (and
        // erasing) it. The plan documents this transient gap as accepted; the
        // settle rebuild below reconciles it. So only the input-chrome invariant
        // is enforced per frame here; the gap is asserted after settle.
        assert_eq!(
            screen.iter().filter(|line| line.starts_with("❯")).count(),
            1,
            "pure height resize should leave one live input prompt at height {height}\n{screen:#?}"
        );
    }
    // Under the codex-style model every resize (including a pure height change)
    // DEFERS a full source-of-truth rebuild instead of purging/repainting on
    // each frame: reflow is marked pending and the viewport repositions, but no
    // scrollback purge happens until the resize settles.
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "pure height resize should defer a committed-history rebuild"
    );
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "pure height resize should not purge scrollback before the settle rebuild"
    );

    // Simulate the main loop's resize-settle deadline firing: the full source
    // rebuild purges native scrollback and re-emits the whole committed
    // transcript at the current size, restoring the banner and history exactly
    // once with no leftover gap.
    state.rebuild_committed_history_from_source();
    assert!(
        state.transcript_ui.emission.needs_scrollback_clear,
        "settle rebuild should purge native scrollback"
    );
    assert!(
        !state.transcript_ui.emission.reflow_pending,
        "settle rebuild should clear the deferred reflow"
    );
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare settle frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw settle frame");
    let settled = terminal.backend_mut().screen_lines();
    let settled_text = settled.join("\n");
    assert_eq!(
        settled_text
            .matches("height resize committed marker")
            .count(),
        1,
        "settle rebuild should restore the committed history exactly once, not duplicate it\n{settled:#?}"
    );
    let settled_gap = max_blank_gap(&settled);
    assert!(
        settled_gap <= 2,
        "settle rebuild should leave no large blank gap: gap={settled_gap}\n{settled:#?}"
    );
}

#[test]
fn active_height_growth_repins_live_viewport_below_history() {
    let backend = VT100Backend::new(88, 22);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 15, 88, 7));
    terminal.note_history_rows_inserted(3);
    let mut state = normal_state("", 0);
    state.transcript_ui.emission.emission_width = Some(88);
    state.transcript_ui.emission.emitted_cell_count = 1;
    state.history_flushed_message_count = 1;
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "active grow live thinking marker".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    terminal.backend_mut().resize(88, 32);
    let txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare grow frame");

    assert!(txn.viewport_mutated);
    assert_eq!(
        terminal.viewport_area.bottom(),
        32,
        "active viewport should return to the screen bottom after height growth"
    );

    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw grow frame");
    let screen = terminal.backend_mut().screen_lines();
    let live_top = terminal.viewport_area.y as usize;
    assert!(
        !screen
            .iter()
            .take(live_top)
            .any(|line| line.contains("Thinking...") || line.contains("active grow live thinking")),
        "active streaming rows should not be left above the live viewport\n{screen:#?}"
    );
    assert!(
        screen
            .iter()
            .skip(live_top)
            .any(|line| line.contains("Thinking...") || line.contains("active grow live thinking")),
        "active streaming rows should render inside the repinned live viewport\n{screen:#?}"
    );
}

#[test]
fn idle_after_streaming_repins_history_footer_to_bottom() {
    let backend = RenderFixtureBackend::new(84, 25);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 3, 84, 6));
    let prompt = "找出目录里行数最多的前十 .rs 文件";
    let mut state = normal_state(prompt, prompt.len());
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "completed streaming prompt marker".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "completed streaming answer marker".to_string(),
    ));
    state.history_flushed_message_count = state.messages.len();
    state.transcript_ui.emission.emitted_cell_count = state.messages.len();
    state.prompt_history = vec![
        "/allow-all on".to_string(),
        prompt.to_string(),
        "dddd".to_string(),
    ];
    state.prompt_history_index = Some(1);
    // Distinctive footer marker: the (left-aligned) status bar renders the cwd.
    let footer_marker = "FOOTER-STATUS-CWD-MARKER";
    state.cwd_display = footer_marker.to_string();

    let txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare idle frame");

    assert!(txn.viewport_mutated);
    assert!(!txn.history_flushed);
    assert_eq!(
        terminal.viewport_area,
        Rect::new(0, 20, 84, 5),
        "idle viewport should return to the bottom after streaming completes"
    );

    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw idle frame");
    let screen = terminal.screen_lines();
    let footer = screen.last().expect("footer line");
    assert!(
        footer.contains(footer_marker),
        "status footer should render at the bottom of the live viewport\n{screen:#?}"
    );
    assert!(
        footer.starts_with("  model \u{b7} "),
        "status bar should be indented 2 cols to align with the prompt input, got: {footer:?}"
    );
    assert!(
        !screen
            .iter()
            .take(screen.len().saturating_sub(1))
            .any(|line| line.contains(footer_marker)),
        "status footer should not be left in the transcript body\n{screen:#?}"
    );
}

#[test]
fn resize_while_streaming_clears_stale_history_rows_before_redraw() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 80, 18));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    let committed_marker = "resize streaming committed marker";
    let stale_streaming_marker = "old width streaming marker must be cleared";

    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "resize streaming prompt marker".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        committed_marker.to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "active resize thought should remain live only".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();
    assert!(
        terminal.visible_history_rows() > 0,
        "initial committed history should establish visible history rows"
    );

    let old_live_row = terminal.viewport_area.y;
    screen
        .process_bytes(format!("\x1b[{};1H{stale_streaming_marker}", old_live_row + 1).as_bytes());
    assert!(
        screen
            .full_contents()
            .join("\n")
            .contains(stale_streaming_marker),
        "test setup should place stale old-width streaming rows above the live viewport"
    );

    terminal.backend_mut().resize(60, 24);
    let txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare resize frame");
    assert!(
        txn.viewport_mutated,
        "streaming width repair should force a prompt redraw"
    );
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw resize frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    let full = screen.full_contents().join("\n");
    assert_eq!(
        full.matches(stale_streaming_marker).count(),
        0,
        "old-width streaming rows should be cleared before the resized live redraw\n{full}"
    );
    assert_eq!(
        full.matches(committed_marker).count(),
        1,
        "committed history marker should be restored exactly once after resize repair\n{full}"
    );
    let scrollback = screen.scrollback_lines().join("\n");
    assert_eq!(
        scrollback.matches(stale_streaming_marker).count(),
        0,
        "old-width streaming rows should not survive in native scrollback\n{scrollback}"
    );
    assert!(
        scrollback.matches(committed_marker).count() <= 1,
        "committed marker should not be duplicated in native scrollback after resize\n{scrollback}"
    );
    assert!(
        terminal.visible_history_rows() > 0,
        "streaming width repair should restore committed history rows above the viewport"
    );
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "committed-history reflow should remain deferred until streaming finishes"
    );
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "streaming width repair should not request the full committed-history purge"
    );
}

#[test]
fn height_shrink_while_streaming_defers_reflow_then_rebuilds_on_settle() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 80, 18));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);

    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "height shrink streaming prompt marker".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "height shrink committed marker".to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "active height shrink thought should remain live only".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    let emitted_before = state.transcript_ui.emission.emitted_cell_count;
    let flushed_before = state.history_flushed_message_count;
    let old_history_bottom = terminal.viewport_area.top();
    terminal.backend_mut().resize(80, 12);
    let txn = prepare_draw_transaction(&mut terminal, &mut state, false)
        .expect("prepare height shrink frame");
    let resize_frame_output = terminal.backend_mut().output_string();

    // The resize frame itself DEFERS the rebuild (codex-style): the viewport
    // repositions and reflow is marked pending, but nothing is purged, flushed
    // or re-emitted mid-drag. The full source rebuild waits for the settle.
    assert!(
        !resize_frame_output.contains("\x1b[3J"),
        "deferred height repair should not purge native scrollback on the resize frame"
    );
    let forbidden_history_scroll = format!("\x1b[1;{old_history_bottom}r");
    assert!(
        !resize_frame_output.contains(&forbidden_history_scroll),
        "height resize must leave rows above the viewport terminal/tmux-owned: \
         {resize_frame_output:?}"
    );
    assert!(
        txn.viewport_mutated,
        "streaming height shrink repair should force a prompt redraw"
    );
    assert!(
        txn.resize_observed,
        "streaming height shrink should be reported as a resize so the main loop arms the settle"
    );
    assert!(
        !txn.history_flushed,
        "the deferred resize frame should not flush or rebuild committed history"
    );
    assert_eq!(
        state.transcript_ui.emission.emitted_cell_count, emitted_before,
        "the deferred resize frame should preserve emitted history accounting"
    );
    assert_eq!(
        state.history_flushed_message_count, flushed_before,
        "the deferred resize frame should preserve flushed message accounting"
    );
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "every resize now defers a committed-history rebuild"
    );
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "the deferred resize frame should not request the full committed-history purge"
    );

    // Simulate the main loop's resize-settle deadline: the full source rebuild
    // purges native scrollback and re-emits the committed transcript once.
    state.rebuild_committed_history_from_source();
    assert!(
        state.transcript_ui.emission.needs_scrollback_clear,
        "settle rebuild should purge native scrollback"
    );
    assert!(
        !state.transcript_ui.emission.reflow_pending,
        "settle rebuild should clear the deferred reflow"
    );
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare settle frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw settle frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    let full = screen.full_contents().join("\n");
    assert_eq!(
        full.matches("height shrink committed marker").count(),
        1,
        "settle rebuild should restore the committed history exactly once\n{full}"
    );
    for line in screen.scrollback_lines() {
        assert!(
            !is_scrollback_chrome(&line),
            "settle rebuild should not leave live chrome in native scrollback: {line:?}"
        );
    }
}

#[test]
fn idle_height_resize_does_not_purge_or_reemit_history() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 80, 18));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);

    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "idle height resize prompt marker".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "idle height resize committed marker".to_string(),
    ));
    state.pending_history_flush = true;

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    let emitted_before = state.transcript_ui.emission.emitted_cell_count;
    let flushed_before = state.history_flushed_message_count;

    terminal.backend_mut().resize(80, 12);
    let txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare resize frame");
    assert!(
        !terminal
            .backend_mut()
            .output
            .windows(4)
            .any(|bytes| bytes == b"\x1b[3J"),
        "idle height repair should not purge native scrollback"
    );
    assert!(txn.viewport_mutated);
    assert!(!txn.history_flushed);
    assert_eq!(
        state.transcript_ui.emission.emitted_cell_count,
        emitted_before
    );
    assert_eq!(state.history_flushed_message_count, flushed_before);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw resized frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    assert_eq!(
        terminal.viewport_area.bottom(),
        12,
        "resized idle viewport should stay pinned to the screen bottom"
    );
}

#[test]
fn height_resize_rebuilds_committed_history_on_settle() {
    let backend = RenderFixtureBackend::new(80, 34);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    let mut screen = TerminalScreenModel::new(80, 34);
    let mut state = normal_state("", 0);
    let marker = "committed history row must survive height resize";

    let old_target_height = state
        .desired_viewport_height(80, 34)
        .min(34_u16.saturating_sub(1).max(1))
        .max(1);
    let old_y = 34_u16.saturating_sub(old_target_height);
    terminal.set_viewport_area(Rect::new(0, old_y, 80, old_target_height));
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "height resize history prompt marker".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        marker.to_string(),
    ));
    state.pending_history_flush = true;

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();
    let emitted_before = state.transcript_ui.emission.emitted_cell_count;
    let flushed_before = state.history_flushed_message_count;
    assert!(
        terminal.visible_history_rows() > 0,
        "initial committed history should establish visible history rows"
    );

    // The legacy in-place repaint of rows moved out of the viewport is retired.
    // A height resize now DEFERS a full source rebuild: the resize frame does
    // not purge, flush or re-emit committed history — it just repositions.
    terminal.backend_mut().resize(80, 28);
    let shrink_txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare shrink frame");
    assert!(shrink_txn.viewport_mutated);
    assert!(shrink_txn.resize_observed);
    assert!(!shrink_txn.history_flushed);
    assert!(
        !terminal
            .backend_mut()
            .output
            .windows(4)
            .any(|bytes| bytes == b"\x1b[3J"),
        "the deferred resize frame should not purge native scrollback"
    );
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "the resize should defer a committed-history rebuild"
    );
    assert_eq!(
        state.transcript_ui.emission.emitted_cell_count,
        emitted_before
    );
    assert_eq!(state.history_flushed_message_count, flushed_before);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw shrink frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();
    let after_shrink = screen.full_contents().join("\n");
    assert!(
        after_shrink.matches(marker).count() <= 1,
        "the deferred resize frame should never duplicate committed history\n{after_shrink}"
    );

    // Simulate the main loop's resize-settle deadline firing: the full source
    // rebuild re-emits every committed history cell from source at the new
    // size, restoring the marker intact and exactly once (no duplication, no
    // leftover chrome).
    state.rebuild_committed_history_from_source();
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare settle frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw settle frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    let settled = screen.full_contents().join("\n");
    assert_eq!(
        settled.matches(marker).count(),
        1,
        "settle rebuild should restore committed history exactly once\n{settled}"
    );
    for line in screen.scrollback_lines() {
        assert!(
            !is_scrollback_chrome(&line),
            "settle rebuild should not leave live chrome in native scrollback: {line:?}"
        );
    }
}

#[test]
fn reflow_rebuild_clears_visible_screen_and_scrollback() {
    // The settle rebuild must clear the VISIBLE screen (ESC[2J) as well as purge
    // native scrollback (ESC[3J) before re-emitting the transcript from source.
    // Without the ESC[2J the re-emit lands on top of stale rows → blank bands and
    // partial-render remnants. Assert both are emitted.
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 80, 18));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "reflow clear marker".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "committed answer to re-emit".to_string(),
    ));
    state.pending_history_flush = true;

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial");
    terminal.backend_mut().output.clear();

    // Simulate the settle rebuild and flush it (drives reset_inline_scrollback_for_reflow).
    state.rebuild_committed_history_from_source();
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare settle rebuild");

    let output = terminal.backend_mut().output_string();
    assert!(
        output.contains("\x1b[3J"),
        "reflow rebuild should purge native scrollback (ESC[3J): {output:?}"
    );
    assert!(
        output.contains("\x1b[2J"),
        "reflow rebuild MUST also clear the visible screen (ESC[2J) so the re-emit \
         doesn't land on stale rows: {output:?}"
    );
}

#[test]
fn idle_bottom_pin_growth_rebuilds_history_on_settle() {
    let backend = VT100Backend::new(88, 13);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 88, 12));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "find the biggest Rust files".to_string(),
    ));
    state.pending_history_flush = true;

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare prompt history");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw prompt history");

    let table_rows = (1..=18)
        .map(|row| format!("│ resize row {row:02} │ src/lib_{row:02}.rs │"))
        .collect::<Vec<_>>();
    let answer = std::iter::once("short flush table start".to_string())
        .chain(table_rows.iter().cloned())
        .chain(std::iter::once("short flush table end".to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    state.request_in_flight = true;
    state.pending_assistant = answer.clone();

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare streaming");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw streaming");

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare final flush");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw final flush");

    // The bottom-pin growth resize DEFERS the rebuild (codex-style): the
    // viewport repositions but committed history is not repainted on the resize
    // frame itself.
    terminal.backend_mut().resize(88, 27);
    let grow_txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare grow");
    assert!(grow_txn.resize_observed);
    assert!(state.transcript_ui.emission.reflow_pending);
    terminal.draw(|frame| state.draw(frame)).expect("draw grow");

    // Simulate the resize-settle rebuild: the committed table is re-emitted from
    // source at the grown size, restoring the rows in order and exactly once.
    state.rebuild_committed_history_from_source();
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare settle");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw settle");

    let screen = terminal.backend_mut().screen_lines();
    let text = screen.join("\n");
    let first = text
        .find("resize row 08")
        .expect("earlier table row visible after settle");
    let last = text
        .find("resize row 18")
        .expect("last table row visible after settle");
    assert!(
        first < last,
        "table rows should remain ordered after the settle rebuild\n{text}"
    );
    for row in [8, 12, 18] {
        let marker = format!("resize row {row:02}");
        assert_eq!(
            text.matches(&marker).count(),
            1,
            "{marker} should appear exactly once after the settle rebuild\n{text}"
        );
    }
}

#[test]
fn idle_height_shrink_keeps_committed_tail_visible_on_small_screen() {
    let backend = VT100Backend::new(88, 27);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 22, 88, 5));
    let mut state = normal_state("", 0);
    let tail_marker = "small resize final tail marker";
    let answer = (1..=18)
        .map(|row| format!("small resize body row {row:02}"))
        .chain(std::iter::once(tail_marker.to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "produce a small resize report".to_string(),
    ));
    state
        .messages
        .push(TranscriptMessage::new(MessageRole::Assistant, answer));
    state.pending_history_flush = true;

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial flush");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial flush");

    // The shrink resize defers the rebuild; the settle re-emits the committed
    // transcript from source at the small size, so the committed tail lands in
    // the (native) scrollback/screen exactly once.
    terminal.backend_mut().resize(88, 11);
    let shrink_txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare small shrink");
    assert!(shrink_txn.resize_observed);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw small shrink");

    state.rebuild_committed_history_from_source();
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare settle");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw settle");

    let screen = terminal.backend_mut().screen_lines();
    let text = screen.join("\n");
    assert!(
        text.contains(tail_marker),
        "small idle height shrink should keep committed tail visible after the settle rebuild\n{text}"
    );
    assert_eq!(
        text.matches(tail_marker).count(),
        1,
        "small idle height shrink should not duplicate committed tail\n{text}"
    );
}

#[test]
fn repeated_idle_height_shrink_keeps_committed_tail_unique_on_screen() {
    let backend = VT100Backend::new(80, 34);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    let mut state = normal_state("", 0);
    let old_target_height = state
        .desired_viewport_height(80, 34)
        .min(34_u16.saturating_sub(1).max(1))
        .max(1);
    terminal.set_viewport_area(Rect::new(
        0,
        34_u16.saturating_sub(old_target_height),
        80,
        old_target_height,
    ));

    let tail_lines = [
        "resize committed tail alpha",
        "resize committed tail beta",
        "resize committed tail gamma",
        "resize committed tail delta",
    ];
    let answer = tail_lines.join("\n");
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "height resize history prompt marker".to_string(),
    ));
    state
        .messages
        .push(TranscriptMessage::new(MessageRole::Assistant, answer));
    state.pending_history_flush = true;

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare initial frame");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw initial frame");
    assert!(
        terminal.visible_history_rows() > 0,
        "initial committed history should establish visible history rows"
    );

    // A drag through many heights defers the rebuild on every frame (no purge
    // per SIGWINCH). The main loop coalesces them into a single settle rebuild
    // once the size stops changing.
    for height in [32, 29, 25, 24, 25, 28, 34] {
        terminal.backend_mut().resize(80, height);
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare resize frame");
        terminal
            .draw(|frame| state.draw(frame))
            .expect("draw resize frame");
    }
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "a run of resizes should leave a single deferred rebuild pending"
    );

    // Simulate the coalesced settle rebuild after the drag stops.
    state.rebuild_committed_history_from_source();
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare settle");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw settle");

    let screen = terminal.backend_mut().screen_lines();
    let text = screen.join("\n");
    for line in tail_lines {
        assert_eq!(
            text.matches(line).count(),
            1,
            "the settle rebuild should leave each committed tail row exactly once on screen: {line}\n\
             {text}"
        );
    }
}

#[test]
fn ctrl_o_open_close_leaves_no_residue_in_viewport() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prompt text".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "answer text".to_string(),
    ));

    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw before pager");
    let pre_pager_viewport = terminal.viewport_area;

    let mut pager_mode = TranscriptPagerTerminalMode::default();
    sync_transcript_pager_terminal_mode(&mut terminal, true, &mut pager_mode).expect("open pager");
    assert!(pager_mode.is_active(), "pager should be active");

    sync_transcript_pager_terminal_mode(&mut terminal, false, &mut pager_mode)
        .expect("close pager");
    assert!(!pager_mode.is_active(), "pager should be inactive");

    terminal.invalidate_viewport();
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw after pager close");
    let post_pager_screen = terminal.screen_lines();

    assert!(
        terminal.viewport_area.width == pre_pager_viewport.width,
        "viewport width should be restored after pager close"
    );
    assert!(
        screen_has_input_chrome(&post_pager_screen),
        "input chrome should be visible after pager close\n{:#?}",
        post_pager_screen
    );
    let post_gap = max_blank_gap(&post_pager_screen);
    assert!(
        post_gap <= 2,
        "no large gap after pager close: gap={post_gap}\n{:#?}",
        post_pager_screen
    );
}

#[test]
fn viewport_shrink_keeps_history_and_live_view_contiguous() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let old_y = terminal.viewport_area.y;

    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello".to_string(),
    ));
    state.request_in_flight = true;
    state.pending_assistant = (0..16)
        .map(|i| format!("streaming line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut screen = TerminalScreenModel::new(80, 24);
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw tall frame");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "done".to_string()),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let result = run_prompt_transition(&mut state, &mut terminal, &mut screen);

    let full_gap = max_blank_gap(&result.full_terminal_screen);
    assert!(
        full_gap <= 2,
        "after shrink, scrollback + screen should not contain a large blank gap: \
         gap={full_gap}\n{:#?}",
        result.full_terminal_screen
    );
    for (i, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_chrome_line(line),
            "scrollback row {i} contains chrome after shrink: {line:?}"
        );
    }
    assert_eq!(
        input_chrome_count(&result.buffer_screen),
        1,
        "final visible viewport should contain exactly one input chrome row\n{:#?}",
        result.buffer_screen
    );
    assert!(
        screen_has_input_chrome(&result.buffer_screen),
        "final visible viewport should show input chrome\n{:#?}",
        result.buffer_screen
    );
    assert!(
        result.viewport_area.y <= old_y + terminal.visible_history_rows() + 2,
        "viewport shrink should not create an empty hole larger than inserted history rows: \
         old_y={}, new_y={}, visible_history_rows={}",
        old_y,
        result.viewport_area.y,
        terminal.visible_history_rows(),
    );
}

#[test]
fn multi_frame_tall_stream_to_short_commit_to_next_cell() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "list the relevant files".to_string(),
    ));
    let old_y = terminal.viewport_area.y;
    state.request_in_flight = true;
    state.pending_assistant = (0..15)
        .map(|i| format!("thinking about search result line {i}"))
        .collect::<Vec<_>>()
        .join("\n");

    // Frame 1: tall streaming viewport
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("frame 1 prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("frame 1 draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    // Commit: streaming ends, short answer committed
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(
            MessageRole::Assistant,
            "Search results:\n- tui/src/app.rs\n- tui/src/tui_runtime/terminal_session.rs"
                .to_string(),
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    // Frame 2: post-commit transition with history flush
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("frame 2 prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("frame 2 draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    // Frame 3: next tool/active cell starts
    state.request_in_flight = true;
    state.pending_assistant = "Reading tui/src/tui_runtime/terminal_session.rs...".to_string();
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("frame 3 prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("frame 3 draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    // Transition guarantees that hold immediately at frame 3: the new active
    // cell never duplicates the just-committed answer, the live viewport does
    // not reserve a hole larger than its own history ledger, and no live chrome
    // leaks into native scrollback.
    assert!(
        terminal.viewport_area.y <= old_y + terminal.visible_history_rows() + 2,
        "multi-frame shrink should not create an empty hole larger than inserted history rows: \
         old_y={}, new_y={}, visible_history_rows={}",
        old_y,
        terminal.viewport_area.y,
        terminal.visible_history_rows(),
    );
    for (i, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_chrome_line(line),
            "scrollback row {i} contains chrome: {line:?}"
        );
    }
    assert!(
        screen_has_input_chrome(&terminal.screen_lines()),
        "input chrome should be visible at frame 3"
    );
    assert_eq!(
        input_chrome_count(&terminal.screen_lines()),
        1,
        "input chrome should appear exactly once at frame 3\n{:#?}",
        terminal.screen_lines()
    );

    // Regression (High #4): the transient hole between the short committed answer
    // and the bottom-pinned active viewport must be reconciled by a
    // PRODUCTION-reachable path — no resize occurs here, so no resize-settle
    // rebuild ever fires. `prepare_draw_transaction` detects the residual blank
    // band and rebuilds from source on its own; the committed history must be
    // contiguous (max blank gap <= 2) with each committed line appearing exactly
    // once and no chrome in native scrollback, WITHOUT any manual rebuild.
    let full = screen.full_contents();
    let full_text = full.join("\n");
    let gap = max_blank_gap(&full);
    assert!(
        gap <= 2,
        "the tall-to-short collapse must be reconciled by production (max blank gap <= 2), \
         got {gap}\n{full:#?}"
    );
    for marker in [
        "list the relevant files",
        "tui/src/app.rs",
        "Search results:",
    ] {
        assert_eq!(
            full_text.matches(marker).count(),
            1,
            "committed line {marker:?} should appear exactly once after reconciliation\n{full_text}"
        );
    }
    assert!(
        screen_has_input_chrome(&terminal.screen_lines()),
        "input chrome should be visible in final frame"
    );
    assert_eq!(
        input_chrome_count(&terminal.screen_lines()),
        1,
        "input chrome should appear exactly once in final frame\n{:#?}",
        terminal.screen_lines()
    );
}

/// Regression test for the multi-tool streaming chrome pollution bug.
/// Simulates multiple Bash tool rounds (tool use committed + tool result committed)
/// with a realistic active viewport (input/status/divider visible), then checks
/// that scrollback never contains live viewport chrome.
#[test]
fn multi_tool_streaming_scrollback_never_contains_viewport_chrome() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 80, 20));
    let mut screen = TerminalScreenModel::new(80, 24);
    let mut state = normal_state("", 0);

    // Establish a realistic active viewport with committed content first
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "evaluate the project".to_string(),
    ));
    state.pending_history_flush = true;

    // Frame 0: flush user message, establish viewport with input/status chrome
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("frame 0 prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("frame 0 draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    assert!(
        screen_has_input_chrome(&terminal.screen_lines()),
        "initial viewport should show input chrome\n{:#?}",
        terminal.screen_lines()
    );

    // Start streaming
    state.request_in_flight = true;
    state.pending_assistant = "Thinking about the project...".to_string();

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("streaming frame prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("streaming frame draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());
    terminal.backend_mut().output.clear();

    // Simulate 5 tool rounds with streaming content between them
    for round in 0..5 {
        let mut tool_msg = TranscriptMessage::new(MessageRole::Assistant, String::new());
        tool_msg.blocks.push(TranscriptBlock::ToolUse {
            id: format!("tool-{round}"),
            name: "Bash".to_string(),
            input: format!("echo round {round}"),
        });
        state.messages.push(tool_msg);
        state.pending_history_flush = true;

        let mut result_msg = TranscriptMessage::new(MessageRole::User, String::new());
        result_msg.blocks.push(TranscriptBlock::ToolResult {
            tool_use_id: format!("tool-{round}"),
            content: format!("output line 1 from round {round}\noutput line 2\noutput line 3")
                .into(),
            is_error: false,
            metadata: None,
        });
        state.messages.push(result_msg);
        state.pending_history_flush = true;

        state.pending_assistant = (0..8)
            .map(|i| format!("streaming line {i} round {round}"))
            .collect::<Vec<_>>()
            .join("\n");

        prepare_draw_transaction(&mut terminal, &mut state, false)
            .unwrap_or_else(|e| panic!("round {round} prepare: {e}"));
        terminal
            .draw(|frame| state.draw(frame))
            .unwrap_or_else(|e| panic!("round {round} draw: {e}"));
        screen.process_bytes(terminal.backend_mut().output.as_slice());
        terminal.backend_mut().output.clear();

        for (i, line) in screen.scrollback_lines().iter().enumerate() {
            assert!(
                !is_scrollback_chrome(line),
                "after tool round {round}: scrollback row {i} contains viewport chrome: {line:?}"
            );
        }
    }

    // Complete the response
    state.pending_assistant.clear();
    state.request_in_flight = false;
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "Final analysis complete.".to_string(),
    ));
    state.pending_history_flush = true;

    prepare_draw_transaction(&mut terminal, &mut state, false).expect("final prepare");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("final draw");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    for (i, line) in screen.scrollback_lines().iter().enumerate() {
        assert!(
            !is_scrollback_chrome(line),
            "after completion: scrollback row {i} contains viewport chrome: {line:?}"
        );
    }

    let scrollback = screen.scrollback_lines();
    let final_msg_count = scrollback
        .iter()
        .filter(|line| line.contains("Final analysis complete"))
        .count();
    assert!(
        final_msg_count <= 1,
        "final message should appear at most once in scrollback, got {final_msg_count}"
    );
}

#[test]
fn pager_active_does_not_resize_viewport() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "hello".to_string(),
    ));
    state.pending_history_flush = true;

    // Open pager: sets viewport to full screen
    let mut pager_mode = TranscriptPagerTerminalMode::default();
    sync_transcript_pager_terminal_mode(&mut terminal, true, &mut pager_mode).expect("open pager");
    let pager_viewport = terminal.viewport_area;
    assert_eq!(
        pager_viewport.height, 24,
        "pager viewport should be full screen height"
    );

    // Run prepare_draw_transaction with pager_active=true
    let txn = prepare_draw_transaction(&mut terminal, &mut state, true)
        .expect("prepare with pager active");

    assert_eq!(
        terminal.viewport_area, pager_viewport,
        "viewport must not change while pager is active"
    );
    assert!(
        !txn.viewport_mutated,
        "pager-active prepare must not report an inline viewport mutation"
    );
    assert!(
        !txn.history_flushed,
        "pager-active prepare must defer history flush until pager closes"
    );

    // Close pager
    sync_transcript_pager_terminal_mode(&mut terminal, false, &mut pager_mode)
        .expect("close pager");

    // Now viewport should be restorable
    prepare_draw_transaction(&mut terminal, &mut state, false).expect("prepare after pager close");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw after pager close");
    assert!(
        screen_has_input_chrome(&terminal.screen_lines()),
        "input chrome should be visible after pager close"
    );
}

#[test]
fn vt100_pager_open_defers_history_until_close_and_restores_inline_viewport() {
    let backend = VT100Backend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let initial_viewport = terminal.viewport_area;
    let mut state = normal_state("", 0);
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::User,
        "pager vt100 initial prompt".to_string(),
    ));
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::Assistant,
        "pager vt100 initial reply".to_string(),
    ));

    let initial_txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("initial prepare");
    assert!(
        initial_txn.history_flushed,
        "initial committed history should flush before opening pager"
    );
    terminal
        .draw(|frame| state.draw(frame))
        .expect("initial draw");

    state.open_transcript_pager(80, 24);
    let mut pager_mode = TranscriptPagerTerminalMode::default();
    sync_transcript_pager_terminal_mode(&mut terminal, true, &mut pager_mode).expect("open pager");
    assert!(pager_mode.is_active(), "pager should own alternate screen");

    let pager_txn =
        prepare_draw_transaction(&mut terminal, &mut state, true).expect("pager prepare");
    assert!(
        !pager_txn.history_flushed,
        "opening pager should not flush inline history"
    );
    terminal
        .draw(|frame| state.draw(frame))
        .expect("pager draw");

    let deferred_text = "pager vt100 deferred committed cell";
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::Assistant,
        deferred_text.to_string(),
    ));
    let active_txn =
        prepare_draw_transaction(&mut terminal, &mut state, true).expect("pager active prepare");
    assert!(
        !active_txn.history_flushed,
        "committed history must remain deferred while pager is active"
    );
    assert!(
        state.should_flush_history(),
        "deferred history should remain queued for primary screen after pager close"
    );
    terminal
        .draw(|frame| state.draw(frame))
        .expect("pager redraw with deferred message");
    let pager_screen = terminal.backend_mut().screen_lines();
    assert!(
        pager_screen
            .iter()
            .any(|line| line.contains("pager vt100 initial reply")),
        "pager overlay should keep its open-time snapshot\n{pager_screen:#?}"
    );
    assert!(
        !pager_screen.iter().any(|line| line.contains(deferred_text)),
        "pager overlay should not sync newly committed cells while open\n{pager_screen:#?}"
    );

    state.toggle_expanded_tool_details();
    assert!(
        !matches!(state.overlay, Some(OverlayState::TranscriptPager(_))),
        "ctrl-o close should remove transcript pager overlay"
    );
    sync_transcript_pager_terminal_mode(&mut terminal, false, &mut pager_mode)
        .expect("close pager");
    assert!(
        !pager_mode.is_active(),
        "pager should leave alternate screen"
    );

    let close_txn =
        prepare_draw_transaction(&mut terminal, &mut state, false).expect("close prepare");
    assert!(
        close_txn.history_flushed,
        "deferred committed history should flush after pager closes"
    );
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw after pager close");

    let screen = terminal.backend_mut().screen_lines();
    let text = screen.join("\n");
    assert_eq!(
        text.matches(deferred_text).count(),
        1,
        "deferred committed cell should appear exactly once after pager close\n{text}"
    );
    assert!(
        !state.should_flush_history(),
        "history queue should be drained after pager close flush"
    );
    let live_prompt_count = screen.iter().filter(|line| line.starts_with("❯")).count();
    assert_eq!(
        live_prompt_count, 1,
        "primary screen should have one live input prompt after pager close\n{text}"
    );
    assert!(
        max_blank_gap(&screen) <= 2,
        "pager close should not leave a large blank gap in primary screen\n{text}"
    );
    assert!(
        !text.contains("jk scroll") && !text.contains("ctrl-b/f") && !text.contains("q close"),
        "pager help text should not leak back to primary screen after close\n{text}"
    );
    assert_eq!(
        terminal.viewport_area.width, initial_viewport.width,
        "inline viewport width should be restored after pager close"
    );
}
