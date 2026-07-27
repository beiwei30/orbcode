use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use super::completion::{
    AddDirCompletionView, SlashArgumentCompletionView, add_dir_completion_scrollbar_active,
};
use super::input_layout::{InputLineLayout, InputView};
use crate::prompt_state::InputSelectionState;
use crate::render::text_utils::{StyledLine, display_width, pad_or_truncate, truncate_chars};
use crate::slash_commands::suggestion_scrollbar_active;
use crate::state::TuiState;
use crate::tui_theme::{
    empty_transcript_placeholder_style, inactive_style, prompt_input_style, subtle_style,
};

fn selection_style(base: Style) -> Style {
    base.remove_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED)
}

fn input_line_spans(
    line: &InputLineLayout,
    selection: Option<&InputSelectionState>,
) -> Vec<Span<'static>> {
    let base_style = prompt_input_style();
    let Some(selection) = selection else {
        return vec![Span::styled(line.text.clone(), base_style)];
    };
    if selection.is_collapsed() {
        return vec![Span::styled(line.text.clone(), base_style)];
    }

    let (selection_start, selection_end) = selection.normalized_range();
    line.text
        .char_indices()
        .map(|(offset, ch)| {
            let char_start = line.start + offset;
            let char_end = char_start + ch.len_utf8();
            let style = if char_start < selection_end && char_end > selection_start {
                selection_style(base_style)
            } else {
                base_style
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn prompt_char_span() -> Span<'static> {
    Span::styled("❯", inactive_style())
}

fn input_prefix_space_span() -> Span<'static> {
    Span::raw(" ")
}

fn continuation_prompt_span() -> Span<'static> {
    Span::styled("  ", subtle_style())
}

fn prompt_clear_span(width: usize) -> Span<'static> {
    Span::raw(" ".repeat(width))
}

fn followup_header_line(text: &str) -> StyledLine {
    Line::from(vec![Span::styled(text.to_string(), subtle_style())])
}

fn followup_item_line(content: &str, width: usize) -> StyledLine {
    let marker = "  ↳ ";
    let available = width
        .saturating_sub(marker.chars().map(display_width).sum::<usize>())
        .max(1);
    Line::from(vec![
        Span::styled(marker, subtle_style()),
        Span::styled(
            truncate_chars(&content.replace('\n', " "), available),
            inactive_style(),
        ),
    ])
}

fn followup_hint_line(text: &str) -> StyledLine {
    Line::from(vec![Span::styled(format!("      {text}"), subtle_style())])
}

impl TuiState {
    pub(crate) fn followup_prompt_lines(&self, width: usize) -> Vec<StyledLine> {
        let mut lines = Vec::new();
        if !self.steered_followups.is_empty() {
            lines.push(followup_header_line(
                "• Messages to be submitted after next tool call (press esc to interrupt and send immediately)",
            ));
            for content in &self.steered_followups {
                lines.push(followup_item_line(content, width));
            }
        }
        if !self.queued_followups.is_empty() {
            lines.push(followup_header_line("• Queued follow-up inputs"));
            for content in &self.queued_followups {
                lines.push(followup_item_line(content, width));
            }
            lines.push(followup_hint_line("shift + ← edit last queued message"));
        }
        lines
    }

    pub(crate) fn prompt_lines(&self, input_view: &InputView) -> Vec<StyledLine> {
        let mut lines = Vec::new();
        if input_view.lines.is_empty() {
            lines.push(Line::from(vec![
                prompt_char_span(),
                input_prefix_space_span(),
                prompt_clear_span(input_view.width.max(1)),
            ]));
            return lines;
        }

        lines.extend(input_view.lines.iter().enumerate().map(|(index, line)| {
            let line_width = line.chars().map(display_width).sum::<usize>();
            let padding_width = input_view.width.saturating_sub(line_width);
            let input_spans = input_view
                .line_layouts
                .get(index)
                .map(|layout| input_line_spans(layout, self.input_selection.as_ref()))
                .unwrap_or_default();
            if index == 0 {
                let mut spans = vec![prompt_char_span(), input_prefix_space_span()];
                if line.is_empty() {
                    spans.push(prompt_clear_span(1));
                } else {
                    spans.extend(input_spans);
                }
                if padding_width > 0 {
                    spans.push(prompt_clear_span(padding_width));
                }
                Line::from(spans)
            } else if line.is_empty() {
                Line::from(vec![
                    continuation_prompt_span(),
                    prompt_clear_span(input_view.width.max(1)),
                ])
            } else {
                let mut spans = vec![continuation_prompt_span()];
                spans.extend(input_spans);
                spans.push(prompt_clear_span(padding_width));
                Line::from(spans)
            }
        }));
        lines
    }

    pub(crate) fn slash_argument_completion_lines(
        &self,
        view: &SlashArgumentCompletionView,
        width: usize,
    ) -> Vec<StyledLine> {
        view.suggestions
            .iter()
            .skip(view.start)
            .take(view.visible_count)
            .enumerate()
            .map(|(index, suggestion)| {
                let absolute_index = view.start + index;
                let selected = absolute_index == view.selected;
                let marker = if selected { "› " } else { "  " };
                let command_width = 12usize.min(width.saturating_sub(10).max(1));
                let label = pad_or_truncate(&suggestion.label, command_width);
                let description_width =
                    width.saturating_sub(command_width).saturating_sub(8).max(1);
                let description = truncate_chars(&suggestion.description, description_width);
                let muted = empty_transcript_placeholder_style();
                let style = if selected { Style::default() } else { muted };
                let scrollbar_style = if suggestion_scrollbar_active(
                    index,
                    view.suggestions.len(),
                    view.start,
                    view.visible_count,
                ) {
                    Style::default()
                } else {
                    muted
                };
                Line::from(vec![
                    Span::styled("│", scrollbar_style),
                    Span::styled("  ", muted),
                    Span::styled(marker.to_string(), style),
                    Span::styled(label, style),
                    Span::styled("  ", muted),
                    Span::styled(description, style),
                ])
            })
            .collect()
    }

    pub(crate) fn add_dir_completion_lines(
        &self,
        view: &AddDirCompletionView,
        width: usize,
    ) -> Vec<StyledLine> {
        let muted = empty_transcript_placeholder_style();
        let label_width = width.saturating_sub(5).max(1);
        view.suggestions
            .iter()
            .skip(view.start)
            .take(view.visible_count)
            .enumerate()
            .map(|(index, suggestion)| {
                let absolute_index = view.start + index;
                let selected = absolute_index == view.selected;
                let style = if selected { Style::default() } else { muted };
                let scrollbar_style = if add_dir_completion_scrollbar_active(index, view) {
                    Style::default()
                } else {
                    muted
                };
                Line::from(vec![
                    Span::styled("│", scrollbar_style),
                    Span::styled("  ", muted),
                    Span::styled(truncate_chars(&suggestion.label, label_width), style),
                ])
            })
            .collect()
    }
}
