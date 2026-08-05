use std::time::Instant;

use crate::tests::support::*;

#[test]
fn user_message_lines_fill_the_available_width() {
    let message = TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::Text {
            text: "hello".to_string(),
        }],
    );

    let lines = render_user_message_lines(&message, 12);
    assert_eq!(lines.len(), 1);
    let rendered = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(rendered.chars().map(display_width).sum::<usize>(), 12);
    assert!(rendered.starts_with("› hello"));
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(USER_BAR_BG))
    );
    let content_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref().contains("hello"))
        .expect("user message content span should be present");
    assert_eq!(content_span.style.fg, Some(Color::White));
}

#[test]
fn user_message_wraps_before_right_padding() {
    let message = TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::Text {
            text: "abcdefghijklmnopqrstuvwxyz".to_string(),
        }],
    );

    let lines = render_user_message_lines(&message, 16);
    let rendered = plain_text_lines(&lines);

    assert!(rendered.len() > 1);
    assert!(
        rendered
            .iter()
            .all(|line| display_width_str(line.trim_end()) <= 14)
    );
    assert!(
        lines
            .iter()
            .all(|line| styled_line_display_width(line) == 16)
    );
}

#[test]
fn streaming_assistant_text_stays_as_single_active_cell() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (1..=30)
        .map(|index| format!("line-{index:02}\n"))
        .collect::<String>();

    assert!(state.pending_assistant.contains("line-01"));
    assert!(state.pending_assistant.contains("line-30"));
    assert!(!state.should_flush_history());
}

#[test]
fn long_pending_assistant_remains_single_active_transcript_cell() {
    let mut state = normal_state("", 0);
    state.history_flushed_message_count = state.stable_transcript_cells(80).len();
    state.request_in_flight = true;
    state.pending_assistant = (1..=24)
        .map(|index| format!("streamed line {index:02}\n"))
        .collect::<String>();

    assert!(!state.should_flush_history());
    assert!(state.pending_assistant.contains("streamed line 24"));
    assert!(state.pending_assistant.contains("streamed line 01"));

    let viewport_text = state
        .transcript_lines(80)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(
        viewport_text.contains("streamed line 01"),
        "{viewport_text}"
    );
    assert!(
        viewport_text.contains("streamed line 24"),
        "{viewport_text}"
    );
    assert_eq!(
        viewport_text.matches("streamed line 01").count(),
        1,
        "{viewport_text}"
    );
}

#[test]
fn long_pending_assistant_stays_live_until_completion() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (1..=30)
        .map(|index| format!("streamed stable-prefix line {index:02}\n"))
        .collect::<String>();

    let history = state.take_history_lines(80, 24);
    let history_text = plain_text_lines(&history).join("\n");
    let live_text = plain_text_lines(&state.pending_assistant_live_lines(80)).join("\n");

    assert!(
        history_text.is_empty(),
        "active assistant output should not be emitted to native history before completion: {history_text}"
    );
    assert!(
        live_text.contains("streamed stable-prefix line 01"),
        "early streaming lines should remain live until completion: {live_text}"
    );
    assert!(
        live_text.contains("streamed stable-prefix line 30"),
        "latest streaming lines should remain live: {live_text}"
    );
}

#[test]
fn pending_assistant_markdown_table_does_not_flush_before_completion() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = [
        "Slash 命令一览：\n",
        "\n",
        "┌──────────────────────┬───────┬──────────────┐\n",
        "│ 命令（144 个）       │ 状态  │ 说明         │\n",
        "├──────────────────────┼───────┼──────────────┤\n",
        "│ /compact             │ ✅    │ 手动触发压缩 │\n",
        "│ /plan                │ ✅    │ 计划模式     │\n",
        "│ /plugin              │ ✅    │ 插件管理     │\n",
        "│ /review              │ ✅    │ 代码审查     │\n",
    ]
    .concat();

    let history = state.take_history_lines(80, 24);
    let history_text = plain_text_lines(&history).join("\n");

    assert!(
        history_text.is_empty(),
        "streaming markdown tables must not be written to native history before final column widths are stable:\n{history_text}"
    );
}

/// Establishes a committed prior cell so the banner first-flush is already
/// done (Phase B gate) — incremental streaming commit is the default behavior,
/// so this just needs a prior committed cell — then returns the state ready to
/// stream.
fn streaming_commit_state() -> TuiState {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prior committed message".to_string(),
    ));
    state.pending_history_flush = true;
    let _ = state.take_history_lines(80, 24);
    assert!(
        state.transcript_ui.emission.emitted_cell_count > 0,
        "prior cell should be committed so streaming commit is past the banner first-flush"
    );
    state.request_in_flight = true;
    state
}

#[test]
fn streaming_incremental_commit_writes_stable_prefix_and_keeps_tail_live() {
    let mut state = streaming_commit_state();
    state.pending_assistant = (1..=30)
        .map(|index| format!("streamed stable-prefix line {index:02}\n"))
        .collect::<String>();

    let history = state.take_history_lines(80, 24);
    let history_text = plain_text_lines(&history).join("\n");
    let live_text = plain_text_lines(&state.pending_assistant_live_lines(80)).join("\n");

    // With the flag on, the stable prefix commits to scrollback incrementally...
    assert!(
        history_text.contains("streamed stable-prefix line 01"),
        "stable prefix should commit to history while streaming: {history_text}"
    );
    // ...but the last ASSISTANT_STREAM_LIVE_TAIL_LINES stay live and are not committed.
    assert!(
        !history_text.contains("streamed stable-prefix line 30"),
        "the growing tail must not be committed: {history_text}"
    );
    assert!(
        live_text.contains("streamed stable-prefix line 30"),
        "tail should still render live: {live_text}"
    );
    assert!(
        !live_text.contains("streamed stable-prefix line 01"),
        "already-committed prefix must not be re-shown live (no duplication): {live_text}"
    );
}

#[test]
fn streaming_commit_paces_lines_per_frame_and_catches_up_on_backlog() {
    // Small steady-state backlog: one frame reveals only a couple of lines so
    // the commit animates smoothly rather than jumping.
    let mut state = streaming_commit_state();
    state.pending_assistant = (1..=20)
        .map(|index| format!("paced line {index:02}\n"))
        .collect::<String>();
    let first = plain_text_lines(&state.take_history_lines(80, 24));
    assert!(
        first.len() <= 3,
        "steady-state commit should be paced to a few lines per frame: {first:?}"
    );

    // Large backlog (burst / paste): catch-up commits it all in one frame
    // instead of dripping it out line-by-line.
    let mut burst = streaming_commit_state();
    burst.pending_assistant = (1..=100)
        .map(|index| format!("burst line {index:03}\n"))
        .collect::<String>();
    let committed = plain_text_lines(&burst.take_history_lines(80, 24));
    assert!(
        committed.iter().any(|line| line.contains("burst line 001"))
            && committed.iter().any(|line| line.contains("burst line 080")),
        "large backlog should catch up in a single frame, got {} committed lines",
        committed.len()
    );
}

#[test]
fn streaming_incremental_commit_holds_back_tall_table() {
    let mut state = streaming_commit_state();
    let mut source =
        String::from("Intro paragraph before the table.\n\n| Column | Notes |\n| --- | --- |\n");
    for index in 0..15 {
        source.push_str(&format!("| row{index:02} | value {index:02} |\n"));
    }
    state.pending_assistant = source;

    let history = state.take_history_lines(80, 24);
    let history_text = plain_text_lines(&history).join("\n");

    // The prose prefix may commit, but no part of the still-streaming table may
    // reach scrollback (it would tear once later rows change column widths).
    assert!(
        !history_text.contains('│') && !history_text.contains('─'),
        "no rendered table border may be committed mid-stream: {history_text}"
    );
    assert!(
        !history_text.contains("row00") && !history_text.contains("Column"),
        "no table row/header may be committed mid-stream: {history_text}"
    );
}

#[test]
fn streaming_commit_releases_completed_table_and_following_prose() {
    // Regression (High #2): once a streamed table is closed (here by a blank
    // line + following prose) it can no longer reflow, so the whole table and
    // the prose beyond the live tail must commit to scrollback. The buggy
    // behavior held the table plus every later prose line stuck in the live tail
    // until turn completion.
    let mut state = streaming_commit_state();
    let mut source = String::from(
        "Intro before the table.\n\n\
         | Column | Notes |\n| --- | --- |\n| a | one |\n| b | two |\n\n",
    );
    for index in 0..40 {
        source.push_str(&format!("prose paragraph line {index:02}\n"));
    }
    state.pending_assistant = source;

    // Drive several frames so frame-paced commit drains the stable backlog.
    let mut committed = Vec::new();
    for _ in 0..40 {
        committed.extend(plain_text_lines(&state.take_history_lines(80, 24)));
    }
    let history_text = committed.join("\n");

    assert!(
        history_text.contains('│') && history_text.contains("Column"),
        "a completed table must commit to scrollback once it is followed by prose:\n{history_text}"
    );
    assert!(
        history_text.contains("prose paragraph line 00"),
        "prose after a completed table must commit past the live tail:\n{history_text}"
    );
    assert!(
        !history_text.contains("prose paragraph line 39"),
        "the still-growing live tail must not be committed:\n{history_text}"
    );
}

#[test]
fn streaming_commit_holds_answer_until_thinking_is_materialized() {
    // Regression (High #3): a completed thinking block is committed to
    // scrollback ahead of the final answer at turn completion. If the answer
    // commits incrementally *before* that, scrollback ends up ordered
    // `answer prefix -> thinking -> answer tail`. Incremental commit must be held
    // until the thinking is materialized so the thinking precedes the whole
    // answer.
    let mut state = streaming_commit_state();

    state.apply_stream_event(StreamEvent::ThinkingStarted {
        session_id: "s".to_string(),
        provider: ProviderId::Anthropic,
    });
    state.apply_stream_event(StreamEvent::ThinkingDelta {
        session_id: "s".to_string(),
        delta: "reasoning-before-answer marker".to_string(),
    });
    state.apply_stream_event(StreamEvent::ThinkingCompleted {
        session_id: "s".to_string(),
        provider: ProviderId::Anthropic,
    });

    let answer: String = (0..20)
        .map(|index| format!("answer paragraph line {index:02}\n"))
        .collect();
    state.apply_stream_event(StreamEvent::AssistantDelta {
        session_id: "s".to_string(),
        delta: answer.clone(),
    });

    // While the completed thinking is still pending materialization, no answer
    // line may be committed to scrollback yet.
    let mid_stream = plain_text_lines(&state.take_history_lines(80, 24)).join("\n");
    assert!(
        !mid_stream.contains("answer paragraph"),
        "answer must not commit to scrollback before thinking is materialized:\n{mid_stream}"
    );

    // Completion: thinking is pushed to history, then the answer.
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Thinking {
                    text: "reasoning-before-answer marker".to_string(),
                    signature: None,
                },
                TranscriptBlock::Text { text: answer },
            ],
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let mut committed = Vec::new();
    for _ in 0..10 {
        committed.extend(plain_text_lines(&state.take_history_lines(80, 24)));
    }
    let joined = committed.join("\n");

    let thinking_idx = joined
        .find("Thinking")
        .or_else(|| joined.find("reasoning-before-answer"))
        .unwrap_or_else(|| panic!("thinking must be committed to scrollback:\n{joined}"));
    let first_answer_idx = joined
        .find("answer paragraph line 00")
        .unwrap_or_else(|| panic!("answer must be committed to scrollback:\n{joined}"));
    assert!(
        thinking_idx < first_answer_idx,
        "thinking must be committed before the answer, not inside it:\n{joined}"
    );
    // The answer must be contiguous — no thinking header sitting between answer
    // lines.
    let answer_region = &joined[first_answer_idx..];
    assert!(
        !answer_region.contains("Thinking"),
        "no thinking may be interleaved inside the committed answer:\n{joined}"
    );
    assert!(
        joined.contains("answer paragraph line 19"),
        "the full answer must be committed:\n{joined}"
    );
}

#[test]
fn streaming_commit_gated_off_before_first_cell_flush() {
    // Streaming, but no committed cell yet (emitted_cell_count == 0): incremental
    // commit must NOT run — otherwise a streaming flush could become the banner
    // first-flush and split/duplicate the intro banner. This banner-atomicity
    // gate holds regardless of the (now removed) opt-in flag.
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    assert_eq!(state.transcript_ui.emission.emitted_cell_count, 0);
    state.pending_assistant = (1..=30)
        .map(|index| format!("streamed line {index:02}\n"))
        .collect::<String>();

    let history = state.take_history_lines(80, 24);
    assert!(
        plain_text_lines(&history).join("\n").is_empty(),
        "streaming must not commit before the first cell/banner has flushed"
    );
}

#[test]
fn streaming_commit_completion_produces_no_scrollback_duplication() {
    let mut state = streaming_commit_state();
    let answer: String = (1..=20)
        .map(|index| format!("answer body line {index:02}\n"))
        .collect();
    state.pending_assistant = answer.clone();

    // Incremental commit of the stable prefix while streaming.
    let mut committed = plain_text_lines(&state.take_history_lines(80, 24));
    assert!(
        !committed.is_empty(),
        "stable prefix should commit incrementally while streaming"
    );

    // Completion must commit only the remaining tail — the already-committed
    // prefix must not be re-emitted (neither by the completed-source path nor
    // by the new committed message cell).
    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    assert!(!finished);
    committed.extend(plain_text_lines(&state.take_history_lines(80, 24)));

    let joined = committed.join("\n");
    for index in 1..=20 {
        let needle = format!("answer body line {index:02}");
        assert_eq!(
            joined.matches(&needle).count(),
            1,
            "line {index} must be committed to scrollback exactly once:\n{joined}"
        );
    }
}

#[test]
fn streaming_commit_width_change_defers_then_rebuilds_on_settle() {
    let mut state = streaming_commit_state();
    state.pending_assistant = (1..=30)
        .map(|index| format!("streamed stable-prefix line {index:02}\n"))
        .collect::<String>();
    let _ = state.take_history_lines(80, 24);
    assert!(
        state
            .transcript_ui
            .emission
            .assistant_stream_emitted_line_count
            > 0,
        "prefix should have committed to scrollback at width 80"
    );

    // A mid-stream width change DEFERS the rebuild (no immediate purge) so a
    // drag does not flash / disrupt the live render on every frame. Stream row
    // accounting resets so the tail re-renders at the new width.
    state.check_width_reflow(60);
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "streaming resize should defer the rebuild"
    );
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "streaming resize must not purge immediately mid-drag"
    );
    assert_eq!(
        state
            .transcript_ui
            .emission
            .assistant_stream_emitted_line_count,
        0
    );
    assert_eq!(
        state.transcript_ui.emission.assistant_stream_width,
        Some(60)
    );

    // Regression (Medium #3): a draw transaction while the reflow is still
    // deferred must NOT commit any assistant lines at the new width. The
    // old-width prefix is still physically in scrollback until settle, and the
    // emitted-line counter was reset to zero, so committing now would re-append
    // the prefix (duplicate) at width 60.
    let deferred = plain_text_lines(&state.take_history_lines(60, 24)).join("\n");
    assert!(
        !deferred.contains("streamed stable-prefix line"),
        "no assistant lines may be committed while the reflow is deferred:\n{deferred}"
    );

    // On settle (driven by the main loop's resize-settle deadline), the full
    // source rebuild purges native scrollback and re-emits the whole transcript.
    state.rebuild_committed_history_from_source();
    assert!(
        state.transcript_ui.emission.needs_scrollback_clear,
        "settle rebuild purges native scrollback"
    );
    assert!(
        !state.transcript_ui.emission.reflow_pending,
        "reflow_pending cleared after the rebuild"
    );
}

#[test]
fn mid_stream_resize_forces_final_rebuild_at_turn_end() {
    // A resize rebuild that runs mid-stream re-emits only the transient stream
    // wrapping. It must be reconciled by ONE final source-backed rebuild when the
    // turn ends. The sticky `reflow_ran_during_stream` flag drives this.
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prompt".to_string(),
    ));
    state.request_in_flight = true;

    // Settle rebuild fires mid-stream (request_in_flight = true).
    state.rebuild_committed_history_from_source();
    assert!(
        state.transcript_ui.emission.reflow_ran_during_stream,
        "a rebuild during streaming must set the sticky repair flag"
    );

    // Simulate the mid-stream flush consuming the purge, so we can detect the
    // SECOND (repair) rebuild at turn end.
    state.transcript_ui.emission.needs_scrollback_clear = false;

    // Turn ends → the repair rebuild must fire and clear the sticky flag.
    state.stop_waiting_animation();
    assert!(
        state.transcript_ui.emission.needs_scrollback_clear,
        "turn end must force one final source-backed rebuild after a mid-stream reflow"
    );
    assert!(
        !state.transcript_ui.emission.reflow_ran_during_stream,
        "the sticky repair flag must be cleared so it fires only once"
    );
}

#[test]
fn turn_end_without_mid_stream_resize_does_not_rebuild() {
    // No resize during the stream → no repair rebuild at turn end.
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prompt".to_string(),
    ));
    state.request_in_flight = true;
    state.transcript_ui.emission.needs_scrollback_clear = false;

    state.stop_waiting_animation();
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "turn end with no mid-stream reflow must not purge/rebuild"
    );
}

#[test]
fn completed_thinking_does_not_split_streaming_assistant_history() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "inspect the repo first".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });
    let answer = [
        "最大缺口：\n",
        "1. Slash 命令 — 144 个中几乎全部缺失，这是用户交互的核心入口\n",
        "2. Voice 语音 — 整个语音输入/输出管线缺失\n",
        "3. 高级协作 — Coordinator 多 Agent、Agent Swarms、Team tools 缺失\n",
    ]
    .concat();
    state.pending_assistant = answer.clone();

    let early_history = state.take_history_lines(80, 24);
    assert!(
        early_history.is_empty(),
        "assistant text must not be flushed before completed thinking can be committed"
    );

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    let history = state.take_history_lines(80, 24);
    let history_text = plain_text_lines(&history).join("\n");
    let thinking_index = history_text
        .find("∴ Thinking")
        .expect("completed thinking should be flushed with final history");
    let first_answer_index = history_text
        .find("1. Slash 命令")
        .expect("final assistant answer should be flushed");
    let second_answer_index = history_text
        .find("2. Voice 语音")
        .expect("final assistant answer should not be truncated");

    assert!(
        thinking_index < first_answer_index && first_answer_index < second_answer_index,
        "completed thinking should precede the final assistant answer instead of splitting it:\n{history_text}"
    );
}

#[test]
fn pending_assistant_lines_use_muted_style_not_bright_white() {
    let rendered = render_pending_assistant_lines("final answer\n", 80);
    let has_bright_white_text = rendered
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| !span.content.trim().is_empty() && span.style.fg == Some(Color::White));

    assert!(
        !has_bright_white_text,
        "pending assistant text should not render as bright white: {rendered:?}"
    );
}

#[test]
fn long_pending_markdown_table_stays_in_live_transcript() {
    let mut state = normal_state("", 0);
    state.pending_assistant = [
            "## Summary\n",
            "\n",
            "This intro paragraph is long enough to consume several visual rows before the table begins.\n",
            "\n",
            "| Column | Notes |\n",
            "| --- | --- |\n",
            "| tool | This cell stays in the live tail only if the viewport can still fit it without pushing the prompt chrome into scrollback. |\n",
        ]
        .concat();

    let transcript = state
        .transcript_lines(24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(transcript.contains("Summary"), "{transcript}");
    assert!(transcript.contains("intro paragraph"), "{transcript}");
    assert!(transcript.contains("Column"), "{transcript}");
}

#[test]
fn trailing_markdown_table_stays_visible_until_completion() {
    let mut state = normal_state("", 0);
    state.pending_assistant = [
            "## Summary\n",
            "\n",
            "This intro paragraph is long enough to consume several visual rows in a narrow terminal.\n",
            "\n",
            "| Column | Notes |\n",
            "| --- | --- |\n",
            "| tool | This row should stay live until the table is followed by non-table content. |\n",
        ]
        .concat();

    let first_transcript = state
        .transcript_lines(24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(first_transcript.contains("Column"), "{first_transcript}");

    state.pending_assistant.push_str("\nAfter table.\n");
    let final_transcript = state
        .transcript_lines(24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(final_transcript.contains("Column"), "{final_transcript}");
    assert!(
        final_transcript.contains("After table"),
        "{final_transcript}"
    );
}

#[test]
fn pending_assistant_with_partial_table_stays_as_active_cell() {
    let mut state = normal_state("", 0);
    state.history_flushed_message_count = state.stable_transcript_cells(24).len();
    state.pending_assistant = [
        "## Summary\n",
        "\n",
        "Intro paragraph that is long enough to consume several visual rows.\n",
        "\n",
        "| Column | Notes |\n",
    ]
    .concat();

    assert!(!state.should_flush_history());
    assert!(state.pending_assistant.contains("| Column | Notes |"));

    let viewport_text = state
        .transcript_lines(24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(viewport_text.contains("Summary"), "{viewport_text}");
}

#[test]
fn streaming_markdown_table_completion_keeps_single_virtual_transcript_copy() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    let answer = [
            "## Summary\n",
            "\n",
            "| Column | Notes |\n",
            "| --- | --- |\n",
            "| tool | This wrapped row is long enough to force multiple viewport reductions before the answer completes. |\n",
            "| status | done |\n",
        ]
        .concat();
    state.pending_assistant = answer.clone();

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    assert!(!finished);
    assert!(state.pending_assistant.is_empty());
    assert_eq!(state.messages.len(), 1);
    let viewport_text = state
        .transcript_lines(24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(
        viewport_text.matches("Summary").count(),
        1,
        "{viewport_text}"
    );
    assert_eq!(
        viewport_text.matches("Column").count(),
        1,
        "{viewport_text}"
    );
    assert_eq!(
        viewport_text.matches("status").count(),
        1,
        "{viewport_text}"
    );
    assert_eq!(viewport_text.matches('└').count(), 1, "{viewport_text}");
}

#[test]
fn finalized_streaming_answer_renders_once_in_virtual_transcript() {
    let mut state = normal_state("", 0);
    let answer = "最终结论\n\n1. Rust TUI should commit history once.\n2. The prompt stays below the committed answer.".to_string();
    state.pending_assistant = answer.clone();

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    assert!(!finished);
    assert!(state.pending_assistant.is_empty());
    assert_eq!(state.messages.len(), 1);
    let viewport_text = state
        .transcript_lines(80)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(
        viewport_text
            .matches("Rust TUI should commit history once")
            .count(),
        1
    );
    assert!(viewport_text.contains("最终结论"), "{viewport_text}");
}

#[test]
fn completed_stream_history_skips_matching_source_cell_by_message_id() {
    let mut state = normal_state("", 0);
    let answer = "streamed prefix\nstreamed tail".to_string();
    let final_message = TranscriptMessage::new(MessageRole::Assistant, answer.clone());
    let final_message_id = final_message.id.clone();
    state.messages.push(final_message);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "later committed cell".to_string(),
    ));
    state.pending_history_flush = true;
    state.transcript_ui.emission.emission_width = Some(80);
    state.transcript_ui.emission.assistant_stream_width = Some(80);
    state
        .transcript_ui
        .emission
        .assistant_stream_emitted_line_count = 1;
    state
        .transcript_ui
        .emission
        .assistant_stream_completed_source = Some(answer);
    state
        .transcript_ui
        .emission
        .assistant_stream_completed_message_id = Some(final_message_id);

    let history = state.take_history_lines(80, 24);
    let history_text = plain_text_lines(&history).join("\n");

    assert_eq!(
        history_text.matches("streamed prefix").count(),
        0,
        "already-emitted stream prefix must not be flushed from the source cell:\n{history_text}"
    );
    assert_eq!(
        history_text.matches("streamed tail").count(),
        1,
        "remaining stream tail should be flushed exactly once:\n{history_text}"
    );
    assert_eq!(
        history_text.matches("later committed cell").count(),
        1,
        "source cells after the completed stream message should still flush:\n{history_text}"
    );
}

#[test]
fn finalized_assistant_history_renders_markdown_as_single_message() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "earlier output".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "## 结果\n\n| 模块 | 状态 |\n| --- | --- |\n| core | ok |\n".to_string(),
    ));
    state.pending_history_flush = true;

    let history = state.take_history_lines(80, 20);
    let rendered = history
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        rendered.iter().filter(|line| line.starts_with('●')).count(),
        2
    );
    assert!(rendered.iter().any(|line| line.contains("结果")));
    assert!(rendered.iter().any(|line| line.contains("┌")));
    assert!(rendered.iter().any(|line| line.contains("模块")));
    assert!(!rendered.iter().any(|line| line.contains("| 模块 | 状态 |")));
}

#[test]
fn transcript_layout_is_content_driven_after_shrink() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (1..=30)
        .map(|index| format!("streamed line {index:02}\n"))
        .collect::<String>();
    let area = Rect::new(0, 0, 80, 24);
    let input_view = build_input_view(&state.input, state.input_cursor, 77, MAX_INPUT_INNER_HEIGHT);
    let request_status_height = state.request_status_lines().len() as u16;

    let active_layout = state.main_layout_regions(area, &input_view, request_status_height);
    assert_eq!(active_layout[0].height, 19);
    assert_eq!(active_layout[7].height, 0);

    state.request_in_flight = false;
    state.pending_assistant.clear();
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "final answer".to_string(),
    ));
    let settled_layout = state.main_layout_regions(area, &input_view, 0);
    assert!(
        settled_layout[0].height < 19,
        "after content shrinks, transcript area should be content-driven, not sticky: {}",
        settled_layout[0].height
    );

    let transcript_view =
        state.visible_transcript_lines_for_view(80, settled_layout[0].height as usize, true);
    let transcript_text = plain_text_lines(&transcript_view.visible_lines).join("\n");
    assert!(
        transcript_text.contains("final answer"),
        "content-driven transcript should contain the final answer: {transcript_text}"
    );
}

#[test]
fn ctrl_o_opens_transcript_pager_overlay() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "short collapsed summary".to_string(),
    ));

    state.toggle_expanded_tool_details();

    assert!(matches!(
        state.overlay,
        Some(OverlayState::TranscriptPager(_))
    ));

    state.toggle_expanded_tool_details();

    assert!(state.overlay.is_none());
}

#[test]
fn transcript_layout_constraint_uses_full_budget_when_content_overflows() {
    assert!(matches!(
        transcript_layout_constraint(30, 13),
        Constraint::Length(13)
    ));
    assert!(matches!(
        transcript_layout_constraint(13, 13),
        Constraint::Length(13)
    ));
    assert!(matches!(
        transcript_layout_constraint(5, 13),
        Constraint::Length(5)
    ));
    assert!(matches!(
        transcript_layout_constraint(5, 0),
        Constraint::Min(1)
    ));
}

#[test]
fn repeated_assistant_deltas_produce_single_active_cell_and_one_committed_entry() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    for i in 0..10 {
        state.apply_stream_event(StreamEvent::AssistantDelta {
            session_id: "s".to_string(),
            delta: format!("chunk-{i} "),
        });
    }

    assert!(state.pending_assistant.contains("chunk-0"));
    assert!(state.pending_assistant.contains("chunk-9"));
    assert_eq!(state.messages.len(), 0);

    let answer = state.pending_assistant.clone();
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(state.pending_assistant.is_empty());
    assert_eq!(state.messages.len(), 1);
}

#[test]
fn history_emission_does_not_include_input_or_status_chrome() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "committed text".to_string(),
    ));
    state.pending_history_flush = true;

    let history = state.take_history_lines(80, 24);
    assert!(!history.is_empty());

    let history_text: String = history
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();

    assert!(
        !history_text.contains("›"),
        "history should not contain input chrome"
    );
    assert!(
        !history_text.contains("Enter submits"),
        "history should not contain status line text"
    );
    assert!(history_text.contains("committed text"));
}

#[test]
fn committed_blocks_have_at_most_one_blank_separator() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "first answer".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "second answer".to_string(),
    ));
    state.pending_history_flush = true;

    let history = state.take_history_lines(80, 24);
    let mut consecutive_blanks = 0usize;
    let mut max_consecutive = 0usize;
    for line in &history {
        if line.spans.is_empty() {
            consecutive_blanks += 1;
        } else {
            max_consecutive = max_consecutive.max(consecutive_blanks);
            consecutive_blanks = 0;
        }
    }
    max_consecutive = max_consecutive.max(consecutive_blanks);
    assert!(
        max_consecutive <= 1,
        "should have at most 1 blank separator, found {max_consecutive} consecutive blanks"
    );
}

#[test]
fn ctrl_o_pager_and_prompt_show_same_cell_order() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prompt one".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "reply one".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prompt two".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "reply two".to_string(),
    ));

    let prompt_lines = state.transcript_lines(80);
    let prompt_text: Vec<String> = prompt_lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .filter(|s| !s.trim().is_empty())
        .collect();

    state.open_transcript_pager(80, 40);
    let pager = match &state.overlay {
        Some(OverlayState::TranscriptPager(p)) => p,
        _ => panic!("pager should be open"),
    };

    let pager_text: Vec<String> = pager
        .rendered_window
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .filter(|s| !s.trim().is_empty())
        .collect();

    let prompt_has_reply_one = prompt_text.iter().any(|l| l.contains("reply one"));
    let pager_has_reply_one = pager_text.iter().any(|l| l.contains("reply one"));
    assert!(prompt_has_reply_one, "prompt should show reply one");
    assert!(pager_has_reply_one, "pager should show reply one");

    let prompt_order = prompt_text
        .iter()
        .position(|l| l.contains("reply one"))
        .zip(prompt_text.iter().position(|l| l.contains("reply two")));
    let pager_order = pager_text
        .iter()
        .position(|l| l.contains("reply one"))
        .zip(pager_text.iter().position(|l| l.contains("reply two")));
    if let (Some((p1, p2)), Some((g1, g2))) = (prompt_order, pager_order) {
        assert!(p1 < p2, "prompt: reply one should come before reply two");
        assert!(g1 < g2, "pager: reply one should come before reply two");
    }
}

#[test]
fn width_change_defers_reflow_then_rebuilds_on_settle() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "initial content".to_string(),
    ));
    state.pending_history_flush = true;
    let _ = state.take_history_lines(80, 24);

    assert!(state.transcript_ui.emission.emitted_cell_count > 0);
    assert_eq!(state.transcript_ui.emission.emission_width, Some(80));

    // A width change defers the committed-history rebuild (debounced by the main
    // loop's resize-settle deadline) instead of purging on every drag frame.
    state.prepare_pending_history_emission(60, 24);
    assert!(
        state.transcript_ui.emission.reflow_pending,
        "width change should mark a deferred reflow"
    );
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "width change must not purge immediately"
    );

    // On settle the full source rebuild purges native scrollback and re-emits.
    state.rebuild_committed_history_from_source();
    assert!(
        state.transcript_ui.emission.needs_scrollback_clear,
        "settle rebuild purges native scrollback"
    );
}

#[test]
fn width_change_during_streaming_defers_reflow() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "prior content".to_string(),
    ));
    state.pending_history_flush = true;
    let _ = state.take_history_lines(80, 24);

    state.request_in_flight = true;
    state.pending_assistant = "streaming...".to_string();
    state
        .transcript_ui
        .emission
        .assistant_stream_emitted_line_count = 4;
    state
        .transcript_ui
        .emission
        .assistant_stream_pending_line_count = Some(6);
    state.transcript_ui.emission.assistant_stream_width = Some(80);

    state.check_width_reflow(60);

    assert!(
        state.transcript_ui.emission.reflow_pending,
        "reflow should be deferred while streaming"
    );
    assert_eq!(
        state
            .transcript_ui
            .emission
            .assistant_stream_emitted_line_count,
        0,
        "streaming width changes should drop old-width assistant row accounting"
    );
    assert_eq!(
        state
            .transcript_ui
            .emission
            .assistant_stream_pending_line_count,
        None,
        "streaming width changes should clear pending old-width assistant row accounting"
    );
    assert_eq!(
        state.transcript_ui.emission.assistant_stream_width,
        Some(60),
        "assistant stream row accounting should move to the resized width"
    );
    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "streaming width changes should not request the full committed-history purge"
    );

    state.stop_waiting_animation();

    assert!(
        state.transcript_ui.emission.needs_scrollback_clear,
        "reflow should execute after streaming stops"
    );
    assert!(!state.transcript_ui.emission.reflow_pending);
}

#[test]
fn bash_streaming_to_completed_produces_one_committed_block() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "s".to_string(),
        tool_use_id: "bash-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: "{\"command\":\"ls\"}".to_string(),
    });
    for i in 0..5 {
        state.apply_stream_event(StreamEvent::ToolProgress {
            session_id: "s".to_string(),
            tool_use_id: "bash-1".to_string(),
            tool_name: "Bash".to_string(),
            progress: serde_json::json!({ "status": format!("running step {i}") }),
        });
    }
    state.apply_stream_event(StreamEvent::ToolUseCompleted {
        session_id: "s".to_string(),
        tool_use_id: "bash-1".to_string(),
        tool_name: "Bash".to_string(),
        kind: orbcode_protocol::ToolUseCompletionKind::Success,
    });

    let message = TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::ToolResult {
            tool_use_id: "bash-1".to_string(),
            content: "total 60".to_string().into(),
            is_error: false,
            metadata: None,
        }],
    );
    state.apply_stream_event(StreamEvent::UserMessage { message });

    assert_eq!(
        state.messages.len(),
        1,
        "tool result should be committed as one message"
    );

    state.pending_history_flush = true;
    let history = state.take_history_lines(80, 24);
    assert!(
        !history.is_empty(),
        "committed bash tool should produce history lines"
    );

    let history_text: String = history
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        history_text.contains("total 60")
            || history_text.contains("Bash")
            || history_text.contains("ls"),
        "history should contain the tool result or tool name: {history_text}"
    );
}

#[test]
fn consecutive_thinking_updates_do_not_duplicate_thinking_blocks() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.apply_stream_event(StreamEvent::ThinkingStarted {
        session_id: "s".to_string(),
        provider: ProviderId::Anthropic,
    });
    for i in 0..5 {
        state.apply_stream_event(StreamEvent::ThinkingDelta {
            session_id: "s".to_string(),
            delta: format!("thinking chunk {i}\n"),
        });
    }

    let transcript = state
        .transcript_lines(80)
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect::<String>();

    let thinking_count = transcript.matches("Thinking").count();
    assert!(
        thinking_count <= 1,
        "should have at most 1 thinking block header, found {thinking_count}: {transcript}"
    );
}

#[test]
fn width_unchanged_emission_does_not_set_scrollback_clear() {
    // Scope: this only covers the emission-state path (`check_width_reflow` via
    // `prepare_pending_history_emission`), which is width-gated and does not set
    // `needs_scrollback_clear` when the width is unchanged.
    //
    // NOTE: it does NOT prove that a height-only resize avoids a scrollback
    // purge at runtime. The runtime resize path
    // (`prepare_draw_transaction` → `mark_resize_reflow_pending` → the main-loop
    // settle → `rebuild_committed_history_from_source` →
    // `reset_inline_scrollback_for_reflow`) fires on ANY observed resize and DOES
    // purge (`ESC[2J`+`ESC[3J`) on a height-only change. That purge is
    // intentional codex-parity: adjusting the window height pins the banner to
    // the top and drops native/pre-TUI scrollback, exactly like codex.
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "some content".to_string(),
    ));
    state.pending_history_flush = true;
    let _ = state.take_history_lines(80, 24);

    state.prepare_pending_history_emission(80, 12);

    assert!(
        !state.transcript_ui.emission.needs_scrollback_clear,
        "same-width emission should not set the scrollback-clear flag"
    );
}

#[test]
fn transcript_pager_search_finds_and_navigates_cells() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "first prompt".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "unique-marker-xyz reply".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "second prompt".to_string(),
    ));

    state.open_transcript_pager(80, 40);
    let pager = match state.overlay.as_mut() {
        Some(OverlayState::TranscriptPager(p)) => p,
        _ => panic!("pager should be open"),
    };

    pager.search_query = "unique-marker-xyz".to_string();
    let action = crate::overlays::transcript_pager::apply_transcript_pager_key(
        pager,
        &crossterm::event::KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
    );
    assert_eq!(
        action,
        crate::overlays::transcript_pager::TranscriptPagerAction::None
    );
    assert!(pager.search_status.is_some());
    assert!(
        pager.search_status.as_ref().unwrap().contains("Match"),
        "search should find the marker: {:?}",
        pager.search_status
    );
}

#[test]
fn transcript_pager_scroll_navigation() {
    let mut state = normal_state("", 0);
    for i in 0..20 {
        state.messages.push(TranscriptMessage::new(
            if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            format!("message {i}"),
        ));
    }

    state.open_transcript_pager(80, 10);
    let pager = match state.overlay.as_mut() {
        Some(OverlayState::TranscriptPager(p)) => p,
        _ => panic!("pager should be open"),
    };

    let initial_window = pager.rendered_window.clone();
    assert!(!initial_window.is_empty(), "pager should have content");

    crate::overlays::transcript_pager::apply_transcript_pager_key(
        pager,
        &crossterm::event::KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
    );
    pager.sync_viewport(Rect::new(0, 0, 80, 10));
    let head_text: String = pager
        .rendered_window
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        head_text.contains("message 0"),
        "g should jump to head: {head_text}"
    );

    crate::overlays::transcript_pager::apply_transcript_pager_key(
        pager,
        &crossterm::event::KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
    );
    pager.sync_viewport(Rect::new(0, 0, 80, 10));
    let tail_text: String = pager
        .rendered_window
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        tail_text.contains("message 19"),
        "G should jump to tail: {tail_text}"
    );
}

#[test]
fn transcript_pager_arrow_and_page_keys_scroll_from_bottom() {
    let mut state = normal_state("", 0);
    for i in 0..40 {
        state.messages.push(TranscriptMessage::new(
            if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            format!("message {i}"),
        ));
    }

    state.open_transcript_pager(80, 10);
    let pager = match state.overlay.as_mut() {
        Some(OverlayState::TranscriptPager(p)) => p,
        _ => panic!("pager should be open"),
    };

    assert_eq!(pager.scroll_from_bottom(), 0);
    crate::overlays::transcript_pager::apply_transcript_pager_key(
        pager,
        &crossterm::event::KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
    );
    assert_eq!(
        pager.scroll_from_bottom(),
        1,
        "Up should scroll one row away from the tail"
    );

    crate::overlays::transcript_pager::apply_transcript_pager_key(
        pager,
        &crossterm::event::KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
    );
    assert_eq!(
        pager.scroll_from_bottom(),
        0,
        "Down should scroll back toward the tail"
    );

    crate::overlays::transcript_pager::apply_transcript_pager_key(
        pager,
        &crossterm::event::KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
    );
    assert!(
        pager.scroll_from_bottom() > 1,
        "PageUp should scroll by more than one row"
    );

    crate::overlays::transcript_pager::apply_transcript_pager_key(
        pager,
        &crossterm::event::KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );
    assert_eq!(
        pager.scroll_from_bottom(),
        0,
        "PageDown should scroll back toward the tail"
    );
}

#[test]
fn search_pattern_summaries_update_in_place_and_do_not_duplicate() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "s".to_string(),
        tool_use_id: "grep-1".to_string(),
        tool_name: "Grep".to_string(),
        tool_input: "{\"pattern\":\"TODO\"}".to_string(),
    });

    for count in 1..=5 {
        state.apply_stream_event(StreamEvent::ToolProgress {
            session_id: "s".to_string(),
            tool_use_id: "grep-1".to_string(),
            tool_name: "Grep".to_string(),
            progress: serde_json::json!({
                "type": "tool_progress",
                "status": format!("Searched for {count} patterns"),
            }),
        });
    }

    let activities = state.live_tool_activities_to_render();
    assert_eq!(
        activities.len(),
        1,
        "should have exactly one live tool activity for the grep"
    );
    assert!(
        activities[0]
            .status_line
            .contains("Searched for 5 patterns"),
        "status line should show the latest count: {}",
        activities[0].status_line
    );
    assert_eq!(
        activities[0].status_line.matches("Searched for").count(),
        1,
        "status line should contain exactly one 'Searched for' mention"
    );
}

#[test]
fn active_transcript_snapshot_covers_thinking_and_pending_assistant() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.active_thinking = Some(crate::prompt_state::ActiveThinkingState {
        text: "Analyzing the codebase structure.".to_string(),
        is_streaming: true,
        completed_at: None,
    });
    state.pending_assistant = "Here is my analysis...".to_string();

    let snapshot = state.active_transcript_snapshot(90);
    let text = plain_text_lines(&snapshot.lines).join("\n");

    assert!(
        text.contains("Analyzing the codebase structure"),
        "snapshot should include active thinking text: {text}"
    );
    assert!(
        text.contains("Here is my analysis"),
        "snapshot should include pending assistant text: {text}"
    );
    assert!(snapshot.revision != 0, "revision should be non-zero");

    let rev1 = snapshot.revision;
    state.pending_assistant.push_str(" with more detail.");
    let rev2 = state.active_transcript_snapshot(90).revision;
    assert_ne!(
        rev1, rev2,
        "revision should change when pending assistant text changes"
    );
}

#[test]
fn ctrl_o_pager_shows_active_snapshot_and_ignores_later_updates() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "first prompt".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "first reply".to_string(),
    ));
    state.request_in_flight = true;
    state.active_thinking = Some(crate::prompt_state::ActiveThinkingState {
        text: "Considering the approach.".to_string(),
        is_streaming: true,
        completed_at: None,
    });
    state.pending_assistant = "Here is my second reply...".to_string();

    state.open_transcript_pager(90, 40);
    let (pager_text, original_source_cells_len) = match &state.overlay {
        Some(OverlayState::TranscriptPager(p)) => (
            plain_text_lines(&p.rendered_window).join("\n"),
            p.source_cells_len(),
        ),
        _ => panic!("pager should be open"),
    };

    assert!(
        pager_text.contains("first reply"),
        "pager should show committed cell: {pager_text}"
    );
    assert!(
        pager_text.contains("Considering the approach"),
        "pager should show active thinking: {pager_text}"
    );
    assert!(
        pager_text.contains("Here is my second reply"),
        "pager should show pending assistant: {pager_text}"
    );

    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "second prompt".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "committed second reply".to_string(),
    ));
    state.pending_assistant = "streaming third reply...".to_string();

    let (after_update_text, after_update_source_cells_len) = match &state.overlay {
        Some(OverlayState::TranscriptPager(p)) => (
            plain_text_lines(&p.rendered_window).join("\n"),
            p.source_cells_len(),
        ),
        _ => panic!("pager should still be open"),
    };

    assert_eq!(
        after_update_source_cells_len, original_source_cells_len,
        "pager source cells should remain the open-time snapshot"
    );
    assert!(
        after_update_text.contains("first reply"),
        "pager should keep open-time committed content: {after_update_text}"
    );
    assert!(
        after_update_text.contains("Here is my second reply"),
        "pager should keep open-time live tail: {after_update_text}"
    );
    assert!(
        !after_update_text.contains("committed second reply"),
        "pager should not show newly committed cells after open: {after_update_text}"
    );
    assert!(
        !after_update_text.contains("streaming third reply"),
        "pager should not show streaming updates after open: {after_update_text}"
    );
}

#[test]
fn turn_finished_keeps_transcript_pager_open() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "finished reply visible in pager".to_string(),
    ));
    state.open_transcript_pager(80, 24);

    let finished = state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(finished);
    let pager_text = match &state.overlay {
        Some(OverlayState::TranscriptPager(p)) => plain_text_lines(&p.rendered_window).join("\n"),
        _ => panic!("transcript pager should remain open after TurnFinished"),
    };
    assert!(
        pager_text.contains("finished reply visible in pager"),
        "pager should preserve its open-time content: {pager_text}"
    );
}

#[test]
fn pager_detail_shows_thinking_in_mixed_assistant_message() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "explain".to_string(),
    ));
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::Thinking {
                text: "Let me reason about this carefully.".to_string(),
                signature: None,
            },
            TranscriptBlock::Text {
                text: "Here is the answer.".to_string(),
            },
        ],
    ));

    state.open_transcript_pager(90, 40);
    let pager_text = match &state.overlay {
        Some(OverlayState::TranscriptPager(p)) => plain_text_lines(&p.rendered_window).join("\n"),
        _ => panic!("pager should be open"),
    };

    assert!(
        pager_text.contains("Let me reason about this carefully"),
        "pager detail should show thinking from mixed assistant message: {pager_text}"
    );
    assert!(
        pager_text.contains("Here is the answer"),
        "pager detail should show text from mixed assistant message: {pager_text}"
    );
}

#[test]
fn snapshot_revision_changes_on_same_length_content_replacement() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = "aaaa".to_string();
    let rev1 = state.active_transcript_snapshot(90).revision;

    state.pending_assistant = "bbbb".to_string();
    let rev2 = state.active_transcript_snapshot(90).revision;

    assert_ne!(
        rev1, rev2,
        "revision must change when content changes even if length is the same"
    );
}

#[test]
fn snapshot_revision_changes_on_width_change() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = "some streaming text".to_string();
    let rev_wide = state.active_transcript_snapshot(120).revision;
    let rev_narrow = state.active_transcript_snapshot(40).revision;

    assert_ne!(
        rev_wide, rev_narrow,
        "revision must change when width changes"
    );
}

#[test]
fn pager_keeps_static_cells_when_source_changes() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "original prompt".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "original reply".to_string(),
    ));

    state.open_transcript_pager(90, 40);
    let before = match &state.overlay {
        Some(OverlayState::TranscriptPager(p)) => plain_text_lines(&p.rendered_window).join("\n"),
        _ => panic!("pager should be open"),
    };
    assert!(before.contains("original reply"), "{before}");

    state.messages[1] =
        TranscriptMessage::new(MessageRole::Assistant, "replaced reply".to_string());
    state.transcript_ui.source_signature = 0;

    let after = match &state.overlay {
        Some(OverlayState::TranscriptPager(p)) => plain_text_lines(&p.rendered_window).join("\n"),
        _ => panic!("pager should be open"),
    };

    assert!(
        after.contains("original reply"),
        "pager should keep original content after source changes: {after}"
    );
    assert!(
        !after.contains("replaced reply"),
        "pager should not show replaced content after source changes: {after}"
    );
}

#[test]
fn streaming_content_does_not_leak_into_history_emission() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "first question".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "first answer".to_string(),
    ));
    state.pending_history_flush = true;
    state.request_in_flight = true;
    state.pending_assistant = "streaming text that should not appear in history".to_string();

    let history = state.take_history_lines(80, 24);
    let history_text: String = history
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();

    assert!(
        history_text.contains("first answer"),
        "committed content must appear in history"
    );
    assert!(
        !history_text.contains("streaming text that should not appear"),
        "streaming pending_assistant content must not appear in history emission"
    );
}

// --- Stream-event lifecycle regressions (code-review 2026-07) ------------

fn budget_event(blocked: bool) -> StreamEvent {
    StreamEvent::Budget {
        session_id: "s".to_string(),
        outcome: BudgetOutcome::Exceeded,
        blocked,
        total_usd: 1.0,
        max_budget_usd: 2.0,
        pricing_known: true,
    }
}

#[test]
fn advisory_budget_warning_does_not_detach_turn_stream() {
    let mut state = normal_state("", 0);
    // A non-blocking (advisory) budget event: the turn keeps running on the
    // server, so it must NOT be treated as terminal (returning `true` would
    // detach the live turn stream and strand the spinner).
    let terminal = state.apply_stream_event(budget_event(false));
    assert!(
        !terminal,
        "advisory budget warning must not terminate the turn stream"
    );
}

#[test]
fn blocked_budget_is_terminal_but_keeps_persistent_overlay() {
    let mut state = normal_state("", 0);
    state.open_transcript_pager(80, 24);
    assert!(matches!(
        state.overlay,
        Some(OverlayState::TranscriptPager(_))
    ));
    let terminal = state.apply_stream_event(budget_event(true));
    assert!(terminal, "a blocked budget event ends the turn");
    assert!(
        matches!(state.overlay, Some(OverlayState::TranscriptPager(_))),
        "a blocked-budget turn end must not tear down an open transcript pager"
    );
}

#[test]
fn stream_error_keeps_persistent_overlay_open() {
    let mut state = normal_state("", 0);
    state.open_transcript_pager(80, 24);
    assert!(matches!(
        state.overlay,
        Some(OverlayState::TranscriptPager(_))
    ));
    let terminal = state.apply_stream_event(StreamEvent::Error {
        session_id: Some("s".to_string()),
        provider: Some(ProviderId::Anthropic),
        category: Some(StreamErrorCategory::RateLimit),
        message: "boom".to_string(),
        suggestion: None,
    });
    assert!(terminal, "an error event ends the turn");
    assert!(
        matches!(state.overlay, Some(OverlayState::TranscriptPager(_))),
        "a stream error must not close an open transcript pager mid-view"
    );
}

#[test]
fn mcp_trust_approval_does_not_detach_turn_stream() {
    let mut state = normal_state("", 0);
    let requested = state.apply_stream_event(StreamEvent::McpTrustApprovalRequested {
        request: orbcode_protocol::McpTrustApprovalRequest {
            request_id: "r".to_string(),
            session_id: "s".to_string(),
            server_id: "srv".to_string(),
            tool_name: "tool".to_string(),
        },
    });
    assert!(
        !requested,
        "an MCP trust request is not terminal; the turn continues after resolution"
    );
    let resolved = state.apply_stream_event(StreamEvent::McpTrustApprovalResolved {
        session_id: "s".to_string(),
        request_id: "r".to_string(),
        kind: orbcode_protocol::McpTrustResolutionKind::Trusted,
    });
    assert!(
        !resolved,
        "an MCP trust resolution must not detach the live turn stream"
    );
}

// --- Permission request queue + empty-id collapse (code-review 2026-07) ------

fn permission_request(request_id: &str, tool_use_id: &str) -> PermissionRequest {
    PermissionRequest {
        request_id: request_id.to_string(),
        session_id: "session".to_string(),
        tool_use_id: tool_use_id.to_string(),
        tool_name: "Bash".to_string(),
        tool_input: "{}".to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    }
}

#[test]
fn concurrent_permission_requests_are_queued_not_dropped() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(StreamEvent::PermissionRequested {
        request: permission_request("req-1", "tool-1"),
    });
    state.apply_stream_event(StreamEvent::PermissionRequested {
        request: permission_request("req-2", "tool-2"),
    });
    // The first request owns the overlay; the second is queued behind it.
    match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(active)) => {
            assert_eq!(active.request.request_id, "req-1");
            assert_eq!(active.queued.len(), 1);
        }
        _ => panic!("expected a permission overlay for req-1"),
    }

    // Resolving the first advances to the queued second request.
    state.apply_stream_event(StreamEvent::PermissionResolved {
        session_id: "session".to_string(),
        request_id: "req-1".to_string(),
        kind: orbcode_protocol::PermissionResolutionKind::Approved,
    });
    match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(active)) => {
            assert_eq!(
                active.request.request_id, "req-2",
                "the queued request must be shown after the first resolves"
            );
            assert!(active.queued.is_empty());
        }
        _ => panic!("expected a permission overlay for req-2"),
    }

    // Resolving the last closes the overlay.
    state.apply_stream_event(StreamEvent::PermissionResolved {
        session_id: "session".to_string(),
        request_id: "req-2".to_string(),
        kind: orbcode_protocol::PermissionResolutionKind::Approved,
    });
    assert!(state.overlay.is_none());
}

#[test]
fn queued_permission_request_resolved_out_of_order_is_dropped() {
    let mut state = normal_state("", 0);
    // Three concurrent requests: req-1 shown, req-2 and req-3 queued.
    for id in ["req-1", "req-2", "req-3"] {
        state.apply_stream_event(StreamEvent::PermissionRequested {
            request: permission_request(id, id),
        });
    }
    // req-2 (queued, not the shown one) resolves out-of-band (timeout / other
    // client). It must be dropped from the queue, not surfaced later.
    state.apply_stream_event(StreamEvent::PermissionResolved {
        session_id: "session".to_string(),
        request_id: "req-2".to_string(),
        kind: orbcode_protocol::PermissionResolutionKind::Interrupted,
    });
    match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(active)) => {
            assert_eq!(active.request.request_id, "req-1", "req-1 still shown");
            assert!(
                !active.queued.iter().any(|q| q.request_id == "req-2"),
                "an out-of-band-resolved queued request must be removed"
            );
            assert!(active.queued.iter().any(|q| q.request_id == "req-3"));
        }
        _ => panic!("expected req-1 overlay"),
    }

    // Resolving req-1 must now advance directly to req-3 (skipping the dropped
    // req-2), never re-showing the stale request.
    state.apply_stream_event(StreamEvent::PermissionResolved {
        session_id: "session".to_string(),
        request_id: "req-1".to_string(),
        kind: orbcode_protocol::PermissionResolutionKind::Approved,
    });
    match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(active)) => {
            assert_eq!(
                active.request.request_id, "req-3",
                "must advance to req-3, not the out-of-band-resolved req-2"
            );
        }
        _ => panic!("expected req-3 overlay"),
    }
}

#[test]
fn two_empty_tool_use_id_permission_requests_do_not_collapse() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(StreamEvent::PermissionRequested {
        request: permission_request("req-1", ""),
    });
    state.apply_stream_event(StreamEvent::PermissionRequested {
        request: permission_request("req-2", ""),
    });
    // Distinct request ids with empty tool_use_ids must not merge into one card.
    assert_eq!(
        state.live_tool_activities().len(),
        2,
        "two empty-tool_use_id requests must render as two distinct live cards"
    );
}

#[test]
fn pending_assistant_render_is_memoized_across_unchanged_frames() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (1..=40)
        .map(|index| format!("streamed answer line {index:02}\n"))
        .collect::<String>();

    // First render is a miss; repeated renders at the same width/source hit the
    // cache instead of re-parsing the full (growing) markdown every frame.
    let first = state.pending_assistant_live_lines(80);
    let repeat = state.pending_assistant_live_lines(80);
    assert_eq!(first.len(), repeat.len());
    let (hits, misses) = state.transcript_ui.emission.pending_render_cache_stats();
    assert!(misses >= 1, "the first render must populate the cache");
    assert!(
        hits >= 1,
        "an unchanged repeat frame must hit the cache, not re-render"
    );

    // A new delta invalidates the cache (new source hash) → a fresh miss.
    let misses_before = misses;
    state
        .pending_assistant
        .push_str("streamed answer line 41\n");
    let _ = state.pending_assistant_live_lines(80);
    let (_, misses_after) = state.transcript_ui.emission.pending_render_cache_stats();
    assert!(
        misses_after > misses_before,
        "a changed source must re-render (cache miss)"
    );
}
