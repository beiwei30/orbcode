//! Table holdback detection for streaming assistant markdown.
//!
//! While an assistant message streams and incremental commit is active (see
//! `history_cell::state::prepare_pending_assistant_history_emission`), Orb Code
//! commits all but a live tail of rendered lines into terminal scrollback. That
//! is correct for prose, but wrong for markdown tables: table rendering is
//! non-incremental — a later row can widen a column and reshape every prior
//! row. Once a table row is committed to scrollback it can no longer reflow, so
//! a table taller than the live tail would tear.
//!
//! This module locates the byte offset in the streaming source at which the
//! first in-progress table begins, so the emission code can withhold everything
//! from that point onward until the stream finalizes (at which point the whole
//! table is rendered once, canonically). Detection mirrors the table and
//! fenced-code-block rules used by `render::markdown::render_markdown_body_lines`
//! so "a table starts here" matches what the renderer actually produces.
//!
//! Ported in spirit from codex `tui/src/streaming/table_holdback.rs`, adapted to
//! Orb Code's whole-source-per-frame rendering model: the scan is stateless over
//! the full source rather than an incremental byte-cursor scanner.

use crate::render::markdown::{is_markdown_table_separator, split_markdown_table_row};

/// Returns the byte offset in `source` at which streamed content should be held
/// back in the mutable tail, or `None` when nothing needs holding back.
///
/// The offset points at the start of the *trailing open* table — a confirmed
/// table (a table-row line immediately followed by a delimiter line, outside a
/// fenced code block) whose block runs to the end of the source. A table that
/// has already been closed by a blank line, a code fence, or a line that is not
/// a table row can no longer reflow, so it is safe to commit and is NOT held
/// back; only the currently-open trailing table (and anything after it, which is
/// empty by definition since the table is trailing) is withheld. If no table is
/// open but the last non-blank content line looks like a table header (a pending
/// header whose delimiter has not streamed in yet), the offset of that header
/// line is returned so a header is never committed to scrollback one frame
/// before its delimiter arrives.
///
/// Lines inside ```` ``` ```` fenced code blocks are ignored, matching
/// `render_markdown_body_lines`, so pipe characters in code do not trip the
/// detector.
pub(crate) fn table_holdback_source_start(source: &str) -> Option<usize> {
    // Collect (byte offset, line-without-trailing-newline) for every line while
    // preserving byte offsets, which `str::lines` discards.
    let mut lines: Vec<(usize, &str, bool)> = Vec::new();
    let mut offset = 0usize;
    for chunk in source.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        lines.push((offset, line, chunk.ends_with('\n')));
        offset += chunk.len();
    }

    // `open_table` is the header offset of the table block we are currently
    // inside; it is cleared the moment that block ends (blank line, fence, or a
    // non-row line), mirroring where `render_markdown_table_block` stops
    // consuming rows. `pending_header` tracks a trailing lone header whose
    // delimiter has not yet streamed in.
    let mut open_table: Option<usize> = None;
    let mut pending_header: Option<usize> = None;
    let mut in_code_block = false;
    let last_index = lines.len().saturating_sub(1);

    for index in 0..lines.len() {
        let (line_offset, line, line_terminated) = lines[index];
        let trimmed = line.trim();
        let is_last_line = index == last_index;

        if trimmed.starts_with("```") {
            // A fence cannot appear inside a table; it closes any open block.
            in_code_block = !in_code_block;
            open_table = None;
            pending_header = None;
            continue;
        }
        if in_code_block {
            continue;
        }
        // A terminated blank line definitively ends a table/header. An
        // unterminated whitespace-only tail can still receive separator or row
        // characters in the next delta, so it remains ambiguous.
        if trimmed.is_empty() {
            if is_last_line && !line_terminated {
                continue;
            }
            open_table = None;
            pending_header = None;
            continue;
        }

        let is_row_candidate = split_markdown_table_row(line).is_some();

        if open_table.is_some() {
            if is_row_candidate {
                // Separator or body row: still inside the table block.
                continue;
            }
            if is_last_line {
                // Conservative (Medium #2): a non-row TRAILING line directly after
                // an open table may still be a body row being typed across deltas
                // — e.g. `new` before ` | value` streams in, with NO leading pipe
                // yet. Keep the table held so its rows are not committed and then
                // torn when the completed row reshapes the columns. A following
                // line (or a blank) settles it on a later frame.
                continue;
            }
            // A settled (non-trailing) non-row line ends the table; re-evaluate
            // this line below as a potential fresh header.
            open_table = None;
        }

        if is_row_candidate
            && lines
                .get(index + 1)
                .is_some_and(|(_, next, _)| is_markdown_table_separator(next))
        {
            open_table = Some(line_offset);
            pending_header = None;
        } else if is_row_candidate {
            pending_header = Some(line_offset);
        } else if pending_header.is_some()
            && is_last_line
            && !line_terminated
            && line_could_extend_table_header(line)
        {
            // Conservative (Medium #2): a header candidate followed by a TRAILING
            // line that could still complete into its separator — a bare `|`, or a
            // partial separator such as `---` / `:--` with NO leading pipe yet —
            // keeps the header held. Otherwise the header commits as plain text one
            // delta before the separator arrives and reclassifies it as a table,
            // and native scrollback cannot un-emit it. pending_header is left as-is.
        } else {
            pending_header = None;
        }
    }

    open_table.or(pending_header)
}

/// Whether a trailing line could still complete a markdown table header into a
/// confirmed table on a later streaming delta. True for a pipe fragment (`|`,
/// `| a`) and for a separator being typed with or without a leading pipe (`---`,
/// `:--`, `--- |`). Ordinary prose returns false so a genuine non-table line
/// still releases the pending header. Mirrors the renderer accepting no-leading-`|`
/// rows/separators (`render::markdown`).
fn line_could_extend_table_header(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('|') {
        return true;
    }
    // A (possibly partial) separator: only dashes/colons/pipes/spaces. It need
    // not contain a dash yet because a later delta can append one.
    trimmed
        .chars()
        .all(|ch| matches!(ch, '-' | ':' | '|' | ' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_for_plain_prose() {
        assert_eq!(
            table_holdback_source_start("hello world\nsecond line\n"),
            None
        );
    }

    #[test]
    fn confirms_table_at_header_offset() {
        let source = "intro paragraph\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n";
        let start = table_holdback_source_start(source).expect("table detected");
        assert_eq!(&source[start..start + 7], "| a | b");
    }

    #[test]
    fn pending_header_before_delimiter_arrives() {
        // Header has streamed but the delimiter line has not yet.
        let source = "intro\n\n| a | b |\n";
        let start = table_holdback_source_start(source).expect("pending header");
        assert_eq!(&source[start..start + 3], "| a");
    }

    #[test]
    fn ignores_pipes_inside_code_fence() {
        let source = "```\n| a | b |\n| --- | --- |\n```\ndone\n";
        assert_eq!(table_holdback_source_start(source), None);
    }

    #[test]
    fn holds_from_trailing_open_table_only() {
        // A completed leading table (closed by a blank line) is safe to commit;
        // only the still-open trailing table is held back.
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |\n\ntext\n\n| c | d |\n| --- | --- |\n";
        let start = table_holdback_source_start(source).expect("trailing table");
        assert_eq!(&source[start..start + 3], "| c");
    }

    #[test]
    fn releases_completed_table_followed_by_prose() {
        // Regression (High #2): a confirmed table that has ended (blank line +
        // trailing prose) must NOT be held back — otherwise the table and every
        // following prose line stay stuck in the mutable live tail forever.
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |\n\n\
             the table above is complete and this is a long\n\
             paragraph of prose that continues well past the\n\
             live tail and must be committable to scrollback\n";
        assert_eq!(table_holdback_source_start(source), None);
    }

    #[test]
    fn trailing_line_directly_after_table_stays_held_until_settled() {
        // No blank line between the last row and the following text: while that
        // text is the trailing (actively-streamed) line it is AMBIGUOUS — it may
        // still be a body row being typed (`new` before ` | value`). Hold the
        // table conservatively (Medium #2) rather than committing rows that a
        // completed row could reshape.
        let held = "| a | b |\n| --- | --- |\n| 1 | 2 |\nplain prose right after";
        assert_eq!(table_holdback_source_start(held), Some(0));

        // Once a following line settles it (the text is no longer the trailing
        // line and did not become a row), the table is definitively closed and
        // released.
        let released = "| a | b |\n| --- | --- |\n| 1 | 2 |\nplain prose\nand more prose\n";
        assert_eq!(table_holdback_source_start(released), None);
    }

    #[test]
    fn forming_separator_without_leading_pipe_keeps_header_held() {
        // Regression (Medium #2): a header row with no leading pipe, then a
        // partial separator (`---`, no pipe yet) as the trailing line. Must hold
        // from the header — the next delta completes `--- | ---` and reclassifies
        // it as a table; committing the header as plain first would tear.
        let source = "intro\n\na | b\n---";
        let start = table_holdback_source_start(source).expect("forming table held");
        assert_eq!(&source[start..start + 5], "a | b");
    }

    #[test]
    fn open_table_trailing_forming_row_without_leading_pipe_stays_held() {
        // Regression (Medium #2): an open table whose trailing line is a bare
        // token with no pipe yet (`new` before `new | value`). Must keep the
        // table held so its rows aren't committed and then torn by the completed
        // row.
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |\nnew";
        assert_eq!(table_holdback_source_start(source), Some(0));
    }

    #[test]
    fn blank_line_does_not_clear_pending_header() {
        let source = "| a | b |\n";
        assert_eq!(table_holdback_source_start(source), Some(0));
    }

    #[test]
    fn partial_separator_fragment_keeps_holding_from_header() {
        // Regression (Medium #2): the header has streamed and the separator has
        // only partially arrived (a bare `|`). The header must stay held back —
        // clearing it here would commit the header one delta before its
        // separator completes, tearing the table.
        let source = "intro\n\n| a | b |\n|";
        let start = table_holdback_source_start(source).expect("still holding the header");
        assert_eq!(&source[start..start + 3], "| a");
    }

    #[test]
    fn colon_only_unterminated_separator_fragment_keeps_header_held() {
        let partial = "a | b\n:";
        assert_eq!(table_holdback_source_start(partial), Some(0));

        let completed = "a | b\n:- | :-";
        assert_eq!(table_holdback_source_start(completed), Some(0));
    }

    #[test]
    fn unterminated_whitespace_can_still_become_a_separator() {
        let partial = "a | b\n   ";
        assert_eq!(table_holdback_source_start(partial), Some(0));

        let completed = "a | b\n   :- | :-";
        assert_eq!(table_holdback_source_start(completed), Some(0));
    }

    #[test]
    fn terminated_blank_line_releases_pending_header() {
        assert_eq!(table_holdback_source_start("a | b\n   \n"), None);
    }

    #[test]
    fn bare_pipe_fragment_after_committed_rows_keeps_table_open() {
        // A confirmed table with a body row, then a bare `|` fragment for the
        // next row. The block must stay open (held from the header), not close.
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |\n|";
        assert_eq!(table_holdback_source_start(source), Some(0));
    }

    #[test]
    fn non_table_line_clears_pending_header() {
        // A pipe-bearing line followed by ordinary prose is not held back.
        let source = "| a | b |\nordinary prose without confirmation\n";
        assert_eq!(table_holdback_source_start(source), None);
    }
}
