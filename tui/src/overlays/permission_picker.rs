use anyhow::Result;
use orbcode_app_server_client::{PermissionMode, SessionControlState};

use crate::numeric::saturating_u16;
use crate::render::styled_wrap::wrap_styled_line;
use crate::state::TuiState;

use super::*;

const PERMISSION_PRESET_VISIBLE_ROWS: usize = 3;
const PERMISSION_LABEL_COLUMN_WIDTH: usize = 31;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum InteractivePermissionMode {
    #[default]
    AskForApproval,
    ApproveForMe,
    FullAccess,
    Plan,
}

impl InteractivePermissionMode {
    const ALL: [Self; 4] = [
        Self::AskForApproval,
        Self::ApproveForMe,
        Self::FullAccess,
        Self::Plan,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AskForApproval => "Ask for approval",
            Self::ApproveForMe => "Approve for me",
            Self::FullAccess => "Full Access",
            Self::Plan => "Plan",
        }
    }

    pub(crate) fn from_active_preset(preset: Option<ModelPermissionPreset>) -> Self {
        match preset {
            Some(ModelPermissionPreset::AskForApproval) => Self::AskForApproval,
            Some(ModelPermissionPreset::ApproveForMe) => Self::ApproveForMe,
            Some(ModelPermissionPreset::FullAccess) => Self::FullAccess,
            None => Self::Plan,
        }
    }

    pub(crate) fn from_controls(controls: &SessionControlState) -> Self {
        controls.active_permission_preset.map_or_else(
            || match controls.permission_mode {
                PermissionMode::Default => Self::AskForApproval,
                PermissionMode::Auto => Self::ApproveForMe,
                PermissionMode::BypassPermissions => Self::FullAccess,
                PermissionMode::Plan => Self::Plan,
            },
            |preset| Self::from_active_preset(Some(preset)),
        )
    }

    pub(crate) fn next_available(self, options: &[PermissionPresetOption]) -> Self {
        let current = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        for offset in 1..=Self::ALL.len() {
            let candidate = Self::ALL[(current + offset) % Self::ALL.len()];
            if candidate != Self::FullAccess
                || options.iter().any(|option| {
                    option.value == ModelPermissionPreset::FullAccess
                        && option.disabled_reason.is_none()
                })
            {
                return candidate;
            }
        }
        self
    }

    fn preset(self) -> Option<ModelPermissionPreset> {
        match self {
            Self::AskForApproval => Some(ModelPermissionPreset::AskForApproval),
            Self::ApproveForMe => Some(ModelPermissionPreset::ApproveForMe),
            Self::FullAccess => Some(ModelPermissionPreset::FullAccess),
            Self::Plan => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PermissionPickerState {
    pub(crate) command: String,
    pub(crate) options: Vec<PermissionPresetOption>,
    pub(crate) selected: usize,
    pub(crate) confirming_full_access: bool,
    pub(crate) lines_cache: PermissionPickerLinesCache,
}

pub(crate) type PermissionPickerLinesCache = LinesCache<PermissionPickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionPickerLinesCacheKey {
    width: usize,
    selected: usize,
    confirming_full_access: bool,
    options: Vec<PermissionPresetOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PermissionPickerKeyAction {
    None,
    Close,
    Status(String),
    SetPreset {
        command: String,
        preset: ModelPermissionPreset,
    },
}

impl PermissionPickerState {
    pub(crate) fn new(command: impl Into<String>, options: Vec<PermissionPresetOption>) -> Self {
        let selected = options
            .iter()
            .position(|option| option.current)
            .unwrap_or(0);
        Self {
            command: command.into(),
            options,
            selected,
            confirming_full_access: false,
            lines_cache: PermissionPickerLinesCache::default(),
        }
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = PermissionPickerLinesCacheKey {
            width,
            selected: self.selected,
            confirming_full_access: self.confirming_full_access,
            options: self.options.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || permission_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

impl TuiState {
    pub(crate) fn open_permission_picker(
        &mut self,
        command: &str,
        result: PermissionPresetsResult,
    ) {
        self.overlay = Some(OverlayState::PermissionPicker(PermissionPickerState::new(
            command,
            result.options,
        )));
        self.set_status_line("Select a permission preset: Enter confirm, Esc cancel.");
    }

    pub(crate) async fn finish_permission_preset_selection(
        &mut self,
        app_server: &AppClient,
        _command: impl Into<String>,
        preset: ModelPermissionPreset,
    ) -> Result<()> {
        let controls = app_server
            .set_session_permission_preset(&self.session_id, preset)
            .await?;
        self.status.permission_mode = InteractivePermissionMode::from_controls(&controls);
        self.overlay = None;
        self.clear_status_line();
        Ok(())
    }

    pub(crate) async fn refresh_permission_mode(&mut self, app_server: &AppClient) {
        if let Ok(controls) = app_server.session_control_state(&self.session_id).await {
            self.status.permission_mode = InteractivePermissionMode::from_controls(&controls);
        }
    }

    pub(crate) async fn cycle_permission_mode(&mut self, app_server: &AppClient) -> Result<()> {
        let controls = app_server.session_control_state(&self.session_id).await?;
        let current = InteractivePermissionMode::from_controls(&controls);
        self.status.permission_mode = current;
        let next = if self.status.permission_mode_cycle_started {
            current.next_available(&controls.permission_presets)
        } else {
            InteractivePermissionMode::AskForApproval
        };

        let controls = if let Some(preset) = next.preset() {
            app_server
                .set_session_permission_preset(&self.session_id, preset)
                .await?
        } else {
            app_server
                .set_session_permission_mode(&self.session_id, PermissionMode::Plan)
                .await?
        };
        self.status.permission_mode = InteractivePermissionMode::from_controls(&controls);
        self.status.permission_mode_cycle_started = true;
        self.clear_status_line();
        Ok(())
    }
}

pub(crate) fn apply_permission_picker_key(
    picker: &mut PermissionPickerState,
    key_event: &KeyEvent,
) -> PermissionPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => {
            if picker.confirming_full_access {
                picker.confirming_full_access = false;
                PermissionPickerKeyAction::Status("Full Access confirmation cancelled.".to_string())
            } else {
                PermissionPickerKeyAction::Close
            }
        }
        KeyCode::Up
        | KeyCode::Char('k' | 'j')
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            let previous = picker.selected;
            SelectedIndex::new(&mut picker.selected, picker.options.len()).apply_key(
                key_event.code,
                Some(PERMISSION_PRESET_VISIBLE_ROWS),
                true,
            );
            if picker.selected != previous {
                picker.confirming_full_access = false;
            }
            PermissionPickerKeyAction::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            let Some(option) = picker.options.get(picker.selected) else {
                return PermissionPickerKeyAction::None;
            };
            if let Some(reason) = &option.disabled_reason {
                return PermissionPickerKeyAction::Status(reason.clone());
            }
            if option.value == ModelPermissionPreset::FullAccess && !picker.confirming_full_access {
                picker.confirming_full_access = true;
                return PermissionPickerKeyAction::Status(
                    "Full Access disables filesystem and network sandbox boundaries. Press Enter again to confirm."
                        .to_string(),
                );
            }
            PermissionPickerKeyAction::SetPreset {
                command: picker.command.clone(),
                preset: option.value,
            }
        }
        _ => PermissionPickerKeyAction::None,
    }
}

pub(crate) fn permission_picker_lines(
    picker: &PermissionPickerState,
    width: usize,
) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let mut lines = vec![
        Line::from(Span::styled(
            "Update Model Permissions",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];

    for (index, option) in picker.options.iter().enumerate() {
        let selected = index == picker.selected;
        let marker = if selected { "  ❯ " } else { "    " };
        let current = if option.current { " (current)" } else { "" };
        let disabled = option
            .disabled_reason
            .as_ref()
            .map(|_| " (disabled)")
            .unwrap_or_default();
        let label = format!("{}. {}{current}{disabled}", index + 1, option.label);
        let label_padding =
            " ".repeat(PERMISSION_LABEL_COLUMN_WIDTH.saturating_sub(display_width_str(&label)));
        let style = if selected && option.disabled_reason.is_none() {
            Style::default()
        } else {
            muted
        };
        append_wrapped_picker_spans(
            &mut lines,
            marker,
            "    ",
            vec![
                Span::styled(label, style.add_modifier(Modifier::BOLD)),
                Span::styled(label_padding, muted),
                Span::styled(option.description.clone(), muted),
            ],
            width,
        );
        if selected && let Some(reason) = &option.disabled_reason {
            append_wrapped_picker_line(
                &mut lines,
                "      ",
                "      ",
                reason,
                warning_style(),
                width,
            );
        }
    }

    lines.push(Line::default());
    if picker.confirming_full_access {
        append_wrapped_picker_line(
            &mut lines,
            "  ",
            "  ",
            "Warning: Full Access can read/write outside the workspace and use the network.",
            warning_style().add_modifier(Modifier::BOLD),
            width,
        );
        append_wrapped_picker_line(
            &mut lines,
            "  ",
            "  ",
            "Press Enter again to confirm · Esc to go back",
            warning_style(),
            width,
        );
    } else {
        append_wrapped_picker_line(
            &mut lines,
            "  ",
            "  ",
            "Enter to confirm · Esc to cancel",
            muted,
            width,
        );
    }
    lines
}

fn append_wrapped_picker_line(
    lines: &mut Vec<StyledLine>,
    first_prefix: &str,
    continuation_prefix: &str,
    text: &str,
    style: Style,
    width: usize,
) {
    append_wrapped_picker_spans(
        lines,
        first_prefix,
        continuation_prefix,
        vec![Span::styled(text.to_string(), style)],
        width,
    );
}

fn append_wrapped_picker_spans(
    lines: &mut Vec<StyledLine>,
    first_prefix: &str,
    continuation_prefix: &str,
    spans: Vec<Span<'static>>,
    width: usize,
) {
    let prefix_width = display_width_str(first_prefix).max(display_width_str(continuation_prefix));
    let content_width = width.saturating_sub(prefix_width).max(1);
    for (index, line) in wrap_styled_line(&Line::from(spans), content_width)
        .into_iter()
        .enumerate()
    {
        let prefix = if index == 0 {
            first_prefix
        } else {
            continuation_prefix
        };
        let mut spans = vec![Span::raw(prefix.to_string())];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
}

pub(crate) fn permission_picker_dialog_inner_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

pub(crate) fn permission_picker_outer_height(inner_line_count: usize) -> u16 {
    saturating_u16(inner_line_count.saturating_add(2))
}

pub(crate) fn permission_picker_dialog_inner_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

pub(crate) fn permission_picker_dialog_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Permissions ")
}

pub(crate) fn permission_picker_cursor(
    _picker: &PermissionPickerState,
    _area: Rect,
) -> Option<(u16, u16)> {
    None
}
