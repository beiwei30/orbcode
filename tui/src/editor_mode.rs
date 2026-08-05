use anyhow::Result;
use orbcode_app_server_client::{AppClient, EditorModeSetting};

use crate::bottom_pane::vim::VimRuntimeState;
use crate::state::TuiState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorMode {
    Standard,
    Insert,
    Normal,
}

pub(crate) fn editor_mode_from_setting(setting: EditorModeSetting) -> EditorMode {
    match setting {
        EditorModeSetting::Normal => EditorMode::Standard,
        EditorModeSetting::Vim => EditorMode::Insert,
    }
}

fn editor_mode_setting_from_state(mode: EditorMode) -> EditorModeSetting {
    match mode {
        EditorMode::Standard => EditorModeSetting::Normal,
        EditorMode::Insert | EditorMode::Normal => EditorModeSetting::Vim,
    }
}

pub(crate) fn editor_mode_value(mode: EditorMode) -> &'static str {
    editor_mode_setting_from_state(mode).as_str()
}

pub(crate) fn editor_mode_next_setting(mode: EditorMode) -> EditorModeSetting {
    match editor_mode_setting_from_state(mode) {
        EditorModeSetting::Normal => EditorModeSetting::Vim,
        EditorModeSetting::Vim => EditorModeSetting::Normal,
    }
}

impl TuiState {
    pub(crate) async fn set_editor_mode_setting(
        &mut self,
        app_server: &AppClient,
        mode: EditorModeSetting,
    ) -> Result<String> {
        let result = app_server.set_editor_mode_setting(mode.as_str()).await?;
        let mode = EditorModeSetting::parse(&result.editor_mode).unwrap_or(mode);
        self.editor_mode = editor_mode_from_setting(mode);
        self.normal_pending = None;
        self.normal_count = None;
        self.vim_state = VimRuntimeState::default();
        Ok(match mode {
            EditorModeSetting::Vim => {
                "Editor mode set to vim. Use Escape key to toggle between INSERT and NORMAL modes."
                    .to_string()
            }
            EditorModeSetting::Normal => {
                "Editor mode set to normal. Using standard keyboard bindings.".to_string()
            }
        })
    }
}
