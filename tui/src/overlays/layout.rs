use super::*;
use crate::numeric::saturating_u16;
use crossterm::cursor::SetCursorStyle;

pub(crate) struct OverlayRequestPanel {
    pub(crate) lines: Vec<StyledLine>,
    pub(crate) use_permission_picker_block: bool,
}

impl OverlayRequestPanel {
    pub(crate) fn height(&self) -> u16 {
        if self.use_permission_picker_block {
            permission_picker_outer_height(self.lines.len())
        } else {
            saturating_u16(self.lines.len())
        }
    }
}

pub(crate) fn overlay_request_panel(
    overlay: Option<&mut OverlayState>,
    cwd: &Path,
    width: usize,
) -> Option<OverlayRequestPanel> {
    let overlay = overlay?;
    let lines = match overlay {
        OverlayState::AddDirPicker(picker) => picker.cached_lines(width).to_vec(),
        OverlayState::MemoryPicker(picker) => picker.cached_lines(cwd, width).to_vec(),
        OverlayState::SessionPicker(picker) => session_picker_lines(picker, width),
        OverlayState::RewindPicker(picker) => rewind_picker_lines(picker, width),
        OverlayState::ModelPicker(picker) => picker.cached_lines(width).to_vec(),
        OverlayState::ThemePicker(picker) => picker.cached_lines(width).to_vec(),
        OverlayState::OutputStylePicker(picker) => picker.cached_lines(width).to_vec(),
        OverlayState::ConfigPicker(picker) => picker.cached_lines(width).to_vec(),
        OverlayState::SandboxPicker(picker) => picker.cached_lines(width).to_vec(),
        OverlayState::PermissionPicker(picker) => picker
            .cached_lines(permission_picker_dialog_inner_width(width))
            .to_vec(),
        OverlayState::PermissionRequest(_)
        | OverlayState::AskUserQuestion(_)
        | OverlayState::Help(_)
        | OverlayState::KeybindHelp(_)
        | OverlayState::Diff(_)
        | OverlayState::BackgroundJobs(_)
        | OverlayState::TranscriptPager(_) => {
            return None;
        }
    };

    Some(OverlayRequestPanel {
        use_permission_picker_block: matches!(overlay, OverlayState::PermissionPicker(_)),
        lines,
    })
}

pub(crate) fn permission_panel_area_for_overlay(
    overlay: Option<&mut OverlayState>,
    transcript_area: Rect,
) -> Option<Rect> {
    let height = match overlay? {
        OverlayState::PermissionRequest(permission) => {
            let inner_width = transcript_area.width.saturating_sub(2).max(1) as usize;
            let cached = permission.cached_panel_content(inner_width);
            permission_panel_height_with_context_from_wrapped(cached.wrapped_body, transcript_area)
        }
        OverlayState::AskUserQuestion(state) => {
            ask_user_question_panel_height(state, transcript_area)
        }
        _ => return None,
    };
    Some(Rect {
        x: transcript_area.x,
        y: transcript_area.bottom().saturating_sub(height),
        width: transcript_area.width,
        height,
    })
}

pub(crate) fn permission_panel_desired_height(
    overlay: Option<&mut OverlayState>,
    transcript_width: usize,
) -> u16 {
    match overlay {
        Some(OverlayState::PermissionRequest(permission)) => {
            let inner_width = transcript_width.saturating_sub(2).max(1);
            let cached = permission.cached_panel_content(inner_width);
            permission_panel_full_height_from_wrapped(cached.wrapped_body)
        }
        Some(OverlayState::AskUserQuestion(state)) => {
            ask_user_question_panel_desired_height(state, transcript_width)
        }
        _ => 0,
    }
}

pub(crate) fn overlay_cursor_style(overlay: Option<&OverlayState>) -> Option<SetCursorStyle> {
    match overlay {
        Some(
            OverlayState::SessionPicker(_)
            | OverlayState::ThemePicker(_)
            | OverlayState::OutputStylePicker(_)
            | OverlayState::ConfigPicker(_)
            | OverlayState::SandboxPicker(_)
            | OverlayState::PermissionPicker(_)
            | OverlayState::AskUserQuestion(_),
        ) => Some(SetCursorStyle::BlinkingBar),
        Some(OverlayState::PermissionRequest(permission)) if permission.editing_rule => {
            Some(SetCursorStyle::BlinkingBar)
        }
        _ => None,
    }
}

pub(crate) fn overlay_should_hide_footer(overlay: Option<&OverlayState>) -> bool {
    matches!(overlay, Some(OverlayState::MemoryPicker(_)))
}

pub(crate) fn overlay_persists_after_turn(overlay: Option<&OverlayState>) -> bool {
    matches!(overlay, Some(OverlayState::TranscriptPager(_)))
}

pub(crate) fn draw_fullscreen_overlay(
    frame: &mut Frame,
    overlay: &mut OverlayState,
    area: Rect,
) -> bool {
    match overlay {
        OverlayState::TranscriptPager(pager) => {
            pager.sync_viewport(area);
            super::transcript_pager::draw_transcript_pager_overlay(frame, pager, area);
            true
        }
        _ => false,
    }
}

pub(crate) fn draw_overlay_after_layout(
    frame: &mut Frame,
    overlay: &mut OverlayState,
    area: Rect,
    request_status_area: Rect,
    permission_panel_area: Option<Rect>,
) {
    match overlay {
        OverlayState::PermissionRequest(permission) => {
            if let Some(panel_area) = permission_panel_area
                && let Some(cursor) = draw_permission_panel(frame, permission, panel_area)
            {
                frame.set_cursor_position(cursor);
            }
        }
        OverlayState::AskUserQuestion(state) => {
            if let Some(panel_area) = permission_panel_area {
                draw_ask_user_question_panel(frame, state, panel_area);
            }
        }
        OverlayState::Help(help) => {
            sync_help_overlay_bounds(help, area);
            draw_help_overlay(frame, help, area);
        }
        OverlayState::KeybindHelp(state) => {
            sync_keybind_help_overlay_bounds(state, area);
            draw_keybind_help_overlay(frame, state, area);
        }
        OverlayState::Diff(diff) => {
            sync_diff_overlay_bounds(diff, area);
            draw_diff_overlay(frame, diff, area);
        }
        OverlayState::BackgroundJobs(state) => {
            sync_background_jobs_overlay_bounds(state, area);
            draw_background_jobs_overlay(frame, state, area);
        }
        OverlayState::SessionPicker(picker) => {
            if let Some(cursor) = session_picker_cursor(picker, request_status_area) {
                frame.set_cursor_position(cursor);
            }
        }
        OverlayState::PermissionPicker(picker) => {
            if let Some(cursor) = permission_picker_cursor(
                picker,
                permission_picker_dialog_inner_area(request_status_area),
            ) {
                frame.set_cursor_position(cursor);
            }
        }
        OverlayState::TranscriptPager(_)
        | OverlayState::AddDirPicker(_)
        | OverlayState::ModelPicker(_)
        | OverlayState::ThemePicker(_)
        | OverlayState::OutputStylePicker(_)
        | OverlayState::ConfigPicker(_)
        | OverlayState::SandboxPicker(_)
        | OverlayState::MemoryPicker(_)
        | OverlayState::RewindPicker(_) => {
            // The transcript pager is drawn before main layout by draw_fullscreen_overlay.
            // The picker variants are rendered as the request-status panel above the prompt.
        }
    }
}
