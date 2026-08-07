use anyhow::Result;

use crate::commands::effort::set_effort_override_message;
use crate::state::TuiState;

use super::*;

const MODEL_PICKER_VISIBLE_ROWS: usize = 6;

pub(crate) struct ModelPickerState {
    pub(crate) command: String,
    pub(crate) options: Vec<ModelOption>,
    pub(crate) selected: usize,
    pub(crate) effort: EffortChoice,
    pub(crate) effort_touched: bool,
    pub(crate) lines_cache: ModelPickerLinesCache,
}

pub(crate) type ModelPickerLinesCache = LinesCache<ModelPickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelPickerLinesCacheKey {
    width: usize,
    selected: usize,
    effort: EffortChoice,
    options: Vec<ModelOption>,
}

pub(crate) enum ModelPickerKeyAction {
    None,
    Close,
    SetModel {
        command: String,
        model: Option<String>,
        effort: EffortOverrideSelection,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffortChoice {
    Auto,
    Level(EffortLevel),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffortCycleDirection {
    Left,
    Right,
}

impl ModelPickerState {
    pub(crate) fn new(
        command: impl Into<String>,
        options: Vec<ModelOption>,
        effort: Option<EffortLevel>,
    ) -> Self {
        let selected = options
            .iter()
            .position(|option| option.current)
            .unwrap_or(0);
        Self {
            command: command.into(),
            options,
            selected,
            effort: EffortChoice::from_effort(effort),
            effort_touched: false,
            lines_cache: ModelPickerLinesCache::default(),
        }
    }

    fn cycle_effort(&mut self, direction: EffortCycleDirection) {
        self.effort = self.effort.cycle(direction);
        self.effort_touched = true;
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = ModelPickerLinesCacheKey {
            width,
            selected: self.selected,
            effort: self.effort,
            options: self.options.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || model_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

impl EffortChoice {
    pub(crate) fn from_effort(effort: Option<EffortLevel>) -> Self {
        match effort {
            Some(effort) => Self::Level(effort),
            None => Self::Auto,
        }
    }

    pub(crate) fn as_effort(self) -> Option<EffortLevel> {
        match self {
            Self::Auto => None,
            Self::Level(effort) => Some(effort),
        }
    }

    pub(crate) fn cycle(self, direction: EffortCycleDirection) -> Self {
        let choices = [
            Self::Auto,
            Self::Level(EffortLevel::Low),
            Self::Level(EffortLevel::Medium),
            Self::Level(EffortLevel::High),
            Self::Level(EffortLevel::Max),
        ];
        let index = choices
            .iter()
            .position(|choice| *choice == self)
            .unwrap_or(0);
        match direction {
            EffortCycleDirection::Left => choices[index.saturating_sub(1)],
            EffortCycleDirection::Right => choices[(index + 1).min(choices.len() - 1)],
        }
    }

    pub(crate) fn display_label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Level(EffortLevel::Low) => "Low",
            Self::Level(EffortLevel::Medium) => "Medium",
            Self::Level(EffortLevel::High) => "High",
            Self::Level(EffortLevel::Max) => "xHigh",
            Self::Level(_) => "Custom",
        }
    }
}

impl TuiState {
    pub(crate) async fn open_model_picker(
        &mut self,
        command: &str,
        app_server: &AppClient,
    ) -> Result<()> {
        let controls = app_server.session_control_state(&self.session_id).await?;
        let options: Vec<ModelOption> = controls
            .model_options
            .iter()
            .map(|o| ModelOption {
                value: o.value.clone(),
                label: o.label.clone(),
                description: o.description.clone(),
                current: o.current,
            })
            .collect();
        let effort = controls.effort_level;
        self.overlay = Some(OverlayState::ModelPicker(ModelPickerState::new(
            command, options, effort,
        )));
        self.set_status_line("Select model: Enter confirm, ←/→ effort, Esc cancel.");
        Ok(())
    }

    pub(crate) async fn finish_model_selection(
        &mut self,
        app_server: &AppClient,
        command: impl Into<String>,
        model: Option<String>,
        effort: EffortOverrideSelection,
    ) -> Result<()> {
        let selected_default = model.is_none();
        let result = app_server
            .set_session_model(&self.session_id, model)
            .await?;
        let display_name = result.model_selection.resolution.display_name.clone();
        let effort_message = match effort {
            Some(effort) => {
                Some(set_effort_override_message(app_server, &self.session_id, effort).await?)
            }
            None => None,
        };
        self.model_display_name = display_name;
        self.overlay = None;
        self.refresh_status_effort(app_server).await;

        let mut model_name = result.model_selection.resolution.request_model;
        if selected_default {
            model_name.push_str(" (default)");
        }
        let summary = format!("Set model to {model_name}");
        self.push_local_slash_command_output(command, summary.clone(), effort_message.clone());
        self.set_status_line(match effort_message {
            Some(effort_message) => format!("{summary}. {effort_message}"),
            None => format!("{summary}."),
        });
        Ok(())
    }
}

pub(crate) fn apply_model_picker_key(
    picker: &mut ModelPickerState,
    key_event: &KeyEvent,
) -> ModelPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => ModelPickerKeyAction::Close,
        KeyCode::Up
        | KeyCode::Char('k' | 'j')
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            SelectedIndex::new(&mut picker.selected, picker.options.len()).apply_key(
                key_event.code,
                Some(MODEL_PICKER_VISIBLE_ROWS),
                true,
            );
            ModelPickerKeyAction::None
        }
        KeyCode::Left | KeyCode::Char('h') => {
            picker.cycle_effort(EffortCycleDirection::Left);
            ModelPickerKeyAction::None
        }
        KeyCode::Right | KeyCode::Char('l') => {
            picker.cycle_effort(EffortCycleDirection::Right);
            ModelPickerKeyAction::None
        }
        KeyCode::Enter => {
            picker
                .options
                .get(picker.selected)
                .map_or(ModelPickerKeyAction::None, |option| {
                    ModelPickerKeyAction::SetModel {
                        command: picker.command.clone(),
                        model: option.value.clone(),
                        effort: picker.effort_touched.then_some(picker.effort.as_effort()),
                    }
                })
        }
        _ => ModelPickerKeyAction::None,
    }
}

pub(crate) fn model_picker_lines(picker: &ModelPickerState, width: usize) -> Vec<StyledLine> {
    if picker.options.is_empty() {
        return Vec::new();
    }

    let muted = empty_transcript_placeholder_style();
    let mut lines = vec![
        Line::from(Span::styled(
            "Select model",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            truncate_display_width(
                "  Switch between models. Applies to this session and future Orb Code sessions.",
                width.max(1),
            ),
            muted,
        )),
        Line::default(),
    ];
    let visible_count = picker.options.len().min(MODEL_PICKER_VISIBLE_ROWS);
    let start = slash_command_view_start(picker.selected, picker.options.len(), visible_count);
    let label_width = model_picker_label_width(&picker.options, width);
    lines.extend(
        picker
            .options
            .iter()
            .skip(start)
            .take(visible_count)
            .enumerate()
            .map(|(index, option)| {
                let absolute_index = start + index;
                let selected = absolute_index == picker.selected;
                let marker = if selected { "❯ " } else { "  " };
                let label = if option.current {
                    format!("{} ✔", option.label)
                } else {
                    option.label.clone()
                };
                let padded_label = pad_or_truncate(&label, label_width.max(1));
                let prefix = format!("{marker}{}.", absolute_index + 1);
                let description_width = width
                    .saturating_sub(label_width)
                    .saturating_sub(display_width_str(&prefix))
                    .saturating_sub(5)
                    .max(1);
                let description = truncate_chars(&option.description, description_width);
                let style = if selected { Style::default() } else { muted };
                Line::from(vec![
                    Span::styled("  ", muted),
                    Span::styled(prefix, style),
                    Span::styled(" ", muted),
                    Span::styled(padded_label, style),
                    Span::styled("  ", muted),
                    Span::styled(description, style),
                ])
            }),
    );
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("  ◉ ", Style::default()),
        Span::styled(
            format!("{} effort", picker.effort.display_label()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ← → to adjust", muted),
    ]));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Enter to confirm · Esc to cancel",
        muted,
    )));
    lines
}

fn model_picker_label_width(options: &[ModelOption], width: usize) -> usize {
    let longest = options
        .iter()
        .map(|option| {
            let extra = if option.current { 2 } else { 0 };
            display_width_str(&option.label) + extra
        })
        .max()
        .unwrap_or(1);
    longest.min(width.saturating_sub(8).max(1))
}
