use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;

use crate::commands::permissions::permission_scope_label;
use crate::history_cell::viewport::TranscriptViewportState;
use crate::overlays::{BackgroundJobsView, DiffOverlayMode, OverlayState};
use crate::render::layout::{rect_contains, rect_contains_row};
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn mouse_visible_state_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.status_line.hash(&mut hasher);
        self.input_area.x.hash(&mut hasher);
        self.input_area.y.hash(&mut hasher);
        self.input_area.width.hash(&mut hasher);
        self.input_area.height.hash(&mut hasher);
        match self.input_selection {
            Some(selection) => {
                true.hash(&mut hasher);
                selection.anchor.hash(&mut hasher);
                selection.focus.hash(&mut hasher);
            }
            None => false.hash(&mut hasher),
        }
        Self::hash_viewport_interaction_state(&self.transcript_ui.viewport, &mut hasher);
        match &self.overlay {
            None => 0_u8.hash(&mut hasher),
            Some(OverlayState::AddDirPicker(_)) => 13_u8.hash(&mut hasher),
            Some(OverlayState::SessionPicker(_)) => 1_u8.hash(&mut hasher),
            Some(OverlayState::ModelPicker(_)) => 2_u8.hash(&mut hasher),
            Some(OverlayState::ThemePicker(_)) => 3_u8.hash(&mut hasher),
            Some(OverlayState::OutputStylePicker(_)) => 4_u8.hash(&mut hasher),
            Some(OverlayState::ConfigPicker(_)) => 5_u8.hash(&mut hasher),
            Some(OverlayState::SandboxPicker(_)) => 6_u8.hash(&mut hasher),
            Some(OverlayState::MemoryPicker(_)) => 7_u8.hash(&mut hasher),
            Some(OverlayState::RewindPicker(picker)) => {
                12_u8.hash(&mut hasher);
                picker.selected.hash(&mut hasher);
                picker.entries.len().hash(&mut hasher);
            }
            Some(OverlayState::PermissionPicker(picker)) => {
                11_u8.hash(&mut hasher);
                picker.tab.hash(&mut hasher);
                picker.focus.hash(&mut hasher);
                picker.selected.hash(&mut hasher);
                picker.search_active.hash(&mut hasher);
                picker.search_query.hash(&mut hasher);
                if let Some(draft) = &picker.adding {
                    1_u8.hash(&mut hasher);
                    permission_scope_label(draft.scope).hash(&mut hasher);
                    draft.kind.as_str().hash(&mut hasher);
                    draft.rule.hash(&mut hasher);
                } else {
                    0_u8.hash(&mut hasher);
                }
                if let Some(destination) = &picker.add_destination {
                    1_u8.hash(&mut hasher);
                    destination.selected.hash(&mut hasher);
                    destination.draft.rule.hash(&mut hasher);
                    destination.draft.kind.as_str().hash(&mut hasher);
                    permission_scope_label(destination.draft.scope).hash(&mut hasher);
                } else {
                    0_u8.hash(&mut hasher);
                }
                if let Some(details) = &picker.rule_details {
                    1_u8.hash(&mut hasher);
                    details.selected.hash(&mut hasher);
                    details.rule.rule.hash(&mut hasher);
                    details.rule.kind.as_str().hash(&mut hasher);
                    details.rule.source.hash(&mut hasher);
                } else {
                    0_u8.hash(&mut hasher);
                }
            }
            Some(OverlayState::PermissionRequest(permission)) => {
                8_u8.hash(&mut hasher);
                permission.selected_option.hash(&mut hasher);
                permission.details_expanded.hash(&mut hasher);
                permission.panel_scroll.hash(&mut hasher);
                Self::hash_viewport_interaction_state(&permission.viewport, &mut hasher);
            }
            Some(OverlayState::Help(help)) => {
                9_u8.hash(&mut hasher);
                help.scroll.hash(&mut hasher);
                help.max_scroll.hash(&mut hasher);
            }
            Some(OverlayState::KeybindHelp(state)) => {
                13_u8.hash(&mut hasher);
                state.scroll.hash(&mut hasher);
                state.max_scroll.hash(&mut hasher);
            }
            Some(OverlayState::Diff(diff)) => {
                10_u8.hash(&mut hasher);
                match diff.mode {
                    DiffOverlayMode::Unstaged => 0_u8.hash(&mut hasher),
                    DiffOverlayMode::Staged => 1_u8.hash(&mut hasher),
                }
                diff.selected_file.hash(&mut hasher);
                diff.line_scroll.hash(&mut hasher);
                diff.max_line_scroll.hash(&mut hasher);
            }
            Some(OverlayState::BackgroundJobs(state)) => {
                14_u8.hash(&mut hasher);
                state.selected.hash(&mut hasher);
                state.scroll.hash(&mut hasher);
                (state.view == BackgroundJobsView::Detail).hash(&mut hasher);
            }
            Some(OverlayState::TranscriptPager(pager)) => {
                15_u8.hash(&mut hasher);
                pager.source_signature.hash(&mut hasher);
                pager.width.hash(&mut hasher);
                pager.loaded_cell_start.hash(&mut hasher);
                pager.loaded_cell_end.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    fn hash_viewport_interaction_state(
        viewport: &TranscriptViewportState,
        hasher: &mut impl Hasher,
    ) {
        viewport.area.x.hash(hasher);
        viewport.area.y.hash(hasher);
        viewport.area.width.hash(hasher);
        viewport.area.height.hash(hasher);
        viewport.current_scroll.hash(hasher);
        viewport.max_scroll.hash(hasher);
        viewport.visible_row_start.hash(hasher);
        viewport.all_line_count.hash(hasher);
        match viewport.selection.as_ref() {
            Some(selection) => {
                true.hash(hasher);
                selection.area.x.hash(hasher);
                selection.area.y.hash(hasher);
                selection.area.width.hash(hasher);
                selection.area.height.hash(hasher);
                selection.anchor.row.hash(hasher);
                selection.anchor.column.hash(hasher);
                selection.focus.row.hash(hasher);
                selection.focus.column.hash(hasher);
            }
            None => false.hash(hasher),
        }
    }

    pub(crate) fn handle_mouse(&mut self, mouse_event: MouseEvent) -> bool {
        let before = self.mouse_visible_state_signature();
        self.apply_mouse_event(mouse_event);
        self.mouse_visible_state_signature() != before
    }

    fn apply_mouse_event(&mut self, mouse_event: MouseEvent) {
        if matches!(self.overlay, Some(OverlayState::PermissionRequest(_))) {
            let permission_area = match &self.overlay {
                Some(OverlayState::PermissionRequest(permission)) => permission.viewport.area,
                _ => Rect::ZERO,
            };
            let over_permission_panel = matches!(
                &self.overlay,
                Some(OverlayState::PermissionRequest(permission))
                    if rect_contains(permission.viewport.area, mouse_event.column, mouse_event.row)
            );
            let over_permission_row = rect_contains_row(permission_area, mouse_event.row);
            let over_transcript_row =
                rect_contains_row(self.transcript_ui.viewport.area, mouse_event.row);
            let past_permission_top =
                permission_area.height > 0 && mouse_event.row >= permission_area.y;
            let before_permission =
                permission_area.height > 0 && mouse_event.row < permission_area.y;
            let has_permission_selection = self.has_permission_selection();
            let has_transcript_selection = self.has_transcript_selection();
            let mut status_line = None;
            let mut auto_copy_screen = false;
            match mouse_event.kind {
                MouseEventKind::ScrollUp => {
                    if over_permission_panel || over_permission_row {
                        if let Some(OverlayState::PermissionRequest(permission)) =
                            self.overlay.as_mut()
                        {
                            permission.viewport.clear_selection();
                            permission.panel_scroll = permission.panel_scroll.saturating_add(3);
                        }
                        status_line = Some("Browsing permission details. End returns to options.");
                    }
                }
                MouseEventKind::ScrollDown => {
                    if over_permission_panel || over_permission_row {
                        if let Some(OverlayState::PermissionRequest(permission)) =
                            self.overlay.as_mut()
                        {
                            permission.viewport.clear_selection();
                            permission.panel_scroll = permission.panel_scroll.saturating_sub(3);
                        }
                        status_line = Some("Browsing permission details. End returns to options.");
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if over_permission_row {
                        self.clear_transcript_selection();
                        if let Some(OverlayState::PermissionRequest(permission)) =
                            self.overlay.as_mut()
                        {
                            if let Some(point) =
                                permission.viewport.selection_point_from_mouse_clamped(
                                    mouse_event.column,
                                    mouse_event.row,
                                )
                            {
                                permission.viewport.begin_selection(point);
                            } else {
                                permission.viewport.clear_selection();
                            }
                        }
                    } else if over_transcript_row
                        && let Some(point) = self
                            .transcript_ui
                            .viewport
                            .selection_point_from_mouse_clamped(mouse_event.column, mouse_event.row)
                    {
                        self.clear_permission_selection();
                        self.transcript_ui.viewport.begin_selection(point);
                    } else {
                        self.clear_permission_selection();
                        self.clear_transcript_selection();
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    if has_permission_selection {
                        if before_permission {
                            if let Some(point) = self
                                .transcript_ui
                                .viewport
                                .selection_point_from_mouse_clamped(
                                    mouse_event.column,
                                    mouse_event.row,
                                )
                                && let Some(bottom) =
                                    self.transcript_ui.viewport.last_visible_selection_point()
                            {
                                self.transcript_ui.viewport.begin_selection(point);
                                self.transcript_ui.viewport.update_selection(bottom);
                            }
                            if let Some(OverlayState::PermissionRequest(permission)) =
                                self.overlay.as_mut()
                                && let Some(first) =
                                    permission.viewport.first_visible_selection_point()
                            {
                                permission.viewport.update_selection(first);
                            }
                        } else if let Some(OverlayState::PermissionRequest(permission)) =
                            self.overlay.as_mut()
                        {
                            permission.autoscroll_selection(&mouse_event);
                            if let Some(point) =
                                permission.viewport.selection_point_from_mouse_clamped(
                                    mouse_event.column,
                                    mouse_event.row,
                                )
                            {
                                permission.viewport.update_selection(point);
                            }
                        }
                    } else if has_transcript_selection {
                        if past_permission_top {
                            if let Some(bottom) =
                                self.transcript_ui.viewport.last_visible_selection_point()
                            {
                                self.transcript_ui.viewport.update_selection(bottom);
                            }
                            if let Some(OverlayState::PermissionRequest(permission)) =
                                self.overlay.as_mut()
                                && let Some(first) =
                                    permission.viewport.first_visible_selection_point()
                                && let Some(point) =
                                    permission.viewport.selection_point_from_mouse_clamped(
                                        mouse_event.column,
                                        mouse_event.row,
                                    )
                            {
                                if permission.viewport.selection.is_none() {
                                    permission.viewport.begin_selection(first);
                                }
                                permission.viewport.update_selection(point);
                                permission.autoscroll_selection(&mouse_event);
                            }
                        } else if over_transcript_row
                            && let Some(point) = self
                                .transcript_ui
                                .viewport
                                .selection_point_from_mouse_clamped(
                                    mouse_event.column,
                                    mouse_event.row,
                                )
                        {
                            self.transcript_ui.viewport.update_selection(point);
                        }
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    if has_permission_selection {
                        if before_permission {
                            if let Some(point) = self
                                .transcript_ui
                                .viewport
                                .selection_point_from_mouse_clamped(
                                    mouse_event.column,
                                    mouse_event.row,
                                )
                                && let Some(bottom) =
                                    self.transcript_ui.viewport.last_visible_selection_point()
                            {
                                self.transcript_ui.viewport.begin_selection(point);
                                self.transcript_ui.viewport.update_selection(bottom);
                            }
                            if let Some(OverlayState::PermissionRequest(permission)) =
                                self.overlay.as_mut()
                                && let Some(first) =
                                    permission.viewport.first_visible_selection_point()
                            {
                                permission.viewport.update_selection(first);
                            }
                        } else if let Some(OverlayState::PermissionRequest(permission)) =
                            self.overlay.as_mut()
                        {
                            permission.autoscroll_selection(&mouse_event);
                            if let Some(point) =
                                permission.viewport.selection_point_from_mouse_clamped(
                                    mouse_event.column,
                                    mouse_event.row,
                                )
                            {
                                permission.viewport.update_selection(point);
                            }
                        }
                        auto_copy_screen = true;
                    } else if has_transcript_selection {
                        if past_permission_top {
                            if let Some(bottom) =
                                self.transcript_ui.viewport.last_visible_selection_point()
                            {
                                self.transcript_ui.viewport.update_selection(bottom);
                            }
                            if let Some(OverlayState::PermissionRequest(permission)) =
                                self.overlay.as_mut()
                                && let Some(first) =
                                    permission.viewport.first_visible_selection_point()
                                && let Some(point) =
                                    permission.viewport.selection_point_from_mouse_clamped(
                                        mouse_event.column,
                                        mouse_event.row,
                                    )
                            {
                                if permission.viewport.selection.is_none() {
                                    permission.viewport.begin_selection(first);
                                }
                                permission.viewport.update_selection(point);
                                permission.autoscroll_selection(&mouse_event);
                            }
                        } else if over_transcript_row
                            && let Some(point) = self
                                .transcript_ui
                                .viewport
                                .selection_point_from_mouse_clamped(
                                    mouse_event.column,
                                    mouse_event.row,
                                )
                        {
                            self.transcript_ui.viewport.update_selection(point);
                        }
                        auto_copy_screen = true;
                    }
                }
                _ => {}
            }
            if let Some(status_line) = status_line {
                self.set_status_line(status_line);
            }
            if auto_copy_screen {
                self.auto_copy_screen_selection();
            }
            return;
        }
        if let Some(OverlayState::Help(help)) = self.overlay.as_mut() {
            match mouse_event.kind {
                MouseEventKind::ScrollUp => {
                    help.scroll = help.scroll.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    help.scroll = help.scroll.saturating_add(3);
                }
                _ => {}
            }
            return;
        }
        if let Some(OverlayState::KeybindHelp(state)) = self.overlay.as_mut() {
            match mouse_event.kind {
                MouseEventKind::ScrollUp => {
                    state.scroll = state.scroll.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    state.scroll = state.scroll.saturating_add(3);
                }
                _ => {}
            }
            return;
        }
        if let Some(OverlayState::Diff(diff)) = self.overlay.as_mut() {
            match mouse_event.kind {
                MouseEventKind::ScrollUp => diff.scroll_lines(-3),
                MouseEventKind::ScrollDown => diff.scroll_lines(3),
                _ => {}
            }
            return;
        }
        if let Some(OverlayState::BackgroundJobs(state)) = self.overlay.as_mut() {
            match mouse_event.kind {
                MouseEventKind::ScrollUp => {
                    state.scroll = state.scroll.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    state.scroll = state.scroll.saturating_add(3);
                }
                _ => {}
            }
            return;
        }
        if self.overlay.is_some() {
            return;
        }
        match mouse_event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.clear_transcript_selection();
                if let Some(cursor) =
                    self.input_cursor_from_mouse(mouse_event.column, mouse_event.row)
                {
                    self.begin_input_selection(cursor);
                } else {
                    self.clear_input_selection();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.has_input_selection()
                    && let Some(cursor) =
                        self.input_cursor_from_mouse_clamped(mouse_event.column, mouse_event.row)
                {
                    self.update_input_selection(cursor);
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.has_input_selection() => {
                if let Some(cursor) =
                    self.input_cursor_from_mouse_clamped(mouse_event.column, mouse_event.row)
                {
                    self.update_input_selection(cursor);
                }
                self.auto_copy_input_selection();
            }
            _ => {}
        }
    }
}
