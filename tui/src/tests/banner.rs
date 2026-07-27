use crate::tests::support::*;

#[test]
fn intro_banner_cell_renders_tip_with_spacing_and_bold_label() {
    let mut state = normal_state("", 0);
    state.session_id = "permissions".to_string();
    let banner_line_count = state.intro_banner_lines(90).len();
    let cell = state.intro_banner_cell(90);

    assert_eq!(cell.len(), banner_line_count + 4);
    assert!(cell[0].spans.is_empty());
    assert!(cell[banner_line_count + 1].spans.is_empty());
    assert!(cell[banner_line_count + 3].spans.is_empty());

    let tip_line = &cell[banner_line_count + 2];
    assert_eq!(
        plain_text_line(tip_line),
        "  Tip: Use /allowed-tools as an alias for /permissions."
    );
    assert_eq!(tip_line.spans[0].content.as_ref(), "  ");
    assert_eq!(tip_line.spans[1].content.as_ref(), "Tip:");
    assert!(
        tip_line.spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn intro_banner_transcript_preserves_blank_line_after_tip() {
    let mut state = normal_state("", 0);
    state.session_id = "permissions".to_string();

    let rendered = plain_text_lines(&state.transcript_lines(90));
    let tip_index = rendered
        .iter()
        .position(|line| line.starts_with("  Tip:"))
        .expect("banner tip should render");

    assert_eq!(
        rendered[tip_index],
        "  Tip: Use /allowed-tools as an alias for /permissions."
    );
    assert_eq!(rendered.get(tip_index + 1).map(String::as_str), Some(""));
}
