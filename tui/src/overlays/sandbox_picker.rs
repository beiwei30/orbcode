use anyhow::Result;

use crate::render::markdown::wrap_inline_markdown_line;
use crate::state::TuiState;

use super::*;

#[derive(Clone, Debug)]
pub(crate) struct SandboxPickerState {
    pub(crate) command: String,
    pub(crate) settings: SandboxLocalSettings,
    pub(crate) tab: SandboxTab,
    pub(crate) mode_selected: usize,
    pub(crate) overrides_selected: usize,
    pub(crate) lines_cache: SandboxPickerLinesCache,
}

pub(crate) type SandboxPickerLinesCache = LinesCache<SandboxPickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SandboxPickerLinesCacheKey {
    width: usize,
    tab: SandboxTab,
    mode_selected: usize,
    overrides_selected: usize,
    settings: SandboxLocalSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SandboxTab {
    Mode,
    Overrides,
    Config,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SandboxModeChoice {
    AutoAllow,
    Regular,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SandboxOverrideChoice {
    AllowFallback,
    Strict,
}

pub(crate) enum SandboxPickerKeyAction {
    None,
    Close,
    SetSandboxMode {
        command: String,
        choice: SandboxModeChoice,
    },
    SetSandboxOverride {
        command: String,
        choice: SandboxOverrideChoice,
    },
}

impl TuiState {
    pub(crate) async fn open_sandbox_picker(
        &mut self,
        command: &str,
        app_server: &AppClient,
    ) -> Result<()> {
        let settings = app_server.sandbox_local_settings_typed().await?;
        self.overlay = Some(OverlayState::SandboxPicker(SandboxPickerState::new(
            command, settings,
        )));
        self.set_status_line("Sandbox: ←/→ tabs, Enter select, Esc quit.");
        Ok(())
    }

    pub(crate) async fn apply_sandbox_mode_choice(
        &mut self,
        app_server: &AppClient,
        command: impl Into<String>,
        choice: SandboxModeChoice,
    ) -> Result<()> {
        let command = command.into();
        app_server
            .update_sandbox_settings_typed(sandbox_mode_update(choice))
            .await?;
        let settings = app_server.sandbox_local_settings_typed().await?;
        if let Some(OverlayState::SandboxPicker(picker)) = self.overlay.as_mut() {
            picker.refresh_settings(settings);
        }
        let message = sandbox_mode_message(choice).to_string();
        self.push_local_slash_command_output(command, message.clone(), None);
        self.set_status_line(message);
        Ok(())
    }

    pub(crate) async fn apply_sandbox_override_choice(
        &mut self,
        app_server: &AppClient,
        command: impl Into<String>,
        choice: SandboxOverrideChoice,
    ) -> Result<()> {
        let command = command.into();
        app_server
            .update_sandbox_settings_typed(sandbox_override_update(choice))
            .await?;
        let settings = app_server.sandbox_local_settings_typed().await?;
        if let Some(OverlayState::SandboxPicker(picker)) = self.overlay.as_mut() {
            picker.refresh_settings(settings);
        }
        let message = sandbox_override_message(choice).to_string();
        self.push_local_slash_command_output(command, message.clone(), None);
        self.set_status_line(message);
        Ok(())
    }
}

impl SandboxPickerState {
    pub(crate) fn new(command: impl Into<String>, settings: SandboxLocalSettings) -> Self {
        let mode_selected = sandbox_mode_options()
            .iter()
            .position(|choice| *choice == sandbox_mode_choice(&settings))
            .unwrap_or(0);
        let overrides_selected = sandbox_override_options()
            .iter()
            .position(|choice| *choice == sandbox_override_choice(&settings))
            .unwrap_or(0);
        Self {
            command: command.into(),
            settings,
            tab: SandboxTab::Mode,
            mode_selected,
            overrides_selected,
            lines_cache: SandboxPickerLinesCache::default(),
        }
    }

    pub(crate) fn refresh_settings(&mut self, settings: SandboxLocalSettings) {
        self.settings = settings;
        self.mode_selected = sandbox_mode_options()
            .iter()
            .position(|choice| *choice == sandbox_mode_choice(&self.settings))
            .unwrap_or(0);
        self.overrides_selected = sandbox_override_options()
            .iter()
            .position(|choice| *choice == sandbox_override_choice(&self.settings))
            .unwrap_or(0);
    }

    fn selected_len(&self) -> usize {
        match self.tab {
            SandboxTab::Mode => sandbox_mode_options().len(),
            SandboxTab::Overrides if self.settings.enabled => sandbox_override_options().len(),
            SandboxTab::Overrides => 0,
            SandboxTab::Config => 0,
        }
    }

    fn selected_index_mut(&mut self) -> Option<&mut usize> {
        match self.tab {
            SandboxTab::Mode => Some(&mut self.mode_selected),
            SandboxTab::Overrides if self.settings.enabled => Some(&mut self.overrides_selected),
            SandboxTab::Overrides => None,
            SandboxTab::Config => None,
        }
    }

    fn previous_tab(&mut self) {
        self.tab = match self.tab {
            SandboxTab::Mode => SandboxTab::Config,
            SandboxTab::Overrides => SandboxTab::Mode,
            SandboxTab::Config => SandboxTab::Overrides,
        };
    }

    pub(crate) fn next_tab(&mut self) {
        self.tab = match self.tab {
            SandboxTab::Mode => SandboxTab::Overrides,
            SandboxTab::Overrides => SandboxTab::Config,
            SandboxTab::Config => SandboxTab::Mode,
        };
    }

    fn selected_mode(&self) -> SandboxModeChoice {
        sandbox_mode_options()
            .get(self.mode_selected)
            .copied()
            .unwrap_or(SandboxModeChoice::AutoAllow)
    }

    fn selected_override(&self) -> SandboxOverrideChoice {
        sandbox_override_options()
            .get(self.overrides_selected)
            .copied()
            .unwrap_or(SandboxOverrideChoice::AllowFallback)
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = SandboxPickerLinesCacheKey {
            width,
            tab: self.tab,
            mode_selected: self.mode_selected,
            overrides_selected: self.overrides_selected,
            settings: self.settings.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || sandbox_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

pub(crate) fn apply_sandbox_picker_key(
    picker: &mut SandboxPickerState,
    key_event: &KeyEvent,
) -> SandboxPickerKeyAction {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') => SandboxPickerKeyAction::Close,
        KeyCode::Left | KeyCode::Char('h') => {
            picker.previous_tab();
            SandboxPickerKeyAction::None
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
            picker.next_tab();
            SandboxPickerKeyAction::None
        }
        KeyCode::Up | KeyCode::Char('k' | 'j') | KeyCode::Down | KeyCode::Home | KeyCode::End => {
            let len = picker.selected_len();
            if let Some(selected) = picker.selected_index_mut() {
                SelectedIndex::new(selected, len).apply_key(key_event.code, None, true);
            }
            SandboxPickerKeyAction::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => match picker.tab {
            SandboxTab::Mode => SandboxPickerKeyAction::SetSandboxMode {
                command: picker.command.clone(),
                choice: picker.selected_mode(),
            },
            SandboxTab::Overrides if picker.settings.enabled => {
                SandboxPickerKeyAction::SetSandboxOverride {
                    command: picker.command.clone(),
                    choice: picker.selected_override(),
                }
            }
            SandboxTab::Overrides | SandboxTab::Config => SandboxPickerKeyAction::None,
        },
        _ => SandboxPickerKeyAction::None,
    }
}

fn sandbox_mode_options() -> [SandboxModeChoice; 3] {
    [
        SandboxModeChoice::AutoAllow,
        SandboxModeChoice::Regular,
        SandboxModeChoice::Disabled,
    ]
}

fn sandbox_override_options() -> [SandboxOverrideChoice; 2] {
    [
        SandboxOverrideChoice::AllowFallback,
        SandboxOverrideChoice::Strict,
    ]
}

fn sandbox_mode_choice(settings: &SandboxLocalSettings) -> SandboxModeChoice {
    if !settings.enabled {
        SandboxModeChoice::Disabled
    } else if settings.auto_allow_bash_if_sandboxed {
        SandboxModeChoice::AutoAllow
    } else {
        SandboxModeChoice::Regular
    }
}

fn sandbox_override_choice(settings: &SandboxLocalSettings) -> SandboxOverrideChoice {
    if settings.allow_unsandboxed_commands {
        SandboxOverrideChoice::AllowFallback
    } else {
        SandboxOverrideChoice::Strict
    }
}

fn sandbox_mode_label(choice: SandboxModeChoice) -> &'static str {
    match choice {
        SandboxModeChoice::AutoAllow => "Sandbox BashTool, with auto-allow",
        SandboxModeChoice::Regular => "Sandbox BashTool, with regular permissions",
        SandboxModeChoice::Disabled => "No Sandbox",
    }
}

fn sandbox_override_label(choice: SandboxOverrideChoice) -> &'static str {
    match choice {
        SandboxOverrideChoice::AllowFallback => "Allow unsandboxed fallback",
        SandboxOverrideChoice::Strict => "Strict sandbox mode",
    }
}

pub(crate) fn sandbox_mode_update(choice: SandboxModeChoice) -> SandboxSettingsUpdate {
    match choice {
        SandboxModeChoice::AutoAllow => SandboxSettingsUpdate {
            enabled: Some(true),
            auto_allow_bash_if_sandboxed: Some(true),
            allow_unsandboxed_commands: None,
        },
        SandboxModeChoice::Regular => SandboxSettingsUpdate {
            enabled: Some(true),
            auto_allow_bash_if_sandboxed: Some(false),
            allow_unsandboxed_commands: None,
        },
        SandboxModeChoice::Disabled => SandboxSettingsUpdate {
            enabled: Some(false),
            auto_allow_bash_if_sandboxed: Some(false),
            allow_unsandboxed_commands: None,
        },
    }
}

pub(crate) fn sandbox_override_update(choice: SandboxOverrideChoice) -> SandboxSettingsUpdate {
    SandboxSettingsUpdate {
        enabled: None,
        auto_allow_bash_if_sandboxed: None,
        allow_unsandboxed_commands: Some(matches!(choice, SandboxOverrideChoice::AllowFallback)),
    }
}

pub(crate) fn sandbox_mode_message(choice: SandboxModeChoice) -> &'static str {
    match choice {
        SandboxModeChoice::AutoAllow => "✓ Sandbox enabled with auto-allow for bash commands",
        SandboxModeChoice::Regular => "✓ Sandbox enabled with regular bash permissions",
        SandboxModeChoice::Disabled => "○ Sandbox disabled",
    }
}

pub(crate) fn sandbox_override_message(choice: SandboxOverrideChoice) -> &'static str {
    match choice {
        SandboxOverrideChoice::AllowFallback => {
            "✓ Unsandboxed fallback allowed - commands can run outside sandbox when necessary"
        }
        SandboxOverrideChoice::Strict => {
            "✓ Strict sandbox mode - all commands must run in sandbox or be excluded via the `excludedCommands` option"
        }
    }
}

pub(crate) fn sandbox_picker_lines(picker: &SandboxPickerState, width: usize) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let mut lines = vec![sandbox_tabs_line(picker.tab), Line::default()];
    match picker.tab {
        SandboxTab::Mode => {
            lines.push(Line::from(Span::styled(
                "  Configure Mode:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::default());
            let current = sandbox_mode_choice(&picker.settings);
            for (index, choice) in sandbox_mode_options().into_iter().enumerate() {
                let selected = index == picker.mode_selected;
                lines.push(sandbox_option_line(
                    index,
                    sandbox_mode_label(choice),
                    selected,
                    choice == current,
                ));
            }
            lines.push(Line::default());
            lines.push(Line::default());
            lines.extend(wrap_sandbox_text(
                "  Auto-allow mode: Commands will try to run in the sandbox automatically, and attempts to run outside of the sandbox fallback to regular permissions. Explicit ask/deny rules are always respected.",
                width,
                muted,
            ));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "  Learn more: https://code.claude.com/docs/en/sandboxing",
                muted,
            )));
        }
        SandboxTab::Overrides => {
            lines.push(Line::from(Span::styled(
                "  Configure Overrides:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::default());
            if !picker.settings.enabled {
                lines.push(Line::from(Span::styled(
                    "  Sandbox is not enabled. Enable sandbox to configure override settings.",
                    muted,
                )));
            } else {
                let current = sandbox_override_choice(&picker.settings);
                for (index, choice) in sandbox_override_options().into_iter().enumerate() {
                    let selected = index == picker.overrides_selected;
                    lines.push(sandbox_option_line(
                        index,
                        sandbox_override_label(choice),
                        selected,
                        choice == current,
                    ));
                }
                lines.push(Line::default());
                lines.push(Line::default());
                lines.extend(wrap_sandbox_text(
                    "  Allow unsandboxed fallback: When a command fails due to sandbox restrictions, Claude can retry outside the sandbox, falling back to default permissions.",
                    width,
                    muted,
                ));
                lines.push(Line::default());
                lines.extend(wrap_sandbox_text(
                    "  Strict sandbox mode: All bash commands invoked by the model must run in the sandbox unless they are explicitly listed in excludedCommands.",
                    width,
                    muted,
                ));
            }
        }
        SandboxTab::Config => {
            lines.push(Line::from(Span::styled(
                "  Config:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::default());
            let mode = sandbox_mode_label(sandbox_mode_choice(&picker.settings));
            let fallback = if picker.settings.allow_unsandboxed_commands {
                "allowed"
            } else {
                "strict"
            };
            push_sandbox_config_line(&mut lines, width, "Mode", mode, muted);
            push_sandbox_config_line(&mut lines, width, "Unsandboxed fallback", fallback, muted);
            push_sandbox_config_line(
                &mut lines,
                width,
                "Platform backend",
                sandbox_platform_backend_label(),
                muted,
            );
            let excluded = if picker.settings.excluded_commands.is_empty() {
                "None".to_string()
            } else {
                picker.settings.excluded_commands.join(", ")
            };
            push_sandbox_config_line(&mut lines, width, "Excluded Commands", &excluded, muted);
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "  Filesystem:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            push_sandbox_list_config_line(
                &mut lines,
                width,
                "Allow write",
                &picker.settings.filesystem.allow_write,
                muted,
            );
            push_sandbox_list_config_line(
                &mut lines,
                width,
                "Deny write",
                &picker.settings.filesystem.deny_write,
                muted,
            );
            push_sandbox_list_config_line(
                &mut lines,
                width,
                "Deny read",
                &picker.settings.filesystem.deny_read,
                muted,
            );
            push_sandbox_list_config_line(
                &mut lines,
                width,
                "Allow read",
                &picker.settings.filesystem.allow_read,
                muted,
            );
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "  Network:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            push_sandbox_list_config_line(
                &mut lines,
                width,
                "Allowed domains",
                &picker.settings.network.allowed_domains,
                muted,
            );
            push_sandbox_list_config_line(
                &mut lines,
                width,
                "Allowed Unix sockets",
                &picker.settings.network.allow_unix_sockets,
                muted,
            );
            push_sandbox_config_line(
                &mut lines,
                width,
                "Allow all Unix sockets",
                option_bool_label(picker.settings.network.allow_all_unix_sockets),
                muted,
            );
            push_sandbox_config_line(
                &mut lines,
                width,
                "Allow local binding",
                option_bool_label(picker.settings.network.allow_local_binding),
                muted,
            );
            push_sandbox_config_line(
                &mut lines,
                width,
                "HTTP proxy port",
                option_u64_label(picker.settings.network.http_proxy_port).as_str(),
                muted,
            );
            push_sandbox_config_line(
                &mut lines,
                width,
                "SOCKS proxy port",
                option_u64_label(picker.settings.network.socks_proxy_port).as_str(),
                muted,
            );
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  ←/→ tabs · Enter to select · Esc to quit",
        muted,
    )));
    lines
}

fn push_sandbox_config_line(
    lines: &mut Vec<StyledLine>,
    width: usize,
    label: &str,
    value: &str,
    style: Style,
) {
    lines.extend(wrap_sandbox_text(
        &format!("  {label}: {value}"),
        width,
        style,
    ));
}

fn push_sandbox_list_config_line(
    lines: &mut Vec<StyledLine>,
    width: usize,
    label: &str,
    values: &[String],
    style: Style,
) {
    let value = if values.is_empty() {
        "None".to_string()
    } else {
        values.join(", ")
    };
    push_sandbox_config_line(lines, width, label, &value, style);
}

fn option_bool_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "enabled",
        Some(false) => "disabled",
        None => "not configured",
    }
}

fn option_u64_label(value: Option<u64>) -> String {
    value.map_or_else(|| "not configured".to_string(), |port| port.to_string())
}

fn sandbox_platform_backend_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS seatbelt"
    }
    #[cfg(target_os = "linux")]
    {
        "Linux bubblewrap"
    }
    #[cfg(target_os = "windows")]
    {
        "Windows sandbox runner"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "unsupported platform"
    }
}

fn sandbox_tabs_line(active: SandboxTab) -> StyledLine {
    let muted = empty_transcript_placeholder_style();
    Line::from(vec![
        Span::styled("Sandbox:  ", Style::default().add_modifier(Modifier::BOLD)),
        sandbox_tab_span("Mode", active == SandboxTab::Mode),
        Span::styled("   ", muted),
        sandbox_tab_span("Overrides", active == SandboxTab::Overrides),
        Span::styled("   ", muted),
        sandbox_tab_span("Config", active == SandboxTab::Config),
    ])
}

fn sandbox_tab_span(label: &'static str, active: bool) -> Span<'static> {
    let style = if active {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        empty_transcript_placeholder_style()
    };
    Span::styled(label, style)
}

fn sandbox_option_line(index: usize, label: &str, selected: bool, current: bool) -> StyledLine {
    let muted = empty_transcript_placeholder_style();
    let marker = if selected { "❯ " } else { "  " };
    let style = if selected { Style::default() } else { muted };
    let label = if current {
        format!("{label} (current)")
    } else {
        label.to_string()
    };
    Line::from(vec![
        Span::styled("  ", muted),
        Span::styled(marker.to_string(), style),
        Span::styled(format!("{}.", index + 1), style),
        Span::styled(" ", muted),
        Span::styled(label, style),
    ])
}

fn wrap_sandbox_text(text: &str, width: usize, style: Style) -> Vec<StyledLine> {
    let limit = width.clamp(32, 88);
    wrap_inline_markdown_line(text, style, limit)
}
