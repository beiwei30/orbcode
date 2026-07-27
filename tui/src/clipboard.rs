#[cfg(not(test))]
use std::io;
#[cfg(not(test))]
use std::io::Write;
#[cfg(not(test))]
use std::process::{Command as ProcessCommand, Stdio};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::overlays::OverlayState;
use crate::state::TuiState;

pub(crate) fn copy_text_to_clipboard(text: &str) -> Result<()> {
    #[cfg(test)]
    {
        let mut clipboard = test_clipboard_capture()
            .lock()
            .expect("test clipboard mutex poisoned");
        *clipboard = Some(text.to_string());
        Ok(())
    }

    #[cfg(not(test))]
    {
        #[cfg(target_os = "macos")]
        {
            pipe_text_to_command("pbcopy", &[], text)
        }

        #[cfg(target_os = "windows")]
        {
            pipe_text_to_command("clip.exe", &[], text)
        }

        #[cfg(target_os = "linux")]
        {
            let clipboard_commands = [
                ("wl-copy", Vec::<&str>::new()),
                ("xclip", vec!["-selection", "clipboard"]),
                ("xsel", vec!["--clipboard", "--input"]),
            ];
            let mut last_error = None;
            for (program, args) in clipboard_commands {
                match pipe_text_to_command(program, &args, text) {
                    Ok(()) => return Ok(()),
                    Err(error) => last_error = Some(error),
                }
            }
            if let Some(error) = last_error {
                return Err(error);
            }
            anyhow::bail!("No clipboard integration is available on this system.");
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = text;
            anyhow::bail!("Clipboard copy is not supported on this platform.");
        }
    }
}

pub(crate) fn is_transcript_copy_shortcut(key_event: &KeyEvent) -> bool {
    match key_event.code {
        KeyCode::Char(ch) => {
            let lower = ch.to_ascii_lowercase();
            lower == 'c' && transcript_copy_modifiers_match(key_event.modifiers)
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) fn transcript_copy_shortcut_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Cmd+C"
    }

    #[cfg(not(target_os = "macos"))]
    {
        "Ctrl+Shift+C"
    }
}

fn transcript_copy_modifiers_match(modifiers: KeyModifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.contains(KeyModifiers::SUPER)
    }

    #[cfg(not(target_os = "macos"))]
    {
        modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::SHIFT)
    }
}

impl TuiState {
    pub(crate) fn clear_screen_selection(&mut self) {
        self.clear_transcript_selection();
        self.clear_permission_selection();
    }

    pub(crate) fn copy_selected_screen_to_clipboard(&mut self) -> Result<usize> {
        self.refresh_transcript_selection_lines();
        let mut selected = Vec::new();
        if let Some(text) = self.transcript_ui.viewport.selected_text() {
            selected.push(text);
        }
        if let Some(OverlayState::PermissionRequest(permission)) = &self.overlay
            && let Some(text) = permission.viewport.selected_text()
        {
            selected.push(text);
        }
        let selected = selected.join("\n");
        if selected.is_empty() {
            anyhow::bail!("No text is selected.");
        }
        copy_text_to_clipboard(&selected)?;
        Ok(selected.chars().count())
    }

    pub(crate) fn report_transcript_copy_result(&mut self, result: Result<usize>) {
        match result {
            Ok(_) => self.set_status_line("Selected text copied."),
            Err(error) => self.set_status_line(format!("Copy failed: {error}")),
        }
    }

    pub(crate) fn auto_copy_screen_selection(&mut self) {
        let has_expanded_selection = self.transcript_ui.viewport.has_expanded_selection()
            || matches!(
                &self.overlay,
                Some(OverlayState::PermissionRequest(permission))
                    if permission.viewport.has_expanded_selection()
            );
        if !has_expanded_selection {
            self.clear_screen_selection();
            return;
        }
        let result = self.copy_selected_screen_to_clipboard();
        self.clear_screen_selection();
        self.report_transcript_copy_result(result);
    }
}

#[cfg(test)]
fn test_clipboard_capture() -> &'static Mutex<Option<String>> {
    static TEST_CLIPBOARD_CAPTURE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    TEST_CLIPBOARD_CAPTURE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) fn take_test_clipboard_capture() -> Option<String> {
    test_clipboard_capture()
        .lock()
        .expect("test clipboard mutex poisoned")
        .take()
}

#[cfg(test)]
pub(crate) fn test_clipboard_assertion_lock() -> &'static Mutex<()> {
    static TEST_CLIPBOARD_ASSERTION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    TEST_CLIPBOARD_ASSERTION_LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(not(test))]
fn pipe_text_to_command(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = match ProcessCommand::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            anyhow::bail!("{program} is not available in PATH.");
        }
        Err(error) => return Err(error.into()),
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        anyhow::bail!("{program} exited with status {}.", output.status);
    }
    anyhow::bail!("{program} failed: {stderr}");
}
