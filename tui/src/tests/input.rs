use super::support::*;
use orbcode_app_server_client::AppClient;

#[test]
fn build_input_layout_tracks_wrapped_cursor_rows() {
    let layout = build_input_layout("abcdef", 5, 4);
    assert_eq!(layout.lines.len(), 2);
    assert_eq!(layout.cursor_row, 1);
    assert_eq!(layout.cursor_col, 1);
    assert_eq!(layout.lines[0].text, "abcd");
    assert_eq!(layout.lines[1].text, "ef");
}

#[test]
fn prompt_lines_clear_remaining_width_without_background() {
    let state = normal_state("评估一下 orbcode", 0);
    let input_view = build_input_view(&state.input, state.input_cursor, 20, MAX_INPUT_INNER_HEIGHT);
    let lines = state.prompt_lines(&input_view);

    assert!(lines[0].spans.iter().all(|span| span.style.bg.is_none()));
    let rendered = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(rendered.starts_with("❯ "));
    let rendered_width = rendered.chars().map(display_width).sum::<usize>();
    assert_eq!(rendered_width, 22);
}

#[test]
fn prompt_input_text_uses_regular_foreground() {
    let state = normal_state("hello", 0);
    let input_view = build_input_view(&state.input, state.input_cursor, 20, MAX_INPUT_INNER_HEIGHT);
    let lines = state.prompt_lines(&input_view);
    let input_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "hello")
        .expect("input text span should be present");

    assert_eq!(input_span.style.fg, None);
}

#[test]
fn prompt_lines_show_followup_controls_and_pending_messages() {
    let mut state = normal_state("new input", "new input".len());
    state.request_in_flight = true;
    state
        .steered_followups
        .push_back("user's another input".to_string());
    state
        .queued_followups
        .push_back("queued for later".to_string());
    let input_view = build_input_view(&state.input, state.input_cursor, 80, MAX_INPUT_INNER_HEIGHT);

    let prompt_rendered = plain_text_lines(&state.prompt_lines(&input_view)).join("\n");
    let followup_rendered =
        plain_text_lines(&state.followup_prompt_lines(input_view.width)).join("\n");

    assert!(!prompt_rendered.contains("Messages to be submitted after next tool call"));
    assert!(
        followup_rendered.contains(
            "• Messages to be submitted after next tool call (press esc to interrupt and send immediately)"
        ),
        "{followup_rendered}"
    );
    assert!(
        followup_rendered.contains("↳ user's another input"),
        "{followup_rendered}"
    );
    assert!(
        followup_rendered.contains("• Queued follow-up inputs"),
        "{followup_rendered}"
    );
    assert!(
        followup_rendered.contains("↳ queued for later"),
        "{followup_rendered}"
    );
    assert!(
        followup_rendered.contains("shift + ← edit last queued message"),
        "{followup_rendered}"
    );
}

#[tokio::test]
async fn jobs_commands_open_overlay_during_active_turn() {
    for command in ["/jobs", "/background"] {
        let home_dir = test_temp_path("jobs-active-turn-home");
        let cwd = test_temp_path("jobs-active-turn-workspace");
        tokio::fs::create_dir_all(&home_dir)
            .await
            .expect("create home");
        tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

        let app_server = AppServer::new(
            cwd,
            orbcode_app_server::AppConfigOverrides {
                home_dir: Some(home_dir),
                ..orbcode_app_server::AppConfigOverrides::default()
            },
        )
        .await
        .expect("create app server");
        let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
        let bootstrap = app_server.bootstrap(None).await.expect("bootstrap");
        let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
        app_server
            .app_server()
            .unwrap()
            .create_background_job(&state.session_id, "observe active turn")
            .await
            .expect("create background job");

        state.input = command.to_string();
        state.input_cursor = state.input.len();
        let (turn_tx, turn_rx) = mpsc::unbounded_channel();
        let mut turn_events = Some(turn_rx);
        let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

        state
            .handle_key(
                &app_server,
                crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut turn_events,
                &local_command_tx,
            )
            .await
            .expect("submit jobs command");
        drop(turn_tx);

        assert!(turn_events.is_some(), "{command}");
        assert!(state.steered_followups.is_empty(), "{command}");
        assert!(state.queued_followups.is_empty(), "{command}");
        assert!(state.input.is_empty(), "{command}");
        assert!(
            matches!(state.overlay, Some(OverlayState::BackgroundJobs(_))),
            "{command}"
        );
    }
}

#[test]
fn pending_followups_are_laid_out_above_input_box() {
    let mut state = normal_state("", 0);
    state
        .queued_followups
        .push_back("当前项目有多少行代码？".to_string());
    let area = Rect::new(0, 0, 80, 20);
    let input_view = build_input_view(&state.input, state.input_cursor, 77, MAX_INPUT_INNER_HEIGHT);

    let layout = state.main_layout_regions(area, &input_view, 0);

    assert_eq!(layout[2].height, state.prompt_followup_line_count() as u16);
    assert_eq!(layout[3].y, layout[2].y.saturating_add(layout[2].height));
    assert_eq!(layout[4].y, layout[3].y.saturating_add(1));
}

#[test]
fn prompt_cursor_is_offset_below_pending_followups() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state
        .steered_followups
        .push_back("user's another input".to_string());
    let mut fixture = RenderMetricsFixture::new(80, 20);

    fixture.draw(&mut state);

    let cursor = fixture.cursor_position();
    assert_eq!(cursor.x, state.input_area.x.saturating_add(2));
    assert_eq!(cursor.y, state.input_area.y);
}

#[test]
fn prompt_input_lines_fit_without_paragraph_wrapping() {
    let input = "Then try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\none line\ntwo line\nyet another line";
    let state = normal_state(input, input.len());
    let area_width = 80;
    let input_view = build_input_view(&state.input, state.input_cursor, area_width - 3, usize::MAX);
    let lines = state.prompt_lines(&input_view);

    for line in lines {
        let width = line
            .spans
            .iter()
            .flat_map(|span| span.content.chars())
            .map(display_width)
            .sum::<usize>();
        assert!(width <= area_width, "{width}");
    }
}

#[test]
fn input_cursor_for_column_clamps_to_line_end() {
    let layout = build_input_layout("hello\nworld", 0, 16);
    assert_eq!(input_cursor_for_column(&layout.lines[1], 0), 6);
    assert_eq!(input_cursor_for_column(&layout.lines[1], 2), 8);
    assert_eq!(input_cursor_for_column(&layout.lines[1], 99), 11);
}

#[test]
fn build_input_layout_uses_display_width_for_wide_chars() {
    let layout = build_input_layout("你好a", '你'.len_utf8(), 8);
    assert_eq!(layout.cursor_row, 0);
    assert_eq!(layout.cursor_col, 2);
}

#[test]
fn input_cursor_for_column_tracks_wide_char_boundaries() {
    let layout = build_input_layout("你好a", 0, 8);
    assert_eq!(
        input_cursor_for_column(&layout.lines[0], 2),
        '你'.len_utf8()
    );
    assert_eq!(input_cursor_for_column(&layout.lines[0], 4), "你好".len());
}

#[test]
fn paste_normalizes_carriage_return_newlines() {
    let mut state = normal_state("", 0);
    state.editor_mode = EditorMode::Insert;

    state.insert_paste_text("one\r\ntwo\rthree");

    assert_eq!(state.input, "one\ntwo\nthree");
    assert_eq!(state.input_cursor, "one\ntwo\nthree".len());
    assert_eq!(state.vim_state.inserted_text, "one\ntwo\nthree");
}

#[test]
fn pasted_prompt_view_keeps_trailing_comment_line_visible() {
    let prompt = "Use Write, Edit, and Read to verify ambiguous edit handling.\n\nCreate ambiguous.txt with exactly:\ntarget\nmiddle\ntarget\n\nThen try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\n// ignore this line\n";
    let state = normal_state(prompt, prompt.len());
    let input_view = build_input_view(prompt, prompt.len(), 77, MAX_INPUT_INNER_HEIGHT);
    let rendered = state
        .prompt_lines(&input_view)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(rendered.contains("// ignore this line"), "{rendered}");
}

#[test]
fn input_view_keeps_following_lines_visible_near_cursor() {
    let input = "one\ntwo\nthree\nfour\nfive\nsix\nseven";
    let cursor = input.find("five").expect("cursor line");
    let view = build_input_view(input, cursor, 80, 5);

    assert_eq!(
        view.lines,
        vec![
            "three".to_string(),
            "four".to_string(),
            "five".to_string(),
            "six".to_string(),
            "seven".to_string()
        ]
    );
    assert_eq!(view.cursor_row, 2);
}

#[test]
fn input_view_shows_line_after_blank_cursor_line() {
    let input = "before\n\n// ignore this line\n\nactual final line";
    let cursor = input.find("actual final line").expect("final line") - 1;
    let view = build_input_view(input, cursor, 80, 5);

    assert!(view.lines.iter().any(|line| line == "actual final line"));
}

#[test]
fn input_view_anchors_to_tail_when_cursor_is_near_end() {
    let input = "After that, read ambiguous.txt and report whether it stayed unchanged.\n\none line\ntwo line\nyet another line";
    let cursor = input.find("two line").expect("two line cursor");
    let view = build_input_view(input, cursor, 80, 4);

    assert_eq!(
        view.lines,
        vec![
            "".to_string(),
            "one line".to_string(),
            "two line".to_string(),
            "yet another line".to_string()
        ]
    );
}

#[test]
fn pasted_prompt_view_anchors_to_tail_from_last_instruction_line() {
    let input = "Use Write, Edit, and Read to verify ambiguous edit handling.\n\nCreate ambiguous.txt with exactly:\ntarget\nmiddle\ntarget\n\nThen try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\none line\ntwo line\nyet another line";
    let cursor = input
        .find("After that, read ambiguous.txt")
        .expect("last instruction line");
    let view = build_input_view(input, cursor, 77, 4);

    assert_eq!(
        view.lines,
        vec![
            "".to_string(),
            "one line".to_string(),
            "two line".to_string(),
            "yet another line".to_string()
        ]
    );
}

#[test]
fn pasted_prompt_tail_pin_shows_tail_when_cursor_is_near_start() {
    let input = "Use Write, Edit, and Read to verify ambiguous edit handling.\n\nCreate ambiguous.txt with exactly:\ntarget\nmiddle\ntarget\n\nThen try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\none line\ntwo line\nyet another line";
    let view = build_input_view_with_tail_pin(input, 0, 77, 10, true);

    assert!(
        view.lines.iter().any(|line| line == "yet another line"),
        "{:?}",
        view.lines
    );
    assert!(
        !view.lines.iter().any(|line| line.starts_with("Use Write")),
        "{:?}",
        view.lines
    );
}

#[test]
fn multiline_paste_tail_pins_until_user_moves_cursor() {
    let mut state = normal_state("", 0);
    state.insert_paste_text("one\ntwo\nthree");

    assert!(state.input_tail_pinned);

    state.move_cursor_left();

    assert!(!state.input_tail_pinned);
}

#[test]
fn appended_multiline_input_tail_pins_without_paste_event() {
    let input = "Use Write, Edit, and Read to verify ambiguous edit handling.\n\nCreate ambiguous.txt with exactly:\ntarget\nmiddle\ntarget\n\nThen try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\none line\ntwo line\nyet another line";
    let mut state = normal_state("", 0);
    for ch in input.chars() {
        state.insert_char(ch);
    }

    assert!(state.input_tail_pinned);
    let view = build_input_view_with_tail_pin(
        &state.input,
        state.input_cursor,
        77,
        10,
        state.input_tail_pinned,
    );
    assert!(
        view.lines.iter().any(|line| line == "yet another line"),
        "{:?}",
        view.lines
    );
    assert!(
        !view.lines.iter().any(|line| line.starts_with("Use Write")),
        "{:?}",
        view.lines
    );
}

#[test]
fn input_view_keeps_trailing_empty_cursor_line_visible() {
    let input = "one\ntwo\nthree\nfour\n";
    let view = build_input_view(input, input.len(), 80, 3);

    assert_eq!(
        view.lines,
        vec!["three".to_string(), "four".to_string(), "".to_string()]
    );
    assert_eq!(view.cursor_row, 2);
}

#[test]
fn pasted_tail_view_keeps_trailing_newline_cursor_inside_view() {
    let input = "one\ntwo\nthree\n";
    let view = build_input_view_with_tail_pin(input, input.len(), 80, 3, true);

    assert_eq!(
        view.lines,
        vec!["two".to_string(), "three".to_string(), "".to_string()]
    );
    assert!(view.cursor_row < view.lines.len(), "{:?}", view.lines);
}

#[test]
fn long_pasted_prompt_layout_keeps_input_view_inside_region() {
    let block = "Use Write, Edit, and Read to verify ambiguous edit handling.\n\nCreate ambiguous.txt with exactly:\ntarget\nmiddle\ntarget\n\nThen try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\none line\ntwo line\nyet another line";
    let input = [block, block, block, block].join("\n\n");
    let mut state = normal_state(&input, input.len());
    state.input_tail_pinned = true;
    let area = Rect::new(0, 0, 80, 30);
    let visible_height = max_input_inner_height(area.height, 0);
    let input_view = build_input_view_with_tail_pin(&input, input.len(), 77, visible_height, true);

    let layout = state.main_layout_regions(area, &input_view, 0);

    assert!(layout[4].height as usize >= input_view.lines.len());
    assert!(input_view.cursor_row < layout[4].height as usize);
    assert_eq!(
        input_view.lines.last().map(String::as_str),
        Some("yet another line")
    );
}

#[test]
fn input_inner_height_uses_available_terminal_space() {
    assert_eq!(max_input_inner_height(12, 0), 8);
    assert!(max_input_inner_height(12, 0) > MAX_INPUT_INNER_HEIGHT);
}

#[test]
fn desired_viewport_height_includes_all_prompt_lines() {
    let input = "one\ntwo\nthree\nfour\nfive\nsix\nseven";
    let mut state = normal_state(input, input.len());

    let height = state.desired_viewport_height(80, 100);

    assert!(height >= input.lines().count() as u16 + 3, "{height}");
}

#[test]
fn desired_viewport_height_includes_slash_suggestions() {
    let mut state = normal_state("/", 1);
    let suggestion_height = state.slash_command_suggestion_lines(80).len() as u16;
    let mut without_suggestions = normal_state("", 0);

    let height = state.desired_viewport_height(80, 100);
    let height_without_suggestions = without_suggestions.desired_viewport_height(80, 100);

    assert_eq!(suggestion_height, SLASH_COMMAND_VISIBLE_ROWS as u16);
    assert_eq!(
        height,
        height_without_suggestions.saturating_add(suggestion_height)
    );
}

#[test]
fn input_view_uses_available_height_before_layout_clips() {
    let input = "After that, read ambiguous.txt and report whether it stayed unchanged.\n\n// ignore this line\n\nyet another line";
    let visible_height = max_input_inner_height(7, 0);
    let view = build_input_view(input, input.len(), 77, visible_height);

    assert_eq!(visible_height, 3);
    assert!(view.lines.iter().any(|line| line == "yet another line"));
    assert!(view.cursor_row < visible_height);
}

#[test]
fn desired_viewport_height_grows_for_multiline_prompt_tail() {
    let prompt = "Use Write, Edit, and Read to verify ambiguous edit handling.\n\nCreate ambiguous.txt with exactly:\ntarget\nmiddle\ntarget\n\nThen try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\none line\ntwo line\nyet another line\n";
    let mut state = normal_state(prompt, prompt.len());

    let height = state.desired_viewport_height(80, 100);

    assert!(height >= 8, "{height}");
}

#[test]
fn prompt_submission_preserves_trailing_comment_line() {
    let prompt = "Use Write, Edit, and Read to verify ambiguous edit handling.\n\nCreate ambiguous.txt with exactly:\ntarget\nmiddle\ntarget\n\nThen try to edit ambiguous.txt by replacing old_string `target` with `changed` without replace_all.\nAfter that, read ambiguous.txt and report whether it stayed unchanged.\n\n// ignore this line\n";

    assert_eq!(
        prompt_input_submission_line(prompt).as_deref(),
        Some(prompt)
    );
}

#[test]
fn prompt_submission_trims_only_slash_command_boundaries() {
    assert_eq!(
        prompt_input_submission_line("  /help  "),
        Some("/help".to_string())
    );
    assert_eq!(prompt_input_submission_line(" \n\t "), None);
}
