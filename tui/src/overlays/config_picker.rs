use anyhow::Result;

use crate::commands::effort::{next_effort_choice, set_effort_override_message};
use crate::editor_mode::editor_mode_next_setting;
use crate::state::TuiState;
use crate::tui_theme::{output_style_label, theme_label};

use super::*;

const CONFIG_PICKER_VISIBLE_ROWS: usize = 19;

#[derive(Clone, Debug)]
pub(crate) struct ConfigPickerState {
    pub(crate) command: String,
    pub(crate) output_style: String,
    pub(crate) all_options: Vec<ConfigOption>,
    pub(crate) options: Vec<ConfigOption>,
    pub(crate) selected: usize,
    pub(crate) query: String,
    pub(crate) searching: bool,
    pub(crate) lines_cache: ConfigPickerLinesCache,
}

pub(crate) type ConfigPickerLinesCache = LinesCache<ConfigPickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigPickerLinesCacheKey {
    width: usize,
    selected: usize,
    query: String,
    options: Vec<ConfigOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigOption {
    pub(crate) label: String,
    pub(crate) value: String,
    pub(crate) description: String,
    pub(crate) current: bool,
    pub(crate) action: ConfigAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigAction {
    Readonly,
    OpenModelPicker,
    OpenThemePicker,
    OpenOutputStylePicker,
    CycleEffort,
    ToggleEditorMode,
}

pub(crate) enum ConfigPickerKeyAction {
    None,
    Close,
    Config {
        command: String,
        action: ConfigAction,
    },
}

impl TuiState {
    pub(crate) async fn open_config_picker(
        &mut self,
        command: &str,
        app_server: &AppClient,
    ) -> Result<()> {
        let output_style = app_server.output_style_setting().await?.style;
        self.overlay = Some(OverlayState::ConfigPicker(
            ConfigPickerState::new_async(command, app_server, self.editor_mode, output_style)
                .await?,
        ));
        self.set_status_line("Config: type to search, Space change, Enter save, Esc cancel.");
        Ok(())
    }

    pub(crate) async fn apply_config_action(
        &mut self,
        app_server: &AppClient,
        command: impl Into<String>,
        action: ConfigAction,
    ) -> Result<()> {
        let command = command.into();
        match action {
            ConfigAction::Readonly => {
                self.set_status_line("This setting is not wired in Rust yet.");
            }
            ConfigAction::OpenModelPicker => {
                self.open_model_picker(&command, app_server).await?;
            }
            ConfigAction::OpenThemePicker => {
                self.open_theme_picker(&command, app_server).await?;
            }
            ConfigAction::OpenOutputStylePicker => {
                self.open_output_style_picker(&command, app_server).await?;
            }
            ConfigAction::CycleEffort => {
                let current_effort = app_server.effort_level().await?.effort;
                let next = next_effort_choice(current_effort).as_effort();
                let message =
                    set_effort_override_message(app_server, &self.session_id, next).await?;
                self.refresh_status_effort(app_server).await;
                if let Some(OverlayState::ConfigPicker(picker)) = self.overlay.as_mut() {
                    picker
                        .refresh_from_app_async(app_server, self.editor_mode)
                        .await?;
                }
                self.push_local_slash_command_output(command, message.clone(), None);
                self.set_status_line(message);
            }
            ConfigAction::ToggleEditorMode => {
                let message = self
                    .set_editor_mode_setting(app_server, editor_mode_next_setting(self.editor_mode))
                    .await?;
                if let Some(OverlayState::ConfigPicker(picker)) = self.overlay.as_mut() {
                    picker
                        .refresh_from_app_async(app_server, self.editor_mode)
                        .await?;
                }
                self.push_local_slash_command_output(command, message.clone(), None);
                self.set_status_line(message);
            }
        }
        Ok(())
    }
}

impl ConfigPickerState {
    pub(crate) async fn new_async(
        command: impl Into<String>,
        app_server: &AppClient,
        editor_mode: EditorMode,
        output_style: String,
    ) -> Result<Self> {
        let model_name = app_server.model_name().await?.model_name;
        let theme_result = app_server.theme_setting().await?;
        let theme = ThemeSetting::parse(&theme_result.theme).unwrap_or(ThemeSetting::Auto);
        let current_effort = app_server.effort_level().await?.effort;
        let options = config_options(
            model_name,
            theme,
            current_effort,
            editor_mode,
            &output_style,
        );
        let mut picker = Self {
            command: command.into(),
            output_style,
            all_options: options.clone(),
            options,
            selected: 0,
            query: String::new(),
            searching: true,
            lines_cache: ConfigPickerLinesCache::default(),
        };
        picker.refresh();
        Ok(picker)
    }

    pub(crate) async fn refresh_from_app_async(
        &mut self,
        app_server: &AppClient,
        editor_mode: EditorMode,
    ) -> Result<()> {
        let model_name = app_server.model_name().await?.model_name;
        let theme_result = app_server.theme_setting().await?;
        let theme = ThemeSetting::parse(&theme_result.theme).unwrap_or(ThemeSetting::Auto);
        let current_effort = app_server.effort_level().await?.effort;
        self.all_options = config_options(
            model_name,
            theme,
            current_effort,
            editor_mode,
            &self.output_style,
        );
        self.refresh();
        Ok(())
    }

    fn refresh(&mut self) {
        let query = self.query.trim().to_ascii_lowercase();
        self.options = if query.is_empty() {
            self.all_options.clone()
        } else {
            self.all_options
                .iter()
                .filter(|option| {
                    option.label.to_ascii_lowercase().contains(&query)
                        || option.value.to_ascii_lowercase().contains(&query)
                        || option.description.to_ascii_lowercase().contains(&query)
                })
                .cloned()
                .collect()
        };
        if self.selected >= self.options.len() {
            self.selected = self.options.len().saturating_sub(1);
        }
    }

    pub(crate) fn push_query_char(&mut self, character: char) {
        self.query.push(character);
        self.refresh();
    }

    fn pop_query_char(&mut self) {
        self.query.pop();
        self.refresh();
    }

    fn clear_query(&mut self) {
        self.query.clear();
        self.refresh();
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = ConfigPickerLinesCacheKey {
            width,
            selected: self.selected,
            query: self.query.clone(),
            options: self.options.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || config_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

pub(crate) fn apply_config_picker_key(
    picker: &mut ConfigPickerState,
    key_event: &KeyEvent,
) -> ConfigPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => ConfigPickerKeyAction::Close,
        KeyCode::Char('/') if !picker.searching || picker.query.is_empty() => {
            picker.searching = true;
            ConfigPickerKeyAction::None
        }
        KeyCode::Backspace if picker.searching => {
            picker.pop_query_char();
            ConfigPickerKeyAction::None
        }
        KeyCode::Char('u')
            if picker.searching && key_event.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            picker.clear_query();
            ConfigPickerKeyAction::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            picker
                .options
                .get(picker.selected)
                .map_or(ConfigPickerKeyAction::None, |option| {
                    ConfigPickerKeyAction::Config {
                        command: picker.command.clone(),
                        action: option.action,
                    }
                })
        }
        KeyCode::Char(character)
            if picker.searching
                && !key_event.modifiers.contains(KeyModifiers::CONTROL)
                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
        {
            picker.push_query_char(character);
            ConfigPickerKeyAction::None
        }
        KeyCode::Up | KeyCode::Char('k' | 'j') | KeyCode::Down | KeyCode::Home | KeyCode::End => {
            SelectedIndex::new(&mut picker.selected, picker.options.len()).apply_key(
                key_event.code,
                None,
                true,
            );
            ConfigPickerKeyAction::None
        }
        _ => ConfigPickerKeyAction::None,
    }
}

pub(crate) fn config_options(
    model_name: String,
    theme: ThemeSetting,
    effort: Option<EffortLevel>,
    editor_mode: EditorMode,
    output_style: &str,
) -> Vec<ConfigOption> {
    let effort_choice = EffortChoice::from_effort(effort);
    vec![
        readonly_config_option("Auto-compact", "true"),
        readonly_config_option("Show tips", "true"),
        readonly_config_option("Reduce motion", "false"),
        readonly_config_option("Thinking mode", "true"),
        readonly_config_option("Prompt suggestions", "false"),
        readonly_config_option("Session recap", "true"),
        readonly_config_option("Rewind code (checkpoints)", "true"),
        readonly_config_option("Verbose output", "false"),
        readonly_config_option("Terminal progress bar", "true"),
        readonly_config_option("Show turn duration", "true"),
        readonly_config_option("Default permission mode", "Default"),
        readonly_config_option("Use auto mode during plan", "true"),
        readonly_config_option("Respect .gitignore in file picker", "true"),
        readonly_config_option("Skip the /copy picker", "false"),
        readonly_config_option("Auto-update channel", "latest"),
        ConfigOption {
            label: "Theme".to_string(),
            value: theme_label(theme).to_string(),
            description: "Choose terminal text style".to_string(),
            current: theme != ThemeSetting::Auto,
            action: ConfigAction::OpenThemePicker,
        },
        readonly_config_option("Local notifications", "Auto"),
        ConfigOption {
            label: "Output style".to_string(),
            value: output_style_label(output_style).to_string(),
            description: "Choose assistant response style".to_string(),
            current: output_style != "default",
            action: ConfigAction::OpenOutputStylePicker,
        },
        readonly_config_option("Language", "Default (English)"),
        ConfigOption {
            label: "Editor mode".to_string(),
            value: editor_mode_value(editor_mode).to_string(),
            description: "Toggle normal or vim prompt editing".to_string(),
            current: matches!(editor_mode, EditorMode::Insert | EditorMode::Normal),
            action: ConfigAction::ToggleEditorMode,
        },
        ConfigOption {
            label: "Model".to_string(),
            value: model_name,
            description: "Select model".to_string(),
            current: false,
            action: ConfigAction::OpenModelPicker,
        },
        ConfigOption {
            label: "Effort".to_string(),
            value: effort_choice.display_label().to_string(),
            description: "Cycle session-local effort".to_string(),
            current: effort.is_some(),
            action: ConfigAction::CycleEffort,
        },
    ]
}

fn readonly_config_option(label: &str, value: &str) -> ConfigOption {
    ConfigOption {
        label: label.to_string(),
        value: value.to_string(),
        description: "Not wired in Rust yet".to_string(),
        current: false,
        action: ConfigAction::Readonly,
    }
}

pub(crate) fn config_picker_lines(picker: &ConfigPickerState, width: usize) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let box_width = width.saturating_sub(3).clamp(24, 80);
    let query_width = box_width.saturating_sub(4).max(1);
    let query = if picker.query.is_empty() {
        "Search settings…"
    } else {
        picker.query.as_str()
    };
    let query_style = if picker.query.is_empty() {
        muted
    } else {
        Style::default()
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" ╭", muted),
            Span::styled("─".repeat(box_width), muted),
            Span::styled("╮", muted),
        ]),
        Line::from(vec![
            Span::styled(" │ ⌕ ", muted),
            Span::styled(pad_or_truncate(query, query_width), query_style),
            Span::styled(" │", muted),
        ]),
        Line::from(vec![
            Span::styled(" ╰", muted),
            Span::styled("─".repeat(box_width), muted),
            Span::styled("╯", muted),
        ]),
        Line::default(),
    ];
    if picker.options.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  ", muted),
            Span::styled("No settings found.", muted),
        ]));
        lines.push(Line::default());
        lines.push(config_picker_footer_line());
        return lines;
    }

    let visible_count = picker.options.len().min(CONFIG_PICKER_VISIBLE_ROWS);
    let start = slash_command_view_start(picker.selected, picker.options.len(), visible_count);
    let label_width = config_picker_label_width(&picker.options, width);
    let value_width = config_picker_value_width(&picker.options, width, label_width);
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
                let value = if option.current {
                    format!("{} ✔", option.value)
                } else {
                    option.value.clone()
                };
                let style = if selected { Style::default() } else { muted };
                Line::from(vec![
                    Span::styled("  ", muted),
                    Span::styled(marker.to_string(), style),
                    Span::styled(pad_or_truncate(&option.label, label_width), style),
                    Span::styled("  ", muted),
                    Span::styled(pad_or_truncate(&value, value_width), style),
                ])
            }),
    );
    if start + visible_count < picker.options.len() {
        lines.push(Line::from(vec![
            Span::styled("  ↓ ", muted),
            Span::styled(
                format!(
                    "{} more below",
                    picker.options.len() - start - visible_count
                ),
                muted,
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(config_picker_footer_line());
    lines
}

fn config_picker_footer_line() -> StyledLine {
    Line::from(Span::styled(
        "  Type to search · Space to change · Enter to save · Esc to cancel",
        empty_transcript_placeholder_style(),
    ))
}

fn config_picker_label_width(options: &[ConfigOption], width: usize) -> usize {
    options
        .iter()
        .map(|option| display_width_str(&option.label))
        .max()
        .unwrap_or(1)
        .min(width.saturating_sub(12).max(1))
}

fn config_picker_value_width(options: &[ConfigOption], width: usize, label_width: usize) -> usize {
    options
        .iter()
        .map(|option| {
            let extra = if option.current { 2 } else { 0 };
            display_width_str(&option.value) + extra
        })
        .max()
        .unwrap_or(1)
        .min(width.saturating_sub(label_width).saturating_sub(12).max(1))
}
