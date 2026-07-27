use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use ratatui::backend::CrosstermBackend;

use crate::commands::utils::slash_command_display_path;
use crate::custom_terminal::Terminal;
use crate::state::TuiState;
use crate::tui_runtime::terminal_session::{restore_terminal, setup_terminal};

pub(crate) struct EditorLaunchInfo {
    pub(crate) source: &'static str,
    pub(crate) value: String,
}

pub(crate) struct ExternalEditorRequest {
    pub(crate) command: String,
    pub(crate) path: PathBuf,
    pub(crate) target: ExternalEditorTarget,
}

pub(crate) enum ExternalEditorTarget {
    Memory,
    Keybindings { created: bool },
}

impl TuiState {
    pub(crate) fn take_external_editor_request(&mut self) -> Option<ExternalEditorRequest> {
        self.external_editor_request.take()
    }

    pub(crate) fn report_external_editor_result(
        &mut self,
        request: ExternalEditorRequest,
        result: Result<EditorLaunchInfo>,
    ) -> bool {
        let display_path = slash_command_display_path(&request.path, &self.cwd);
        let needs_keybinding_reload = matches!(
            (&result, &request.target),
            (Ok(_), ExternalEditorTarget::Keybindings { .. })
        );
        match result {
            Ok(editor) => {
                let hint = if editor.source == "default" {
                    "> To use a different editor, set the $EDITOR or $VISUAL environment variable."
                        .to_string()
                } else {
                    format!(
                        "> Using {}=\"{}\". To change editor, set $EDITOR or $VISUAL environment variable.",
                        editor.source, editor.value
                    )
                };
                let summary = match request.target {
                    ExternalEditorTarget::Memory => {
                        format!("Opened memory file at {display_path}")
                    }
                    ExternalEditorTarget::Keybindings { created } => {
                        let base = if created {
                            format!("Created keybindings file at {display_path}")
                        } else {
                            format!("Opened keybindings file at {display_path}")
                        };
                        let warnings = crate::keybindings::keybinding_warnings();
                        if warnings.is_empty() {
                            base
                        } else {
                            format!("{base} ({} warning(s))", warnings.len())
                        }
                    }
                };
                self.push_local_slash_command_output(request.command, summary.clone(), Some(hint));
                self.set_status_line(format!("{summary}."));
            }
            Err(error) => {
                let (target_name, sentence_target_name) = match request.target {
                    ExternalEditorTarget::Memory => ("memory file", "Memory file"),
                    ExternalEditorTarget::Keybindings { .. } => {
                        ("keybindings file", "Keybindings file")
                    }
                };
                let summary = format!("{sentence_target_name} editor failed: {error}");
                self.push_local_slash_command_output(
                    request.command,
                    format!("Failed to open {target_name} at {display_path}"),
                    Some(format!("> {error}")),
                );
                self.set_status_line(summary);
            }
        }
        needs_keybinding_reload
    }
}

pub(crate) fn open_file_in_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    terminal_reader_paused: &AtomicBool,
    path: &Path,
) -> Result<EditorLaunchInfo> {
    terminal_reader_paused.store(true, Ordering::SeqCst);
    let restore_result = restore_terminal(terminal);
    let editor_result = match restore_result {
        Ok(()) => edit_file_in_editor(path),
        Err(error) => Err(error),
    };
    let setup_result = setup_terminal();
    terminal_reader_paused.store(false, Ordering::SeqCst);
    *terminal = setup_result?;
    editor_result
}

fn edit_file_in_editor(path: &Path) -> Result<EditorLaunchInfo> {
    let editor = selected_editor();
    let status = std::process::Command::new(&editor.program)
        .args(&editor.args)
        .arg(path)
        .status()?;
    if !status.success() {
        anyhow::bail!("{} exited with status {status}.", editor.value);
    }
    Ok(EditorLaunchInfo {
        source: editor.source,
        value: editor.value,
    })
}

pub(crate) fn open_path_in_system(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open").arg(path).status()?;
        if status.success() {
            return Ok(());
        }
        anyhow::bail!("open exited with status {status}.");
    }

    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .status()?;
        if status.success() {
            return Ok(());
        }
        anyhow::bail!("start exited with status {status}.");
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let status = std::process::Command::new("xdg-open").arg(path).status()?;
        if status.success() {
            return Ok(());
        }
        anyhow::bail!("xdg-open exited with status {status}.");
    }
}

struct SelectedEditor {
    source: &'static str,
    value: String,
    program: String,
    args: Vec<String>,
}

fn selected_editor() -> SelectedEditor {
    let (source, value) = if let Ok(value) = std::env::var("VISUAL")
        && !value.trim().is_empty()
    {
        ("$VISUAL", value)
    } else if let Ok(value) = std::env::var("EDITOR")
        && !value.trim().is_empty()
    {
        ("$EDITOR", value)
    } else {
        (
            "default",
            if cfg!(target_os = "windows") {
                "notepad".to_string()
            } else {
                "vi".to_string()
            },
        )
    };
    let mut parts = value.split_whitespace();
    let program = parts.next().unwrap_or(value.as_str()).to_string();
    let args = parts.map(str::to_string).collect();
    SelectedEditor {
        source,
        value,
        program,
        args,
    }
}

#[allow(dead_code)]
pub(crate) fn use_tmux_alternate_screen() -> bool {
    std::env::var_os("TMUX").is_some()
}
