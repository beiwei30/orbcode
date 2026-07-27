use std::path::PathBuf;

use ratatui::prelude::*;

use crate::line_cache::LinesCache;
use crate::render::text_utils::{StyledLine, pad_or_truncate, truncate_chars};
use crate::slash_commands::{
    SuggestionEntry, slash_command_column_width, slash_command_scrollbar_active,
};
use crate::state::TuiState;
use crate::tui_theme::empty_transcript_placeholder_style;

pub(crate) type SlashSuggestionLinesCache = LinesCache<SlashSuggestionLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashSuggestionLinesCacheKey {
    cwd: PathBuf,
    input: String,
    input_cursor: usize,
    selected: usize,
    width: usize,
    mcp_revision: u64,
}

impl TuiState {
    pub(crate) fn idle_transient_panel_visible_for_width(&self, width: usize) -> bool {
        // Keep this predicate self-contained; callers may already check some
        // idle state, but this method is also the panel visibility boundary.
        if self.overlay.is_some()
            || self.request_in_flight
            || self.rendering_paused()
            || self.pending_history_flush
            || !self.transcript_ui.emission.pending_lines.is_empty()
        {
            return false;
        }
        if !self.request_status_lines_for_width(width).is_empty() {
            return true;
        }
        self.add_dir_completion_view().is_some()
            || self.slash_argument_completion_view().is_some()
            || self.slash_command_view().is_some()
    }

    pub(crate) fn slash_command_suggestion_lines(&self, width: usize) -> Vec<StyledLine> {
        if let Some(view) = self.add_dir_completion_view() {
            return self.add_dir_completion_lines(&view, width);
        }
        if let Some(view) = self.slash_argument_completion_view() {
            return self.slash_argument_completion_lines(&view, width);
        }

        let Some(view) = self.slash_command_view() else {
            return Vec::new();
        };

        let command_width = slash_command_column_width(&view.entries, width);
        let muted = empty_transcript_placeholder_style();
        view.entries
            .iter()
            .skip(view.start)
            .take(view.visible_count)
            .enumerate()
            .map(|(index, entry)| {
                let absolute_index = view.start + index;
                let scrollbar_style = if slash_command_scrollbar_active(index, &view) {
                    Style::default()
                } else {
                    muted
                };
                match entry {
                    SuggestionEntry::GroupHeader(label) => {
                        let header_text = format!("── {label} ──");
                        let header_width = width.saturating_sub(4);
                        let header = truncate_chars(&header_text, header_width);
                        Line::from(vec![
                            Span::styled("│", scrollbar_style),
                            Span::styled("  ", muted),
                            Span::styled("  ", muted),
                            Span::styled(header, muted),
                        ])
                    }
                    SuggestionEntry::Command(command) => {
                        let selected = absolute_index == view.selected;
                        let marker = if selected { "› " } else { "  " };
                        let command_text = format!("/{}", command.name);
                        let padded_command = pad_or_truncate(&command_text, command_width.max(1));
                        let description_width =
                            width.saturating_sub(command_width).saturating_sub(8).max(1);
                        let description = truncate_chars(command.description, description_width);
                        let command_style = if selected { Style::default() } else { muted };
                        let description_style = if selected { Style::default() } else { muted };
                        Line::from(vec![
                            Span::styled("│", scrollbar_style),
                            Span::styled("  ", muted),
                            Span::styled(marker.to_string(), command_style),
                            Span::styled(padded_command, command_style),
                            Span::styled("  ", muted),
                            Span::styled(description, description_style),
                        ])
                    }
                }
            })
            .collect()
    }

    pub(crate) fn cached_slash_command_suggestion_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = SlashSuggestionLinesCacheKey {
            cwd: self.cwd.clone(),
            input: self.input.clone(),
            input_cursor: self.input_cursor,
            selected: self.slash_command_selected,
            width,
            mcp_revision: self.mcp_slash_suggestion_revision,
        };
        let mut lines_cache = std::mem::take(&mut self.slash_suggestion_lines_cache);
        lines_cache.refresh(key, || self.slash_command_suggestion_lines(width));
        self.slash_suggestion_lines_cache = lines_cache;
        &self.slash_suggestion_lines_cache.lines
    }
}
