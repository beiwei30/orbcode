pub(crate) const MAX_WEB_OUTPUT_CHARS: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TruncatedToolOutput {
    pub(crate) output: String,
    pub(crate) truncated: bool,
    pub(crate) original_chars: usize,
    pub(crate) omitted_chars: usize,
}

pub(crate) fn truncate_tool_output(output: String, max_chars: usize, note: &str) -> String {
    truncate_tool_output_with_metadata(output, max_chars, note).output
}

pub(crate) fn truncate_tool_output_with_metadata(
    output: String,
    max_chars: usize,
    note: &str,
) -> TruncatedToolOutput {
    let original_chars = output.chars().count();
    if output.chars().count() <= max_chars {
        return TruncatedToolOutput {
            output,
            truncated: false,
            original_chars,
            omitted_chars: 0,
        };
    }

    let reserve = note.chars().count() + 32;
    let preview_len = max_chars.saturating_sub(reserve).max(1);
    let preview = output.chars().take(preview_len).collect::<String>();
    let omitted = original_chars.saturating_sub(preview_len);

    TruncatedToolOutput {
        output: format!("{preview}\n\n[{note} Omitted {omitted} characters.]"),
        truncated: true,
        original_chars,
        omitted_chars: omitted,
    }
}
