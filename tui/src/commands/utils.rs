use std::path::Path;

use orbcode_tools::ToolOutcome;

use crate::workspace_display::shorten_display_path;

pub(crate) fn short_session_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

pub(crate) fn split_first_word(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Advance past the whitespace char by its UTF-8 length: a multibyte
    // whitespace char (e.g. NBSP) would make `index + 1` land mid-char and panic
    // when slicing.
    match trimmed.char_indices().find(|(_, ch)| ch.is_whitespace()) {
        Some((index, ch)) => Some((
            &trimmed[..index],
            trimmed[index + ch.len_utf8()..].trim_start(),
        )),
        None => Some((trimmed, "")),
    }
}

pub(crate) fn clean_sandbox_exclude_pattern(input: &str) -> String {
    let mut pattern = input.trim().to_string();
    if matches!(pattern.as_bytes().first(), Some(b'"' | b'\'')) {
        pattern.remove(0);
    }
    if matches!(pattern.as_bytes().last(), Some(b'"' | b'\'')) {
        pattern.pop();
    }
    pattern
}

pub(crate) fn render_tool_note(outcome: &ToolOutcome) -> String {
    let mut rendered = format!("Tool: {}\n{}", outcome.name, outcome.summary);
    if !outcome.output.is_empty() {
        rendered.push('\n');
        rendered.push_str(&outcome.output);
    }
    rendered
}

pub(crate) fn parse_model_argument(argument: &str) -> Option<String> {
    let trimmed = argument.trim();
    if trimmed.eq_ignore_ascii_case("default") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn slash_command_display_path(path: &Path, cwd: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(cwd) {
        let rendered = relative.display().to_string();
        if !rendered.is_empty() {
            return format!("./{rendered}");
        }
    }

    shorten_display_path(path)
}
