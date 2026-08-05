use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use orbcode_app_server_client::{
    AskUserAnswerValue, AskUserCancellationReason, AskUserQuestionRequest, AskUserQuestionSpec,
    AskUserResponseOutcome,
};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::custom_terminal::Frame;
use crate::numeric::saturating_u16;
use crate::render::styled_wrap::wrap_styled_lines;
use crate::render::text_utils::{StyledLine, truncate_display_width};
use crate::tui_theme::{accent_style, emphasis_style, inactive_style, subtle_style, warning_style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AskUserTextField {
    Other,
    Annotation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AskUserRow {
    Option(usize),
    Other,
    Annotation,
    Submit,
    Reject,
    Clarify,
    Finish,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AskUserQuestionKeyAction {
    None,
    Respond {
        request_id: String,
        outcome: AskUserResponseOutcome,
        interrupt_turn: bool,
    },
}

#[derive(Clone)]
pub(crate) struct AskUserQuestionOverlayState {
    pub(crate) request: AskUserQuestionRequest,
    pub(crate) question_index: usize,
    focused_row: usize,
    single_answers: BTreeMap<String, String>,
    multi_answers: BTreeMap<String, BTreeSet<String>>,
    other_answers: BTreeMap<String, String>,
    annotations: BTreeMap<String, String>,
    editing: Option<AskUserTextField>,
    pub(crate) validation_error: Option<String>,
    pub(crate) panel_scroll: usize,
    pub(crate) panel_area: Rect,
    queued: VecDeque<AskUserQuestionRequest>,
}

impl AskUserQuestionOverlayState {
    pub(crate) fn new(request: AskUserQuestionRequest) -> Self {
        Self {
            request,
            question_index: 0,
            focused_row: 0,
            single_answers: BTreeMap::new(),
            multi_answers: BTreeMap::new(),
            other_answers: BTreeMap::new(),
            annotations: BTreeMap::new(),
            editing: None,
            validation_error: None,
            panel_scroll: 0,
            panel_area: Rect::ZERO,
            queued: VecDeque::new(),
        }
    }

    pub(crate) fn enqueue(&mut self, request: AskUserQuestionRequest) {
        self.queued.push_back(request);
    }

    pub(crate) fn take_next_queued(&mut self) -> Option<Self> {
        self.queued.pop_front().map(|request| {
            let mut next = Self::new(request);
            next.queued = std::mem::take(&mut self.queued);
            next
        })
    }

    pub(crate) fn remove_queued(&mut self, request_id: &str) {
        self.queued
            .retain(|request| request.request_id != request_id);
    }

    fn questions(&self) -> &[AskUserQuestionSpec] {
        &self.request.questions
    }

    fn question(&self) -> &AskUserQuestionSpec {
        &self.questions()[self.question_index]
    }

    fn rows(&self) -> Vec<AskUserRow> {
        let question = self.question();
        let mut rows = (0..question.options.len())
            .map(AskUserRow::Option)
            .collect::<Vec<_>>();
        if question.allow_free_text {
            rows.push(AskUserRow::Other);
        }
        if question.allow_annotation {
            rows.push(AskUserRow::Annotation);
        }
        rows.extend([
            AskUserRow::Submit,
            AskUserRow::Reject,
            AskUserRow::Clarify,
            AskUserRow::Finish,
        ]);
        rows
    }

    fn focused(&self) -> AskUserRow {
        let rows = self.rows();
        rows[self.focused_row.min(rows.len().saturating_sub(1))]
    }

    pub(crate) fn move_focus(&mut self, delta: isize) {
        let len = self.rows().len();
        self.focused_row = if delta < 0 {
            self.focused_row
                .checked_sub(delta.unsigned_abs())
                .unwrap_or(len - 1)
        } else {
            (self.focused_row + delta as usize) % len
        };
        self.validation_error = None;
    }

    pub(crate) fn focus_from_mouse_row(&mut self, row: u16) {
        if self.panel_area.height <= 2 {
            return;
        }
        let relative = row
            .saturating_sub(self.panel_area.y.saturating_add(1))
            .min(self.panel_area.height.saturating_sub(3)) as usize;
        let visible_height = self.panel_area.height.saturating_sub(2).max(1) as usize;
        let len = self.rows().len();
        self.focused_row = relative.saturating_mul(len) / visible_height;
        self.focused_row = self.focused_row.min(len.saturating_sub(1));
        self.validation_error = None;
    }

    fn move_question(&mut self, delta: isize) {
        let len = self.questions().len();
        self.question_index = if delta < 0 {
            self.question_index
                .checked_sub(delta.unsigned_abs())
                .unwrap_or(len - 1)
        } else {
            (self.question_index + delta as usize) % len
        };
        self.focused_row = 0;
        self.editing = None;
        self.validation_error = None;
    }

    fn edit_buffer_mut(&mut self, field: AskUserTextField) -> &mut String {
        let question_id = self.question().id.clone();
        match field {
            AskUserTextField::Other => self.other_answers.entry(question_id).or_default(),
            AskUserTextField::Annotation => self.annotations.entry(question_id).or_default(),
        }
    }

    fn activate_focused(&mut self) -> AskUserQuestionKeyAction {
        let question = self.question().clone();
        match self.focused() {
            AskUserRow::Option(index) => {
                let option_id = question.options[index].id.clone();
                self.other_answers.remove(&question.id);
                if question.multi_select {
                    let selected = self.multi_answers.entry(question.id).or_default();
                    if !selected.remove(&option_id) {
                        selected.insert(option_id);
                    }
                } else {
                    self.single_answers.insert(question.id, option_id);
                }
                AskUserQuestionKeyAction::None
            }
            AskUserRow::Other => {
                self.editing = match self.editing {
                    Some(AskUserTextField::Other) => None,
                    _ => Some(AskUserTextField::Other),
                };
                AskUserQuestionKeyAction::None
            }
            AskUserRow::Annotation => {
                self.editing = match self.editing {
                    Some(AskUserTextField::Annotation) => None,
                    _ => Some(AskUserTextField::Annotation),
                };
                AskUserQuestionKeyAction::None
            }
            AskUserRow::Submit => self.submit(),
            AskUserRow::Reject => self.respond(AskUserResponseOutcome::Rejected, false),
            AskUserRow::Clarify => self.respond(AskUserResponseOutcome::Clarify, false),
            AskUserRow::Finish => self.respond(AskUserResponseOutcome::FinishPlanInterview, false),
        }
    }

    fn submit(&mut self) -> AskUserQuestionKeyAction {
        let mut answers = BTreeMap::new();
        let questions = self.questions().to_vec();
        let question_count = questions.len();
        for (question_index, question) in questions.into_iter().enumerate() {
            let answer = if let Some(text) = self
                .other_answers
                .get(&question.id)
                .filter(|text| !text.trim().is_empty())
            {
                AskUserAnswerValue::Text { text: text.clone() }
            } else if question.multi_select {
                let option_ids = self
                    .multi_answers
                    .get(&question.id)
                    .map(|selected| selected.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                if option_ids.is_empty() {
                    self.question_index = question_index;
                    self.focused_row = 0;
                    self.validation_error = Some(format!(
                        "Answer required for {} ({}/{})",
                        question.header,
                        question_index + 1,
                        question_count
                    ));
                    return AskUserQuestionKeyAction::None;
                }
                AskUserAnswerValue::SelectedMany { option_ids }
            } else if let Some(option_id) = self.single_answers.get(&question.id) {
                AskUserAnswerValue::Selected {
                    option_id: option_id.clone(),
                }
            } else {
                self.question_index = question_index;
                self.focused_row = 0;
                self.validation_error = Some(format!(
                    "Answer required for {} ({}/{})",
                    question.header,
                    question_index + 1,
                    question_count
                ));
                return AskUserQuestionKeyAction::None;
            };
            answers.insert(question.id.clone(), answer);
        }
        let annotations = self
            .annotations
            .iter()
            .filter(|(_, annotation)| !annotation.trim().is_empty())
            .map(|(id, annotation)| (id.clone(), annotation.clone()))
            .collect();
        self.respond(
            AskUserResponseOutcome::Answered {
                answers,
                annotations,
            },
            false,
        )
    }

    fn respond(
        &self,
        outcome: AskUserResponseOutcome,
        interrupt_turn: bool,
    ) -> AskUserQuestionKeyAction {
        AskUserQuestionKeyAction::Respond {
            request_id: self.request.request_id.clone(),
            outcome,
            interrupt_turn,
        }
    }

    fn selected(&self, question: &AskUserQuestionSpec, option_id: &str) -> bool {
        if question.multi_select {
            self.multi_answers
                .get(&question.id)
                .is_some_and(|selected| selected.contains(option_id))
        } else {
            self.single_answers.get(&question.id).map(String::as_str) == Some(option_id)
        }
    }
}

pub(crate) fn apply_ask_user_question_key(
    state: &mut AskUserQuestionOverlayState,
    key_event: &KeyEvent,
) -> AskUserQuestionKeyAction {
    if key_event.code == KeyCode::Char('c') && key_event.modifiers.contains(KeyModifiers::CONTROL) {
        return state.respond(
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Interrupt,
            },
            true,
        );
    }
    if key_event.code == KeyCode::Esc {
        if state.editing.take().is_some() {
            return AskUserQuestionKeyAction::None;
        }
        return state.respond(
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::ClientClosed,
            },
            false,
        );
    }
    if let Some(field) = state.editing {
        match key_event.code {
            KeyCode::Enter | KeyCode::Tab => state.editing = None,
            KeyCode::Backspace => {
                state.edit_buffer_mut(field).pop();
            }
            KeyCode::Char(character)
                if !key_event.modifiers.contains(KeyModifiers::CONTROL)
                    && !key_event.modifiers.contains(KeyModifiers::ALT) =>
            {
                state.edit_buffer_mut(field).push(character);
            }
            _ => {}
        }
        return AskUserQuestionKeyAction::None;
    }

    match key_event.code {
        KeyCode::Up | KeyCode::BackTab => state.move_focus(-1),
        KeyCode::Down | KeyCode::Tab => state.move_focus(1),
        KeyCode::Left => state.move_question(-1),
        KeyCode::Right => state.move_question(1),
        KeyCode::PageUp => state.panel_scroll = state.panel_scroll.saturating_add(6),
        KeyCode::PageDown => state.panel_scroll = state.panel_scroll.saturating_sub(6),
        KeyCode::Home => state.panel_scroll = usize::MAX / 2,
        KeyCode::End => state.panel_scroll = 0,
        KeyCode::Enter | KeyCode::Char(' ') => return state.activate_focused(),
        _ => {}
    }
    AskUserQuestionKeyAction::None
}

pub(crate) fn ask_user_question_panel_lines(
    state: &AskUserQuestionOverlayState,
    width: usize,
) -> Vec<StyledLine> {
    let question = state.question();
    let compact = width < 50;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "AskUserQuestion ",
            accent_style().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{}/{} · {}",
                state.question_index + 1,
                state.questions().len(),
                question.header
            ),
            emphasis_style(),
        ),
    ])];
    lines.push(Line::from(Span::styled(
        truncate_display_width(&question.question, width.saturating_sub(2).max(1)),
        inactive_style().add_modifier(Modifier::BOLD),
    )));
    if let Some(error) = &state.validation_error {
        lines.push(Line::from(Span::styled(error.clone(), warning_style())));
    }

    for (index, option) in question.options.iter().enumerate() {
        let focused = state.focused() == AskUserRow::Option(index);
        let selected = state.selected(question, &option.id);
        let marker = if question.multi_select {
            if selected { "[x]" } else { "[ ]" }
        } else if selected {
            "(•)"
        } else {
            "( )"
        };
        lines.push(Line::from(vec![
            Span::styled(if focused { "› " } else { "  " }, accent_style()),
            Span::styled(format!("{marker} {}", option.label), emphasis_style()),
        ]));
        if !compact && (!option.description.is_empty() || option.preview.is_some()) {
            if !option.description.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("      {}", option.description),
                    subtle_style(),
                )));
            }
            if focused && let Some(preview) = option.preview.as_deref() {
                lines.push(Line::from(Span::styled("      Preview", accent_style())));
                for preview_line in preview.lines().take(6) {
                    lines.push(Line::from(Span::styled(
                        format!("        {preview_line}"),
                        inactive_style(),
                    )));
                }
            }
        }
    }

    if question.allow_free_text {
        let value = state
            .other_answers
            .get(&question.id)
            .map(String::as_str)
            .unwrap_or("");
        lines.push(text_row(
            state.focused() == AskUserRow::Other,
            "Other",
            value,
            state.editing == Some(AskUserTextField::Other),
        ));
    }
    if question.allow_annotation {
        let value = state
            .annotations
            .get(&question.id)
            .map(String::as_str)
            .unwrap_or("");
        lines.push(text_row(
            state.focused() == AskUserRow::Annotation,
            "Note",
            value,
            state.editing == Some(AskUserTextField::Annotation),
        ));
    }

    lines.push(Line::from(""));
    for (row, label) in [
        (AskUserRow::Submit, "Submit"),
        (AskUserRow::Reject, "Reject"),
        (AskUserRow::Clarify, "Clarify"),
        (AskUserRow::Finish, "Finish interview"),
    ] {
        let focused = state.focused() == row;
        lines.push(Line::from(vec![
            Span::styled(if focused { "› " } else { "  " }, accent_style()),
            Span::styled(format!("[ {label} ]"), emphasis_style()),
        ]));
    }
    lines.push(Line::from(Span::styled(
        if compact {
            "↑↓ choose · ←→ question · Enter select · Esc cancel"
        } else {
            "↑↓/Tab choose · ←→ question · Space select · Enter edit/submit · Esc cancel · Ctrl+C interrupt"
        },
        subtle_style(),
    )));
    lines
}

fn text_row(focused: bool, label: &str, value: &str, editing: bool) -> StyledLine {
    Line::from(vec![
        Span::styled(if focused { "› " } else { "  " }, accent_style()),
        Span::styled(format!("{label}: "), emphasis_style()),
        Span::styled(
            if editing {
                format!("{value}▏")
            } else if value.is_empty() {
                "<empty>".to_string()
            } else {
                value.to_string()
            },
            inactive_style(),
        ),
    ])
}

pub(crate) fn ask_user_question_panel_height(
    state: &AskUserQuestionOverlayState,
    host_area: Rect,
) -> u16 {
    let inner_width = host_area.width.saturating_sub(2).max(1) as usize;
    let wrapped = wrap_styled_lines(
        &ask_user_question_panel_lines(state, inner_width),
        inner_width,
    );
    let desired = saturating_u16(wrapped.len()).saturating_add(2);
    desired.clamp(5, host_area.height.saturating_sub(3).max(5))
}

pub(crate) fn ask_user_question_panel_desired_height(
    state: &AskUserQuestionOverlayState,
    width: usize,
) -> u16 {
    let inner_width = width.saturating_sub(2).max(1);
    saturating_u16(
        wrap_styled_lines(
            &ask_user_question_panel_lines(state, inner_width),
            inner_width,
        )
        .len(),
    )
    .saturating_add(2)
}

pub(crate) fn draw_ask_user_question_panel(
    frame: &mut Frame,
    state: &mut AskUserQuestionOverlayState,
    area: Rect,
) {
    state.panel_area = area;
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;
    let lines = wrap_styled_lines(
        &ask_user_question_panel_lines(state, inner_width),
        inner_width,
    );
    let max_scroll = lines.len().saturating_sub(inner_height);
    let scroll = state.panel_scroll.min(max_scroll);
    let start = max_scroll.saturating_sub(scroll);
    let visible = lines
        .into_iter()
        .skip(start)
        .take(inner_height)
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(visible)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Ask user "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_app_server_client::{AskUserOption, AskUserQuestionSpec};

    fn request(multi_select: bool) -> AskUserQuestionRequest {
        AskUserQuestionRequest {
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            tool_use_id: "tool-1".into(),
            request_id: "ask-1".into(),
            deadline: None,
            validation_error: None,
            questions: vec![AskUserQuestionSpec {
                id: "features".into(),
                question: "Which features?".into(),
                header: "Features".into(),
                multi_select,
                options: vec![AskUserOption {
                    id: "search".into(),
                    label: "Search".into(),
                    description: "Full-text search".into(),
                    preview: Some("Search preview".into()),
                }],
                allow_free_text: true,
                allow_annotation: true,
            }],
            question: String::new(),
            options: Vec::new(),
        }
    }

    #[test]
    fn multi_select_and_submit_build_typed_answer() {
        let mut state = AskUserQuestionOverlayState::new(request(true));
        assert_eq!(state.activate_focused(), AskUserQuestionKeyAction::None);
        state.focused_row = state.rows().len() - 4;
        let action = state.activate_focused();
        let AskUserQuestionKeyAction::Respond { outcome, .. } = action else {
            panic!("expected response");
        };
        let AskUserResponseOutcome::Answered { answers, .. } = outcome else {
            panic!("expected answered outcome");
        };
        assert_eq!(
            answers["features"],
            AskUserAnswerValue::SelectedMany {
                option_ids: vec!["search".into()]
            }
        );
    }

    #[test]
    fn narrow_panel_keeps_non_color_selection_markers() {
        let state = AskUserQuestionOverlayState::new(request(false));
        let rendered = ask_user_question_panel_lines(&state, 32)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("( ) Search"));
        assert!(rendered.contains("←→ question"));
    }

    fn snapshot_text(state: &AskUserQuestionOverlayState, width: usize) -> String {
        ask_user_question_panel_lines(state, width)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn ratatui_snapshot_single_other_and_preview() {
        let state = AskUserQuestionOverlayState::new(request(false));
        let snapshot = snapshot_text(&state, 80);
        assert!(snapshot.contains("AskUserQuestion 1/1 · Features"));
        assert!(snapshot.contains("( ) Search"));
        assert!(snapshot.contains("Preview\n        Search preview"));
        assert!(snapshot.contains("Other: <empty>"));
        assert!(snapshot.contains("Note: <empty>"));
    }

    #[test]
    fn ratatui_snapshot_four_question_navigation() {
        let mut request = request(false);
        let template = request.questions[0].clone();
        request.questions = (1..=4)
            .map(|index| AskUserQuestionSpec {
                id: format!("question-{index}"),
                question: format!("Question {index}?"),
                header: format!("Q{index}"),
                ..template.clone()
            })
            .collect();
        let mut state = AskUserQuestionOverlayState::new(request);
        state.move_question(-1);
        let snapshot = snapshot_text(&state, 80);
        assert!(snapshot.contains("AskUserQuestion 4/4 · Q4"));
        assert!(snapshot.contains("Question 4?"));
    }

    #[test]
    fn ratatui_snapshot_multi_select_and_validation_error() {
        let mut state = AskUserQuestionOverlayState::new(request(true));
        state.focused_row = state.rows().len() - 4;
        assert_eq!(state.activate_focused(), AskUserQuestionKeyAction::None);
        let snapshot = snapshot_text(&state, 80);
        assert!(snapshot.contains("[ ] Search"));
        assert!(snapshot.contains("Answer required for Features (1/1)"));
    }

    #[test]
    fn ratatui_snapshot_narrow_cjk_text_is_width_safe() {
        let mut request = request(false);
        request.questions[0].question = "选择数据库数据库数据库数据库数据库".into();
        let state = AskUserQuestionOverlayState::new(request);
        let snapshot = snapshot_text(&state, 24);
        assert!(snapshot.contains("选择数据库"));
        assert!(snapshot.contains("↑↓ choose"));
    }

    #[test]
    fn ask_user_escape_and_ctrl_c_have_distinct_cancellation_contracts() {
        let mut state = AskUserQuestionOverlayState::new(request(false));
        let escape = apply_ask_user_question_key(
            &mut state,
            &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(matches!(
            escape,
            AskUserQuestionKeyAction::Respond {
                outcome: AskUserResponseOutcome::Cancelled {
                    reason: AskUserCancellationReason::ClientClosed
                },
                interrupt_turn: false,
                ..
            }
        ));

        let interrupt = apply_ask_user_question_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(matches!(
            interrupt,
            AskUserQuestionKeyAction::Respond {
                outcome: AskUserResponseOutcome::Cancelled {
                    reason: AskUserCancellationReason::Interrupt
                },
                interrupt_turn: true,
                ..
            }
        ));
    }

    #[test]
    fn ask_user_overlay_queues_requests_in_arrival_order() {
        let mut first_request = request(false);
        first_request.request_id = "ask-1".into();
        let mut second_request = request(false);
        second_request.request_id = "ask-2".into();
        let mut third_request = request(false);
        third_request.request_id = "ask-3".into();
        let mut state = AskUserQuestionOverlayState::new(first_request);
        state.enqueue(second_request);
        state.enqueue(third_request);
        let mut second = state.take_next_queued().expect("second request");
        assert_eq!(second.request.request_id, "ask-2");
        let third = second.take_next_queued().expect("third request");
        assert_eq!(third.request.request_id, "ask-3");
    }
}
