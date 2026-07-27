use orbcode_protocol::{MessageRole, TranscriptMessage};

use crate::state::TuiState;

use super::*;

const REWIND_PICKER_VISIBLE_ROWS: usize = 8;

/// One rewindable checkpoint: a user turn in the transcript. Restoring it
/// keeps every message *before* the turn (`keep_messages`) and pre-fills the
/// prompt input with the turn text so the user can edit and resubmit.
pub(crate) struct RewindEntry {
    /// Display-list index of this turn — a best-effort estimate of the kept
    /// count, used only for the status text. The actual truncation resolves
    /// [`anchor_id`](Self::anchor_id) against the persisted record.
    pub(crate) keep_messages: usize,
    /// Persisted id of the user message this turn rewinds to. The truncation is
    /// resolved from this id, because the in-memory display list can diverge
    /// from the persisted transcript (which is what the server truncates).
    pub(crate) anchor_id: String,
    pub(crate) ordinal: usize,
    pub(crate) preview: String,
    pub(crate) prompt: String,
}

pub(crate) struct RewindPickerState {
    pub(crate) command: String,
    pub(crate) session_id: String,
    pub(crate) entries: Vec<RewindEntry>,
    pub(crate) selected: usize,
}

pub(crate) enum RewindPickerKeyAction {
    None,
    Close,
    Rewind {
        command: String,
        session_id: String,
        keep_messages: usize,
        anchor_id: String,
        restore_prompt: String,
    },
}

impl RewindPickerState {
    /// Build a picker from the in-memory transcript. Returns `None` when there
    /// is no user turn to rewind to.
    pub(crate) fn from_messages(
        command: impl Into<String>,
        session_id: impl Into<String>,
        messages: &[TranscriptMessage],
    ) -> Option<Self> {
        let mut entries = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if !matches!(message.role, MessageRole::User) {
                continue;
            }
            let prompt = message.content.clone();
            let preview = collapse_inline_whitespace(&prompt);
            let preview = if preview.trim().is_empty() {
                "(empty prompt)".to_string()
            } else {
                preview
            };
            entries.push(RewindEntry {
                keep_messages: index,
                anchor_id: message.id.clone(),
                ordinal: entries.len() + 1,
                preview,
                prompt,
            });
        }
        if entries.is_empty() {
            return None;
        }
        // Default selection to the most recent user turn.
        let selected = entries.len() - 1;
        Some(Self {
            command: command.into(),
            session_id: session_id.into(),
            entries,
            selected,
        })
    }
}

impl TuiState {
    pub(crate) fn open_rewind_picker(&mut self, command: &str) {
        match RewindPickerState::from_messages(command, &self.session_id, &self.messages) {
            Some(picker) => {
                self.overlay = Some(OverlayState::RewindPicker(picker));
                self.set_status_line("Rewind: ↑↓ select a turn, Enter restore, Esc close.");
            }
            None => {
                self.push_local_slash_command_output(
                    command,
                    "No user turns to rewind to yet.",
                    None,
                );
                self.set_status_line("No user turns to rewind to yet.");
            }
        }
    }
}

pub(crate) fn apply_rewind_picker_key(
    picker: &mut RewindPickerState,
    key_event: &KeyEvent,
) -> RewindPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => RewindPickerKeyAction::Close,
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            SelectedIndex::new(&mut picker.selected, picker.entries.len()).apply_key(
                key_event.code,
                Some(REWIND_PICKER_VISIBLE_ROWS),
                false,
            );
            RewindPickerKeyAction::None
        }
        KeyCode::Enter => {
            picker
                .entries
                .get(picker.selected)
                .map_or(RewindPickerKeyAction::None, |entry| {
                    RewindPickerKeyAction::Rewind {
                        command: picker.command.clone(),
                        session_id: picker.session_id.clone(),
                        keep_messages: entry.keep_messages,
                        anchor_id: entry.anchor_id.clone(),
                        restore_prompt: entry.prompt.clone(),
                    }
                })
        }
        _ => RewindPickerKeyAction::None,
    }
}

pub(crate) fn rewind_picker_lines(picker: &RewindPickerState, width: usize) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" ", muted),
            Span::styled(
                "Rewind conversation".to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", muted),
            Span::styled(format!("{} turn(s)", picker.entries.len()), muted),
        ]),
        Line::from(vec![Span::styled(
            "  Select a user turn to restore. The transcript is trimmed to before it and the prompt returns to your input.".to_string(),
            muted,
        )]),
    ];

    let visible_count = picker.entries.len().min(REWIND_PICKER_VISIBLE_ROWS);
    let start = slash_command_view_start(picker.selected, picker.entries.len(), visible_count);
    lines.extend(
        picker
            .entries
            .iter()
            .skip(start)
            .take(visible_count)
            .enumerate()
            .map(|(index, entry)| {
                let absolute_index = start + index;
                format_rewind_entry_line(
                    entry,
                    absolute_index == picker.selected,
                    suggestion_scrollbar_active(index, picker.entries.len(), start, visible_count),
                    width,
                )
            }),
    );
    lines
}

fn format_rewind_entry_line(
    entry: &RewindEntry,
    selected: bool,
    scrollbar_active: bool,
    max_width: usize,
) -> StyledLine {
    let muted = empty_transcript_placeholder_style();
    let marker = if selected { "› " } else { "  " };
    let ordinal = format!("#{}", entry.ordinal);
    let fixed = 1 + 1 + 2 + marker.chars().count() + ordinal.chars().count() + 2;
    let available_preview = max_width.saturating_sub(fixed).max(8);
    let preview = truncate_chars(&entry.preview, available_preview);
    let style = if selected { Style::default() } else { muted };
    let scrollbar_style = if scrollbar_active {
        Style::default()
    } else {
        muted
    };
    let ordinal_style = if selected { accent_style() } else { muted };
    Line::from(vec![
        Span::styled(" ", muted),
        Span::styled("│", scrollbar_style),
        Span::styled("  ", muted),
        Span::styled(marker.to_string(), style),
        Span::styled(ordinal, ordinal_style),
        Span::styled("  ", muted),
        Span::styled(preview, style),
    ])
}
