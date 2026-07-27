use crate::tests::support::*;

#[test]
fn normal_mode_dd_deletes_current_line() {
    let mut state = normal_state("one\ntwo\nthree", "one\n".len());

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "one\nthree");
    assert_eq!(state.input_cursor, "one\n".len());
}

#[test]
fn normal_mode_cw_deletes_to_word_end_and_enters_insert() {
    let mut state = normal_state("hello world", 0);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('w'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, " world");
    assert_eq!(state.input_cursor, 0);
    assert_eq!(state.editor_mode, EditorMode::Insert);
}

#[test]
fn normal_mode_dw_deletes_to_next_word_start() {
    let mut state = normal_state("hello world", 0);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('w'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "world");
    assert_eq!(state.input_cursor, 0);
    assert_eq!(state.editor_mode, EditorMode::Normal);
}

#[test]
fn normal_mode_diw_deletes_inner_word() {
    let mut state = normal_state("say hello world", "say h".len());

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('w'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "say  world");
    assert_eq!(state.input_cursor, "say ".len());
}

#[test]
fn normal_mode_ci_quote_removes_inner_text_and_enters_insert() {
    let mut state = normal_state("\"hello\"", 1);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('"'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "\"\"");
    assert_eq!(state.input_cursor, 1);
    assert_eq!(state.editor_mode, EditorMode::Insert);
}

#[test]
fn normal_mode_yy_and_p_paste_linewise() {
    let mut state = normal_state("one\ntwo", 0);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "one\none\ntwo");
    assert_eq!(state.input_cursor, "one\n".len());
}

#[test]
fn normal_mode_x_and_p_paste_characterwise() {
    let mut state = normal_state("abc", 1);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "ac");

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "acb");
}

#[test]
fn normal_mode_replace_dot_and_undo_work() {
    let mut state = normal_state("abc", 0);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('X'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(state.input, "Xbc");

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('.'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "XXc");

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "Xbc");
}

#[test]
fn normal_mode_o_and_shift_o_place_cursor_on_blank_line() {
    let mut below = normal_state("foo\nbar", 0);
    below
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(below.input, "foo\n\nbar");
    assert_eq!(below.input_cursor, "foo\n".len());
    assert_eq!(below.editor_mode, EditorMode::Insert);

    let mut above = normal_state("foo\nbar", "foo\n".len());
    above
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('O'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(above.input, "foo\n\nbar");
    assert_eq!(above.input_cursor, "foo\n".len());
    assert_eq!(above.editor_mode, EditorMode::Insert);
}

#[test]
fn normal_mode_join_and_indent_work() {
    let mut state = normal_state("foo\n  bar\nbaz", 0);
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('J'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(state.input, "foo bar\nbaz");

    let mut indent = normal_state("foo\nbar", 0);
    indent
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('>'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    indent
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('>'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(indent.input, "  foo\nbar");

    indent
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('<'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    indent
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('<'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(indent.input, "foo\nbar");
}

#[test]
fn normal_mode_counts_apply_to_line_and_word_operators() {
    let mut linewise = normal_state("one\ntwo\nthree", 0);
    linewise
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    linewise
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    linewise
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(linewise.input, "three");

    let mut wordwise = normal_state("one two three four", 0);
    wordwise
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    wordwise
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    wordwise
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('w'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(wordwise.input, "three four");
}

#[test]
fn normal_mode_d_c_y_shorthand_commands_work() {
    let mut delete_to_end = normal_state("hello world", "hello ".len());
    delete_to_end
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('D'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(delete_to_end.input, "hello ");

    let mut change_to_end = normal_state("hello world", "hello ".len());
    change_to_end
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(change_to_end.input, "hello ");
    assert_eq!(change_to_end.editor_mode, EditorMode::Insert);

    let mut yank_line = normal_state("one\ntwo", 0);
    yank_line
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('Y'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    yank_line
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(yank_line.input, "one\none\ntwo");
}

#[test]
fn normal_mode_g_line_navigation_and_operator_g_work() {
    let mut state = normal_state("one\ntwo\nthree\nfour", 0);
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('G'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(state.input_cursor, "one\ntwo\nthree\n".len());

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('G'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(state.input_cursor, "one\n".len());

    let mut delete_to_first = normal_state("one\ntwo\nthree", "one\ntwo\n".len());
    delete_to_first
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    delete_to_first
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    delete_to_first
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(delete_to_first.input, "");

    let mut delete_to_last = normal_state("one\ntwo\nthree", 0);
    delete_to_last
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    delete_to_last
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('G'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(delete_to_last.input, "");
}

#[test]
fn normal_mode_d_percent_deletes_balanced_block_both_directions() {
    let mut forward = normal_state("foo(bar) baz", "foo".len());
    forward
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    forward
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('%'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(forward.input, "foo baz");

    let mut backward = normal_state("foo(bar) baz", "foo(bar".len());
    backward
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    backward
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('%'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(backward.input, "foo baz");
}

#[test]
fn normal_mode_insert_escape_and_dot_repeat_replays_inserted_text() {
    let mut state = normal_state("abc", 1);
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state.insert_text("zz");
    let _ = state.handle_escape_key(false);
    assert_eq!(state.editor_mode, EditorMode::Normal);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('.'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "azzzzbc");
    assert_eq!(state.editor_mode, EditorMode::Normal);
}

#[test]
fn normal_mode_s_and_shift_s_change_text_and_enter_insert() {
    let mut substitute = normal_state("hello", 1);
    substitute
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(substitute.input, "hllo");
    assert_eq!(substitute.input_cursor, 1);
    assert_eq!(substitute.editor_mode, EditorMode::Insert);

    let mut line_substitute = normal_state("one\ntwo", "one\n".len());
    line_substitute
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('S'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert_eq!(line_substitute.input, "one\n");
    assert_eq!(line_substitute.input_cursor, "one\n".len());
    assert_eq!(line_substitute.editor_mode, EditorMode::Insert);
}

#[test]
fn normal_mode_dot_repeats_change_with_inserted_text() {
    let mut state = normal_state("foo bar", 0);
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('w'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state.insert_text("zip");
    let _ = state.handle_escape_key(false);
    assert_eq!(state.input, "zip bar");

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('w'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('.'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "zip zip");
    assert_eq!(state.editor_mode, EditorMode::Normal);
}

#[test]
fn normal_mode_dot_repeats_open_line_with_inserted_text() {
    let mut state = normal_state("foo", 0);
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state.insert_text("bar");
    let _ = state.handle_escape_key(false);
    assert_eq!(state.input, "foo\nbar");

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('.'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "foo\nbar\nbar");
    assert_eq!(state.editor_mode, EditorMode::Normal);
}

#[test]
fn normal_mode_dot_repeats_paste() {
    let mut state = normal_state("abc", 1);
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "acb");

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('.'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "acbb");
}
