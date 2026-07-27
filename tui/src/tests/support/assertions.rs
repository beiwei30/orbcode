use super::*;

pub fn plain_text_line(line: &StyledLine) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub fn plain_text_lines(lines: &[StyledLine]) -> Vec<String> {
    lines.iter().map(plain_text_line).collect()
}

pub fn platform_tool_line(text: &str) -> String {
    format!("{} {text}", black_circle_glyph())
}

pub fn committed_transcript_fixture_text(state: &TuiState, transcript_width: usize) -> String {
    let cells = render_committed_transcript_cells(
        &state.messages,
        &state.cwd,
        state.expanded_tool_details,
        transcript_width,
        &state.model_display_name,
    );
    plain_text_lines(&flatten_transcript_cells(&cells)).join("\n")
}

pub fn assert_fixture(actual: &str, fixture: &str) {
    let platform_fixture = fixture.replace("⏺", black_circle_glyph());
    assert_eq!(actual.trim_end(), platform_fixture.trim_end());
}
