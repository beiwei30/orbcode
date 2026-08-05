use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use orbcode_app_server_client::{
    AppClient, MemoryFileOverview, MemoryOverview, PermissionDecision, PermissionOverview,
    SandboxLocalSettings, SandboxSettingsUpdate, ThemeSetting, WorkspaceDiff,
};
use orbcode_config::{
    ModelOption, OutputStyleOption, PermissionRuleSettingKind, normalize_permission_rule_for_edit,
};
use orbcode_protocol::{EffortLevel, PermissionRequest, SessionSummary};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::commands::permissions::{PermissionRuleAction, PermissionRuleScope};
use crate::commands::utils::short_session_id;
use crate::editor_mode::{EditorMode, editor_mode_value};
use crate::history_cell::viewport::TranscriptViewportState;
use crate::line_cache::LinesCache;
use crate::pickers::SelectedIndex;
use crate::render::text_utils::{
    StyledLine, collapse_inline_whitespace, display_width, display_width_str, pad_or_truncate,
    truncate_chars, truncate_display_width,
};
use crate::slash_commands::{
    SlashCommandSource, fuzzy_match_score, slash_command_view_start, slash_commands,
    suggestion_scrollbar_active,
};
use crate::tui_theme::{
    TuiPalette, accent_style, active_palette, emphasis_style, empty_transcript_placeholder_style,
    highlight_style, inactive_style, subtle_style, warning_style,
};
use crate::{custom_terminal::Frame, syntax_highlight};
use serde_json::Value;

pub(crate) const HELP_OVERLAY_PAGE_STEP: usize = 8;

/// Three-state effort override from the model picker: untouched, explicit
/// automatic/default effort, or explicit effort level.
pub(crate) type EffortOverrideSelection = Option<Option<EffortLevel>>;

mod add_dir_picker;
mod appearance_picker;
mod background_jobs;
mod config_picker;
mod diff;
mod help;
mod key_handling;
mod keybind_help;
mod layout;
mod memory_picker;
mod model_picker;
mod permission_panel;
mod permission_picker;
mod rewind_picker;
mod sandbox_picker;
mod session_picker;
pub(crate) mod transcript_pager;

pub(crate) use add_dir_picker::*;
pub(crate) use appearance_picker::*;
pub(crate) use background_jobs::*;
pub(crate) use config_picker::*;
pub(crate) use diff::*;
pub(crate) use help::*;
pub(crate) use keybind_help::*;
pub(crate) use layout::*;
pub(crate) use memory_picker::*;
pub(crate) use model_picker::*;
pub(crate) use permission_panel::*;
pub(crate) use permission_picker::*;
pub(crate) use rewind_picker::*;
pub(crate) use sandbox_picker::*;
pub(crate) use session_picker::*;

#[allow(dead_code, clippy::large_enum_variant)]
pub(crate) enum OverlayState {
    AddDirPicker(AddDirPickerState),
    SessionPicker(SessionPickerState),
    ModelPicker(ModelPickerState),
    ThemePicker(ThemePickerState),
    OutputStylePicker(OutputStylePickerState),
    ConfigPicker(ConfigPickerState),
    SandboxPicker(SandboxPickerState),
    MemoryPicker(MemoryPickerState),
    PermissionPicker(PermissionPickerState),
    PermissionRequest(PermissionOverlayState),
    RewindPicker(RewindPickerState),
    Help(HelpOverlayState),
    KeybindHelp(KeybindHelpOverlayState),
    Diff(DiffOverlayState),
    BackgroundJobs(BackgroundJobsOverlayState),
    TranscriptPager(transcript_pager::TranscriptPagerState),
}

pub(crate) enum OverlayAction {
    None,
    AddDirectory {
        command: String,
        path: PathBuf,
    },
    Resume {
        command: String,
        session_id: String,
    },
    Fork {
        command: String,
        session_id: String,
    },
    SetModel {
        command: String,
        model: Option<String>,
        effort: EffortOverrideSelection,
    },
    SetTheme {
        command: String,
        theme: Option<ThemeSetting>,
    },
    SetOutputStyle {
        command: String,
        style: String,
    },
    Config {
        command: String,
        action: ConfigAction,
    },
    SetSandboxMode {
        command: String,
        choice: SandboxModeChoice,
    },
    SetSandboxOverride {
        command: String,
        choice: SandboxOverrideChoice,
    },
    EditMemory {
        command: String,
        path: PathBuf,
    },
    OpenPath {
        command: String,
        path: PathBuf,
    },
    PermissionRuleUpdate {
        command: String,
        action: PermissionRuleAction,
        scope: PermissionRuleScope,
        kind: PermissionRuleSettingKind,
        rule: String,
    },
    Permission {
        request_id: String,
        decision: PermissionDecision,
    },
    Rewind {
        command: String,
        session_id: String,
        keep_messages: usize,
        anchor_id: String,
        restore_prompt: String,
    },
    CancelBackgroundJob {
        job_id: String,
    },
}

pub(crate) fn highlight_code_line(line: &str, extension: &str) -> Vec<Span<'static>> {
    let lang = extension.trim_start_matches('.').to_ascii_lowercase();
    let Some(lines) = syntax_highlight::highlight_code_to_styled_spans(line, &lang) else {
        return vec![Span::styled(line.to_string(), inactive_style())];
    };
    lines
        .into_iter()
        .next()
        .unwrap_or_else(|| vec![Span::styled(line.to_string(), inactive_style())])
}
