use anyhow::Result;
use orbcode_protocol::SessionStatus;

use crate::numeric::saturating_u16;
use crate::state::TuiState;

use super::*;

const SESSION_PICKER_VISIBLE_ROWS: usize = 8;

pub(crate) struct SessionPickerState {
    pub(crate) command: String,
    pub(crate) title: String,
    pub(crate) all_sessions: Vec<SessionSummary>,
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) selected: usize,
    pub(crate) query: String,
}

pub(crate) enum SessionPickerKeyAction {
    None,
    Close,
    Resume { command: String, session_id: String },
    Fork { command: String, session_id: String },
}

impl TuiState {
    pub(crate) async fn open_session_picker(
        &mut self,
        app_server: &AppClient,
        command: &str,
        title: &str,
    ) -> Result<()> {
        let sessions = app_server.list_sessions().await?;
        if sessions.is_empty() {
            self.push_local_slash_command_output(
                command,
                "No resumable sessions found for this project.",
                None,
            );
            self.set_status_line("No resumable sessions found for this project.");
            return Ok(());
        }

        self.overlay = Some(OverlayState::SessionPicker(SessionPickerState::new(
            command,
            title.to_string(),
            sessions,
            &self.session_id,
        )));
        self.set_status_line(
            "Session picker: type to filter, Enter resume, Ctrl+F fork, Esc close.",
        );
        Ok(())
    }
}

impl SessionPickerState {
    pub(crate) fn new(
        command: impl Into<String>,
        title: impl Into<String>,
        sessions: Vec<SessionSummary>,
        current_session_id: &str,
    ) -> Self {
        let mut picker = Self {
            command: command.into(),
            title: title.into(),
            all_sessions: sessions,
            sessions: Vec::new(),
            selected: 0,
            query: String::new(),
        };
        picker.refresh(Some(current_session_id));
        picker
    }

    pub(crate) fn refresh(&mut self, preferred_session_id: Option<&str>) {
        let preferred_session_id = preferred_session_id.map(str::to_string);
        let query = self.query.trim().to_lowercase();
        let terms = query
            .split_whitespace()
            .filter(|term| !term.is_empty())
            .collect::<Vec<_>>();
        self.sessions = if terms.is_empty() {
            self.all_sessions.clone()
        } else {
            let mut matches = self
                .all_sessions
                .iter()
                .enumerate()
                .filter_map(|(index, session)| {
                    session_match_score(session, &terms)
                        .map(|score| (score, index, session.clone()))
                })
                .collect::<Vec<_>>();
            matches.sort_by_key(|(score, index, _)| (*score, *index));
            matches.into_iter().map(|(_, _, session)| session).collect()
        };
        self.selected = preferred_session_id
            .as_deref()
            .and_then(|session_id| {
                self.sessions
                    .iter()
                    .position(|session| session.session_id == session_id)
            })
            .unwrap_or(0);
        if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len().saturating_sub(1);
        }
    }

    fn push_query_char(&mut self, character: char) {
        self.query.push(character);
        self.refresh(None);
    }

    fn pop_query_char(&mut self) {
        self.query.pop();
        self.refresh(None);
    }

    fn clear_query(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            self.refresh(None);
        }
    }
}

pub(crate) fn apply_session_picker_key(
    picker: &mut SessionPickerState,
    key_event: &KeyEvent,
) -> SessionPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => SessionPickerKeyAction::Close,
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            SelectedIndex::new(&mut picker.selected, picker.sessions.len()).apply_key(
                key_event.code,
                Some(8),
                false,
            );
            SessionPickerKeyAction::None
        }
        KeyCode::Backspace => {
            picker.pop_query_char();
            SessionPickerKeyAction::None
        }
        KeyCode::Delete if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.clear_query();
            SessionPickerKeyAction::None
        }
        KeyCode::Char('u') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.clear_query();
            SessionPickerKeyAction::None
        }
        KeyCode::Enter => picker
            .sessions
            .get(picker.selected)
            .filter(|session| matches!(session.status, SessionStatus::Available))
            .map_or(SessionPickerKeyAction::None, |session| {
                SessionPickerKeyAction::Resume {
                    command: picker.command.clone(),
                    session_id: session.session_id.clone(),
                }
            }),
        KeyCode::Char('f') if key_event.modifiers.contains(KeyModifiers::CONTROL) => picker
            .sessions
            .get(picker.selected)
            .filter(|session| matches!(session.status, SessionStatus::Available))
            .map_or(SessionPickerKeyAction::None, |session| {
                SessionPickerKeyAction::Fork {
                    command: picker.command.clone(),
                    session_id: session.session_id.clone(),
                }
            }),
        KeyCode::Char(character)
            if !key_event.modifiers.contains(KeyModifiers::CONTROL)
                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
        {
            picker.push_query_char(character);
            SessionPickerKeyAction::None
        }
        _ => SessionPickerKeyAction::None,
    }
}

pub(crate) fn session_picker_lines(picker: &SessionPickerState, width: usize) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let query = if picker.query.is_empty() {
        "Search…"
    } else {
        picker.query.as_str()
    };
    let query_style = if picker.query.is_empty() {
        muted
    } else {
        Style::default()
    };
    let layout = session_picker_filter_layout(picker, width);
    let mut lines = vec![Line::from(vec![
        Span::styled(" ", muted),
        Span::styled(
            layout.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", muted),
        Span::styled(layout.count.clone(), muted),
    ])];
    lines.push(Line::from(vec![
        Span::styled(" ╭", muted),
        Span::styled("─".repeat(layout.box_width), muted),
        Span::styled("╮", muted),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" │ ⌕ ", muted),
        Span::styled(pad_or_truncate(query, layout.query_width), query_style),
        Span::styled(" │", muted),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" ╰", muted),
        Span::styled("─".repeat(layout.box_width), muted),
        Span::styled("╯", muted),
    ]));

    if picker.sessions.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" │", muted),
            Span::styled("  No sessions found.", muted),
        ]));
        return lines;
    }

    let visible_count = picker.sessions.len().min(SESSION_PICKER_VISIBLE_ROWS);
    let start = slash_command_view_start(picker.selected, picker.sessions.len(), visible_count);
    lines.extend(
        picker
            .sessions
            .iter()
            .skip(start)
            .take(visible_count)
            .enumerate()
            .map(|(index, session)| {
                let absolute_index = start + index;
                format_session_picker_line(
                    session,
                    absolute_index == picker.selected,
                    session_picker_scrollbar_active(
                        index,
                        picker.sessions.len(),
                        start,
                        visible_count,
                    ),
                    width,
                )
            }),
    );
    lines
}

struct SessionPickerFilterLayout {
    title: String,
    count: String,
    box_width: usize,
    query_width: usize,
    query_offset: usize,
}

fn session_picker_filter_layout(
    picker: &SessionPickerState,
    width: usize,
) -> SessionPickerFilterLayout {
    let title_width = width / 4;
    let title = truncate_chars(&picker.title, title_width.saturating_sub(4).max(1));
    let count = format!("{} / {}", picker.sessions.len(), picker.all_sessions.len());
    let box_width = width.saturating_sub(3).clamp(8, 80);
    let query_width = box_width.saturating_sub(4).max(1);
    SessionPickerFilterLayout {
        title,
        count,
        box_width,
        query_width,
        query_offset: 5,
    }
}

fn session_picker_scrollbar_active(
    row: usize,
    total: usize,
    start: usize,
    visible_count: usize,
) -> bool {
    suggestion_scrollbar_active(row, total, start, visible_count)
}

pub(crate) fn session_picker_cursor(picker: &SessionPickerState, area: Rect) -> Option<(u16, u16)> {
    if area.height < 3 || area.width == 0 {
        return None;
    }
    let layout = session_picker_filter_layout(picker, area.width as usize);
    let query_cursor = display_width_str(&truncate_chars(&picker.query, layout.query_width));
    Some((
        area.x
            .saturating_add(saturating_u16(layout.query_offset))
            .saturating_add(saturating_u16(query_cursor))
            .min(area.x.saturating_add(area.width.saturating_sub(1))),
        area.y.saturating_add(2),
    ))
}

fn format_session_picker_line(
    session: &SessionSummary,
    selected: bool,
    scrollbar_active: bool,
    max_width: usize,
) -> StyledLine {
    let muted = empty_transcript_placeholder_style();
    let marker = if selected { "› " } else { "  " };
    let corrupt = matches!(session.status, SessionStatus::Corrupt { .. });
    let mut suffix_parts = vec![format!("msgs={}", session.message_count)];
    if let Some(branch) = session.git_branch.as_deref()
        && !branch.is_empty()
    {
        suffix_parts.push(format!("@{branch}"));
    }
    suffix_parts.push(session.updated_at.format("%m-%d %H:%M").to_string());
    let suffix = format!("  {}", suffix_parts.join("  "));
    let fixed = 1 + 1 + 2 + marker.chars().count() + 8 + 2 + suffix.chars().count();
    let available_title = max_width.saturating_sub(fixed).max(8);
    let display_title: String = if corrupt {
        match &session.status {
            SessionStatus::Corrupt { reason } => format!("(corrupt: {reason})"),
            _ => "(corrupt)".to_string(),
        }
    } else {
        session.title.clone().unwrap_or_else(|| "(untitled)".into())
    };
    let title = truncate_chars(&display_title, available_title);
    let palette = active_palette();
    let title_style = if corrupt {
        Style::default().fg(palette.error)
    } else if selected {
        Style::default()
    } else {
        muted
    };
    let style = if selected { Style::default() } else { muted };
    let scrollbar_style = if scrollbar_active {
        Style::default()
    } else {
        muted
    };
    let id_style = if corrupt {
        Style::default().fg(palette.error)
    } else if selected {
        Style::default().fg(palette.accent)
    } else {
        muted
    };
    Line::from(vec![
        Span::styled(" ", muted),
        Span::styled("│", scrollbar_style),
        Span::styled("  ", muted),
        Span::styled(marker.to_string(), style),
        Span::styled(short_session_id(&session.session_id).to_string(), id_style),
        Span::styled("  ", muted),
        Span::styled(title, title_style),
        Span::styled(suffix, muted),
    ])
}

fn session_match_score(session: &SessionSummary, terms: &[&str]) -> Option<usize> {
    if terms.is_empty() {
        return Some(0);
    }

    terms.iter().try_fold(0usize, |score, term| {
        session_term_score(session, term).map(|term_score| score + term_score)
    })
}

fn session_term_score(session: &SessionSummary, term: &str) -> Option<usize> {
    let title = session.title.as_deref().unwrap_or("(untitled)");
    let mut scores = vec![
        session_visible_field_score(short_session_id(&session.session_id), term),
        session_full_id_score(session.session_id.as_str(), term).map(|score| 10 + score),
        session_visible_field_score(title, term).map(|score| 100 + score),
    ];
    if let Some(branch) = session.git_branch.as_deref() {
        scores.push(session_visible_field_score(branch, term).map(|score| 80 + score));
    }
    if let Some(cwd) = session.cwd.as_deref() {
        scores.push(session_visible_field_score(cwd, term).map(|score| 140 + score));
    }
    if let Some(model) = session.model.as_deref() {
        scores.push(session_visible_field_score(model, term).map(|score| 160 + score));
    }
    if let Some(provider) = session.provider {
        scores.push(session_visible_field_score(provider.as_str(), term).map(|score| 180 + score));
    }
    scores.into_iter().flatten().min()
}

fn session_visible_field_score(candidate: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }

    let candidate_lower = candidate.to_lowercase();
    let query_lower = query.to_lowercase();
    if candidate_lower == query_lower {
        return Some(0);
    }
    if candidate_lower.starts_with(&query_lower) {
        return Some(1);
    }
    if let Some(index) = candidate_lower.find(&query_lower) {
        return Some(20 + index);
    }
    fuzzy_match_score(candidate, query).map(|score| 100 + score)
}

fn session_full_id_score(candidate: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }

    let candidate_lower = candidate.to_lowercase();
    let query_lower = query.to_lowercase();
    if candidate_lower == query_lower {
        return Some(0);
    }
    if candidate_lower.starts_with(&query_lower) {
        return Some(1);
    }
    None
}
