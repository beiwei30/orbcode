use anyhow::Result;

use crate::state::TuiState;
use crate::tui_theme::{output_style_label, palette_for_theme, set_active_theme};

use super::*;

const THEME_PICKER_VISIBLE_ROWS: usize = 8;
const OUTPUT_STYLE_PICKER_VISIBLE_ROWS: usize = 10;

#[derive(Clone, Debug)]
pub(crate) struct ThemePickerState {
    pub(crate) command: String,
    pub(crate) options: Vec<ThemeOption>,
    pub(crate) selected: usize,
    pub(crate) lines_cache: ThemePickerLinesCache,
}

pub(crate) type ThemePickerLinesCache = LinesCache<ThemePickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThemePickerLinesCacheKey {
    width: usize,
    selected: usize,
    options: Vec<ThemeOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThemeOption {
    pub(crate) label: &'static str,
    pub(crate) value: Option<ThemeSetting>,
    pub(crate) current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct OutputStylePickerState {
    pub(crate) command: String,
    pub(crate) options: Vec<OutputStyleOption>,
    pub(crate) selected: usize,
    pub(crate) locked: bool,
    pub(crate) lines_cache: OutputStylePickerLinesCache,
}

pub(crate) type OutputStylePickerLinesCache = LinesCache<OutputStylePickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OutputStylePickerLinesCacheKey {
    width: usize,
    selected: usize,
    locked: bool,
    options: Vec<OutputStyleOption>,
}

pub(crate) enum ThemePickerKeyAction {
    None,
    Close,
    SetTheme {
        command: String,
        theme: Option<ThemeSetting>,
    },
}

pub(crate) enum OutputStylePickerKeyAction {
    None,
    Close,
    SetOutputStyle { command: String, style: String },
}

impl ThemePickerState {
    pub(crate) fn new(command: impl Into<String>, current: ThemeSetting) -> Self {
        let options = theme_options(current);
        let selected = options
            .iter()
            .position(|option| option.current)
            .unwrap_or(0);
        Self {
            command: command.into(),
            options,
            selected,
            lines_cache: ThemePickerLinesCache::default(),
        }
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = ThemePickerLinesCacheKey {
            width,
            selected: self.selected,
            options: self.options.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || theme_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

impl OutputStylePickerState {
    pub(crate) fn new(
        command: impl Into<String>,
        options: Vec<OutputStyleOption>,
        locked: bool,
    ) -> Self {
        let selected = options
            .iter()
            .position(|option| option.current)
            .unwrap_or(0);
        Self {
            command: command.into(),
            options,
            selected,
            locked,
            lines_cache: OutputStylePickerLinesCache::default(),
        }
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = OutputStylePickerLinesCacheKey {
            width,
            selected: self.selected,
            locked: self.locked,
            options: self.options.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || output_style_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

impl TuiState {
    pub(crate) async fn open_theme_picker(
        &mut self,
        command: &str,
        app_server: &AppClient,
    ) -> Result<()> {
        let theme_val = app_server.theme_setting().await?;
        let theme = ThemeSetting::parse(theme_val["theme"].as_str().unwrap_or("auto"))
            .unwrap_or(ThemeSetting::Auto);
        self.overlay = Some(OverlayState::ThemePicker(ThemePickerState::new(
            command, theme,
        )));
        self.set_status_line("Theme: Enter to select, Esc to cancel.");
        Ok(())
    }

    pub(crate) async fn open_output_style_picker(
        &mut self,
        command: &str,
        app_server: &AppClient,
    ) -> Result<()> {
        let options_val = app_server.output_style_options().await?;
        let options: Vec<OutputStyleOption> = options_val
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|o| OutputStyleOption {
                value: o["value"].as_str().unwrap_or("").to_string(),
                label: o["label"].as_str().unwrap_or("").to_string(),
                description: o["description"].as_str().unwrap_or("").to_string(),
                current: o["current"].as_bool().unwrap_or(false),
            })
            .collect();
        let locked = app_server.is_setting_locked("outputStyle").await?;
        self.overlay = Some(OverlayState::OutputStylePicker(
            OutputStylePickerState::new(command, options, locked),
        ));
        if locked {
            self.set_status_line("Output style (managed-locked): Esc to close.");
        } else {
            self.set_status_line("Output style: Enter to select, Esc to cancel.");
        }
        Ok(())
    }

    pub(crate) async fn finish_theme_selection(
        &mut self,
        app_server: &AppClient,
        command: impl Into<String>,
        theme: Option<ThemeSetting>,
    ) -> Result<()> {
        let command = command.into();
        let Some(theme) = theme else {
            self.set_status_line("Custom themes are not supported in Rust yet.");
            return Ok(());
        };
        let result = app_server.set_theme_setting(theme.as_str()).await?;
        let applied_theme = ThemeSetting::parse(result["theme"].as_str().unwrap_or("auto"))
            .unwrap_or(ThemeSetting::Auto);
        set_active_theme(applied_theme);
        self.overlay = None;
        let summary = format!("Theme set to {}", applied_theme.as_str());
        self.push_local_slash_command_output(command, summary.clone(), None);
        self.set_status_line(format!("{summary}."));
        Ok(())
    }

    pub(crate) async fn finish_output_style_selection(
        &mut self,
        app_server: &AppClient,
        command: impl Into<String>,
        style: String,
    ) -> Result<()> {
        let command = command.into();
        let requested = style.clone();
        let _ = app_server.set_output_style_setting(&style).await?;
        self.overlay = None;
        let matched = app_server.active_output_style_matched().await?;
        let summary = if matched {
            let name = app_server.active_output_style_name().await?;
            format!("Set output style to {}", output_style_label(&name))
        } else {
            format!("Unknown output style '{requested}', using default")
        };
        self.push_local_slash_command_output(command, summary.clone(), None);
        self.set_status_line(summary);
        Ok(())
    }
}

pub(crate) fn apply_theme_picker_key(
    picker: &mut ThemePickerState,
    key_event: &KeyEvent,
) -> ThemePickerKeyAction {
    match key_event.code {
        KeyCode::Esc => ThemePickerKeyAction::Close,
        KeyCode::Up
        | KeyCode::Char('k' | 'j')
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            SelectedIndex::new(&mut picker.selected, picker.options.len()).apply_key(
                key_event.code,
                Some(THEME_PICKER_VISIBLE_ROWS),
                true,
            );
            ThemePickerKeyAction::None
        }
        KeyCode::Enter => {
            picker
                .options
                .get(picker.selected)
                .map_or(ThemePickerKeyAction::None, |option| {
                    ThemePickerKeyAction::SetTheme {
                        command: picker.command.clone(),
                        theme: option.value,
                    }
                })
        }
        _ => ThemePickerKeyAction::None,
    }
}

pub(crate) fn apply_output_style_picker_key(
    picker: &mut OutputStylePickerState,
    key_event: &KeyEvent,
) -> OutputStylePickerKeyAction {
    match key_event.code {
        KeyCode::Esc => OutputStylePickerKeyAction::Close,
        KeyCode::Up
        | KeyCode::Char('k' | 'j')
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            SelectedIndex::new(&mut picker.selected, picker.options.len()).apply_key(
                key_event.code,
                Some(OUTPUT_STYLE_PICKER_VISIBLE_ROWS),
                true,
            );
            OutputStylePickerKeyAction::None
        }
        KeyCode::Enter => {
            if picker.locked {
                return OutputStylePickerKeyAction::None;
            }
            picker
                .options
                .get(picker.selected)
                .map_or(OutputStylePickerKeyAction::None, |option| {
                    OutputStylePickerKeyAction::SetOutputStyle {
                        command: picker.command.clone(),
                        style: option.value.clone(),
                    }
                })
        }
        _ => OutputStylePickerKeyAction::None,
    }
}

fn theme_options(current: ThemeSetting) -> Vec<ThemeOption> {
    [
        ("Auto (match terminal)", Some(ThemeSetting::Auto)),
        ("Dark mode", Some(ThemeSetting::Dark)),
        ("Light mode", Some(ThemeSetting::Light)),
        (
            "Dark mode (colorblind-friendly)",
            Some(ThemeSetting::DarkDaltonized),
        ),
        (
            "Light mode (colorblind-friendly)",
            Some(ThemeSetting::LightDaltonized),
        ),
        ("Dark mode (ANSI colors only)", Some(ThemeSetting::DarkAnsi)),
        (
            "Light mode (ANSI colors only)",
            Some(ThemeSetting::LightAnsi),
        ),
        ("New custom theme…", None),
    ]
    .into_iter()
    .map(|(label, value)| ThemeOption {
        label,
        value,
        current: value == Some(current),
    })
    .collect()
}

pub(crate) fn theme_picker_lines(picker: &ThemePickerState, width: usize) -> Vec<StyledLine> {
    if picker.options.is_empty() {
        return Vec::new();
    }

    let muted = empty_transcript_placeholder_style();
    let mut lines = vec![
        Line::from(Span::styled(
            "Theme",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        Line::from(Span::styled(
            "  Choose the text style that looks best with your terminal",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];
    let visible_count = picker.options.len().min(THEME_PICKER_VISIBLE_ROWS);
    let start = slash_command_view_start(picker.selected, picker.options.len(), visible_count);
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
                    option.label.to_string()
                };
                let style = if selected { Style::default() } else { muted };
                Line::from(vec![
                    Span::styled("  ", muted),
                    Span::styled(marker.to_string(), style),
                    Span::styled(format!("{}.", absolute_index + 1), style),
                    Span::styled(" ", muted),
                    Span::styled(label, style),
                ])
            }),
    );
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        theme_preview_separator(width),
        muted,
    )));
    let preview_theme = picker
        .options
        .get(picker.selected)
        .and_then(|option| option.value)
        .unwrap_or_else(|| {
            picker
                .options
                .iter()
                .find_map(|option| option.current.then_some(option.value).flatten())
                .unwrap_or(ThemeSetting::Auto)
        });
    lines.extend(theme_preview_diff_lines(width, preview_theme));
    lines.push(Line::from(Span::styled(
        theme_preview_separator(width),
        muted,
    )));
    lines.push(Line::from(Span::styled(
        "  Syntax theme: ANSI (ctrl+t unavailable in Rust)",
        muted,
    )));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Enter to select · Esc to cancel",
        muted,
    )));
    lines
}

fn theme_preview_separator(width: usize) -> String {
    "╌".repeat(width.clamp(1, 80))
}

fn theme_preview_diff_lines(width: usize, theme: ThemeSetting) -> Vec<StyledLine> {
    let available_width = width.clamp(24, 80);
    let palette = palette_for_theme(theme);
    [
        DiffRenderLine {
            old_line: Some(1),
            new_line: Some(1),
            marker: ' ',
            content: "function greet() {".to_string(),
            kind: DiffLineKind::Context,
        },
        DiffRenderLine {
            old_line: Some(2),
            new_line: None,
            marker: '-',
            content: "  console.log(\"Hello, World!\");".to_string(),
            kind: DiffLineKind::Removed,
        },
        DiffRenderLine {
            old_line: None,
            new_line: Some(2),
            marker: '+',
            content: "  console.log(\"Hello, Claude!\");".to_string(),
            kind: DiffLineKind::Added,
        },
        DiffRenderLine {
            old_line: Some(3),
            new_line: Some(3),
            marker: ' ',
            content: "}".to_string(),
            kind: DiffLineKind::Context,
        },
    ]
    .iter()
    .map(|line| render_diff_line_with_palette(line, false, 2, available_width, None, "js", palette))
    .collect()
}

pub(crate) fn output_style_picker_lines(
    picker: &OutputStylePickerState,
    width: usize,
) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let title = if picker.locked {
        "Preferred output style (managed-locked)"
    } else {
        "Preferred output style"
    };
    let mut lines = vec![
        Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];
    if picker.locked {
        lines.push(Line::from(Span::styled(
            "  This setting is managed by your organization and cannot be changed.",
            muted,
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  This changes how Orb Code communicates with you",
            muted,
        )));
    }
    lines.push(Line::default());
    if picker.options.is_empty() {
        lines.push(Line::from(Span::styled("  No output styles found.", muted)));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  Enter to select · Esc to cancel",
            muted,
        )));
        return lines;
    }

    let visible_count = picker.options.len().min(OUTPUT_STYLE_PICKER_VISIBLE_ROWS);
    let start = slash_command_view_start(picker.selected, picker.options.len(), visible_count);
    let label_width = output_style_label_width(&picker.options, width);
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
                    Span::styled(pad_or_truncate(&label, label_width), style),
                    Span::styled("  ", muted),
                    Span::styled(description, style),
                ])
            }),
    );
    lines.push(Line::default());
    if picker.locked {
        lines.push(Line::from(Span::styled("  Esc to close", muted)));
    } else {
        lines.push(Line::from(Span::styled(
            "  Enter to select · Esc to cancel",
            muted,
        )));
    }
    lines
}

fn output_style_label_width(options: &[OutputStyleOption], width: usize) -> usize {
    options
        .iter()
        .map(|option| {
            let extra = if option.current { 2 } else { 0 };
            display_width_str(&option.label) + extra
        })
        .max()
        .unwrap_or(1)
        .min(width.saturating_sub(8).max(1))
}
