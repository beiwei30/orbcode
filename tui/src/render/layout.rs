use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
};

use crate::bottom_pane::input_layout::{
    InputView, build_input_view, build_input_view_with_tail_pin, max_input_inner_height,
};
use crate::custom_terminal::Frame;
use crate::numeric::saturating_u16;
use crate::overlays::{
    OverlayState, draw_fullscreen_overlay, draw_overlay_after_layout, overlay_request_panel,
    permission_panel_area_for_overlay, permission_panel_desired_height,
    permission_picker_dialog_block, permission_picker_outer_height,
};
use crate::render::styled_wrap::{transcript_layout_constraint, wrap_styled_lines};
use crate::render::text_utils::{StyledLine, styled_line_display_width};
use crate::state::TuiState;
use crate::tui_theme::subtle_style;

/// Width of the `❯ ` input prompt prefix. The input text and the footer status
/// bar are both indented by this amount so they line up in the same column.
const FOOTER_STATUS_INDENT: u16 = 2;

impl TuiState {
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if let Some(overlay) = self.overlay.as_mut()
            && draw_fullscreen_overlay(frame, overlay, area)
        {
            return;
        }

        let input_inner_width = area.width.saturating_sub(3).max(1) as usize;
        let request_panel_width = area.width.max(1) as usize;
        let permission_overlay_open =
            matches!(self.overlay, Some(OverlayState::PermissionRequest(_)));
        let cwd = self.cwd.clone();
        let overlay_request_panel =
            overlay_request_panel(self.overlay.as_mut(), &cwd, request_panel_width);
        let permission_picker_open = overlay_request_panel
            .as_ref()
            .is_some_and(|panel| panel.use_permission_picker_block);
        let task_panel_lines = if !permission_overlay_open && overlay_request_panel.is_none() {
            self.request_status_lines_for_width(request_panel_width)
        } else {
            Vec::new()
        };
        let slash_suggestion_lines = if !permission_overlay_open
            && overlay_request_panel.is_none()
            && !self.request_in_flight
            && task_panel_lines.is_empty()
        {
            self.cached_slash_command_suggestion_lines(request_panel_width)
                .to_vec()
        } else {
            Vec::new()
        };
        let request_status_lines = if permission_overlay_open {
            Vec::new()
        } else if let Some(panel) = overlay_request_panel {
            panel.lines
        } else if !task_panel_lines.is_empty() {
            task_panel_lines
        } else if !slash_suggestion_lines.is_empty() {
            slash_suggestion_lines
        } else {
            Vec::new()
        };
        let request_status_height = if permission_picker_open {
            permission_picker_outer_height(request_status_lines.len())
        } else {
            saturating_u16(request_status_lines.len())
        };
        let input_view = build_input_view_with_tail_pin(
            &self.input,
            self.input_cursor,
            input_inner_width,
            max_input_inner_height(
                area.height,
                request_status_height
                    .saturating_add(saturating_u16(self.prompt_followup_line_count())),
            ),
            self.input_tail_pinned,
        );
        let layout = self.main_layout_regions(area, &input_view, request_status_height);
        self.input_area = layout[4];
        self.clamp_input_selection();
        let mut transcript_area = layout[0];
        let permission_panel_area =
            permission_panel_area_for_overlay(self.overlay.as_mut(), layout[0]);
        if let Some(panel_area) = permission_panel_area {
            transcript_area.height = panel_area.y.saturating_sub(transcript_area.y);
        }

        let transcript_view = self.visible_transcript_lines_for_view(
            transcript_area.width as usize,
            transcript_area.height as usize,
            self.history_flushed_message_count == 0,
        );
        self.transcript_ui.viewport.sync_with_window(
            transcript_area,
            transcript_view.visible_lines,
            transcript_view.all_lines,
            transcript_view.all_lines_start,
            transcript_view.all_line_count,
            transcript_view.selection_lines,
            transcript_view.selection_lines_start,
            transcript_view.visible_row_start,
            transcript_view.actual_scroll,
            transcript_view.max_scroll,
        );
        let transcript = Paragraph::new(self.transcript_ui.viewport.render_lines());
        frame.render_widget(transcript, transcript_area);

        if request_status_height > 0 {
            if permission_picker_open {
                frame.render_widget(Clear, layout[1]);
                frame.render_widget(
                    Paragraph::new(request_status_lines)
                        .block(permission_picker_dialog_block())
                        .wrap(Wrap { trim: false }),
                    layout[1],
                );
            } else {
                frame.render_widget(
                    Paragraph::new(request_status_lines).wrap(Wrap { trim: false }),
                    layout[1],
                );
            }
        }
        if self.prompt_followup_line_count() > 0 {
            frame.render_widget(
                Paragraph::new(self.followup_prompt_lines(input_inner_width)),
                layout[2],
            );
        }
        frame.render_widget(Paragraph::new(divider_line(area.width)), layout[3]);
        frame.render_widget(Paragraph::new(self.prompt_lines(&input_view)), layout[4]);
        frame.render_widget(Paragraph::new(divider_line(area.width)), layout[5]);

        let hide_footer = self.overlay_hides_footer();
        // The status bar (model · cwd) is indented to align with the input text
        // after the `❯ ` prompt; the transient request hint (e.g.
        // `esc to interrupt`), when present, sits at the right edge.
        let status_line = if hide_footer {
            Line::default()
        } else {
            self.footer_right_line()
        };
        let hint_line = if hide_footer {
            Line::default()
        } else {
            self.footer_left_line()
        };
        let hint_width =
            saturating_u16(styled_line_display_width(&hint_line).min(usize::from(area.width)));
        let footer_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(FOOTER_STATUS_INDENT),
                Constraint::Min(8),
                Constraint::Length(hint_width),
            ])
            .split(layout[6]);
        frame.render_widget(
            Paragraph::new(vec![status_line]).wrap(Wrap { trim: false }),
            footer_layout[1],
        );
        frame.render_widget(
            Paragraph::new(vec![hint_line]).wrap(Wrap { trim: false }),
            footer_layout[2],
        );

        if self.overlay.is_none() {
            frame.set_cursor_position((
                layout[4]
                    .x
                    .saturating_add(FOOTER_STATUS_INDENT)
                    .saturating_add(saturating_u16(input_view.cursor_col)),
                layout[4]
                    .y
                    .saturating_add(saturating_u16(input_view.cursor_row)),
            ));
        }

        if let Some(overlay) = self.overlay.as_mut() {
            draw_overlay_after_layout(frame, overlay, area, layout[1], permission_panel_area);
        }
    }

    pub(crate) fn desired_viewport_height(&mut self, width: u16, terminal_height: u16) -> u16 {
        if matches!(
            self.overlay,
            Some(
                OverlayState::Help(_)
                    | OverlayState::KeybindHelp(_)
                    | OverlayState::Diff(_)
                    | OverlayState::BackgroundJobs(_)
                    | OverlayState::TranscriptPager(_)
            )
        ) {
            return terminal_height;
        }
        let transcript_width = width.max(1) as usize;
        let input_inner_width = width.saturating_sub(3).max(1) as usize;
        let request_status_height = self.request_status_height_for_layout(transcript_width);
        let input_view = build_input_view(
            &self.input,
            self.input_cursor,
            input_inner_width,
            usize::MAX,
        );
        let transcript_height = wrap_styled_lines(
            &self.transcript_lines_for_messages(
                transcript_width,
                self.history_flushed_message_count == 0,
            ),
            transcript_width,
        )
        .len()
        .max(1);
        let transcript_height = saturating_u16(transcript_height);
        transcript_height
            .saturating_add(permission_panel_desired_height(
                self.overlay.as_mut(),
                transcript_width,
            ))
            .saturating_add(saturating_u16(input_view.lines.len().max(1)))
            .saturating_add(saturating_u16(self.prompt_followup_line_count()))
            .saturating_add(request_status_height)
            .saturating_add(3)
    }

    pub(crate) fn transcript_content_height(
        &mut self,
        transcript_width: usize,
        show_empty_placeholder: bool,
    ) -> u16 {
        let height = wrap_styled_lines(
            &self.transcript_lines_for_messages(transcript_width, show_empty_placeholder),
            transcript_width,
        )
        .len()
        .max(1);
        saturating_u16(height)
    }

    pub(crate) fn main_layout_regions(
        &mut self,
        area: Rect,
        input_view: &InputView,
        request_status_height: u16,
    ) -> Vec<Rect> {
        let input_height = saturating_u16(self.prompt_lines(input_view).len().max(1));
        let followup_height = saturating_u16(self.prompt_followup_line_count());
        let fixed_panel_height = request_status_height
            .saturating_add(followup_height)
            .saturating_add(input_height)
            .saturating_add(3);
        let transcript_budget = area.height.saturating_sub(fixed_panel_height);
        let transcript_content_height = self.transcript_content_height(
            area.width.max(1) as usize,
            self.history_flushed_message_count == 0,
        );
        let transcript_constraint = if matches!(
            self.overlay,
            Some(
                OverlayState::PermissionRequest(_)
                    | OverlayState::Help(_)
                    | OverlayState::KeybindHelp(_)
                    | OverlayState::Diff(_)
            )
        ) && transcript_budget > 0
        {
            Constraint::Length(transcript_budget)
        } else {
            transcript_layout_constraint(transcript_content_height, transcript_budget)
        };

        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                transcript_constraint,
                Constraint::Length(request_status_height),
                Constraint::Length(followup_height),
                Constraint::Length(1),
                Constraint::Length(input_height),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area)
            .to_vec()
    }
}

pub(crate) fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    area.width > 0
        && area.height > 0
        && column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

pub(crate) fn rect_contains_row(area: Rect, row: u16) -> bool {
    area.height > 0 && row >= area.y && row < area.y.saturating_add(area.height)
}

fn divider_line(width: u16) -> StyledLine {
    Line::from(Span::styled("─".repeat(width as usize), subtle_style()))
}
