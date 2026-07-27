//! TUI-global keybinding runtime.
//!
//! Mirrors the existing `set_active_theme` pattern: the resolved keymap is
//! loaded once from `TuiState::new` (via the bootstrap `home_dir`) into a
//! process-global cell so key dispatch can look up configurable chords without
//! threading the keymap through every call. Only a small, behavior-safe subset
//! of actions is dispatched here; the built-in defaults reproduce the previous
//! hard-coded chords exactly, so an empty/absent `keybindings.json` changes
//! nothing.

use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use orbcode_app_server_client::AppClient;
use orbcode_config::{KeyChord, KeyToken, KeybindingContext, ResolvedKeybindings};

struct KeybindingRuntime {
    home_dir: PathBuf,
    resolved: ResolvedKeybindings,
}

static RUNTIME: OnceLock<RwLock<KeybindingRuntime>> = OnceLock::new();

/// The actions Orb Code actually dispatches through the configurable keymap. Other
/// known actions remain in the table for discoverability but are handled by the
/// fixed input pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeybindingAction {
    ToggleTranscript,
    ToggleTodos,
    ToggleBackgroundJobs,
    HistorySearch,
    LineStart,
    LineEnd,
    ClearInput,
}

impl KeybindingAction {
    fn from_action(action: &str) -> Option<KeybindingAction> {
        Some(match action {
            "app:toggleTranscript" => KeybindingAction::ToggleTranscript,
            "app:toggleTodos" => KeybindingAction::ToggleTodos,
            "app:toggleBackgroundJobs" => KeybindingAction::ToggleBackgroundJobs,
            "history:search" => KeybindingAction::HistorySearch,
            "chat:lineStart" => KeybindingAction::LineStart,
            "chat:lineEnd" => KeybindingAction::LineEnd,
            "chat:clearInput" => KeybindingAction::ClearInput,
            _ => return None,
        })
    }
}

/// Install pre-loaded keybindings into the global keymap for `home_dir`.
/// Returns any load/merge warnings so the caller can surface them on the
/// status line.
pub(crate) fn load_keybindings_global(
    resolved: ResolvedKeybindings,
    home_dir: PathBuf,
) -> Vec<String> {
    let warnings = resolved.warnings().to_vec();
    let runtime = KeybindingRuntime { home_dir, resolved };
    match RUNTIME.get() {
        Some(lock) => {
            if let Ok(mut guard) = lock.write() {
                *guard = runtime;
            }
        }
        None => {
            let _ = RUNTIME.set(RwLock::new(runtime));
        }
    }
    warnings
}

/// Re-read `keybindings.json` via AppClient and install the result. Used
/// after the user edits the file via `/keybindings`.
pub(crate) fn reload_keybindings_global(_app_server: &AppClient) -> Vec<String> {
    let home_dir = RUNTIME
        .get()
        .and_then(|lock| lock.read().ok().map(|guard| guard.home_dir.clone()));
    match home_dir {
        Some(home_dir) => {
            let resolved = orbcode_config::load_keybindings(&home_dir);
            load_keybindings_global(resolved, home_dir)
        }
        None => Vec::new(),
    }
}

/// Return warnings from the currently installed keymap, if any.
pub(crate) fn keybinding_warnings() -> Vec<String> {
    match RUNTIME.get().and_then(|lock| lock.read().ok()) {
        Some(guard) => guard.resolved.warnings().to_vec(),
        None => Vec::new(),
    }
}

/// Resolve `chord` to a dispatchable action by searching `contexts` in order.
/// Returns `None` when unbound or bound to an action handled elsewhere.
pub(crate) fn action_for(
    contexts: &[KeybindingContext],
    chord: KeyChord,
) -> Option<KeybindingAction> {
    let chords = [chord];
    let resolve = |resolved: &ResolvedKeybindings| {
        for &context in contexts {
            if let Some(action) = resolved.action(context, &chords)
                && let Some(parsed) = KeybindingAction::from_action(action)
            {
                return Some(parsed);
            }
        }
        None
    };
    match RUNTIME.get().and_then(|lock| lock.read().ok()) {
        Some(guard) => resolve(&guard.resolved),
        None => resolve(&ResolvedKeybindings::defaults()),
    }
}

pub(crate) fn resolved_keybinding_entries(context: KeybindingContext) -> Vec<(String, String)> {
    match RUNTIME.get().and_then(|lock| lock.read().ok()) {
        Some(guard) => guard.resolved.entries(context).to_vec(),
        None => ResolvedKeybindings::defaults().entries(context).to_vec(),
    }
}

/// Convert a crossterm key event into a single keybinding chord. Returns `None`
/// for key codes that have no chord representation (so they fall through to the
/// fixed input pipeline untouched).
pub(crate) fn chord_from_key_event(event: &KeyEvent) -> Option<KeyChord> {
    let key = match event.code {
        KeyCode::Char(' ') => KeyToken::Space,
        KeyCode::Char(character) => KeyToken::Char(character.to_ascii_lowercase()),
        KeyCode::Enter => KeyToken::Enter,
        KeyCode::Esc => KeyToken::Escape,
        KeyCode::Tab => KeyToken::Tab,
        KeyCode::BackTab => KeyToken::BackTab,
        KeyCode::Backspace => KeyToken::Backspace,
        KeyCode::Delete => KeyToken::Delete,
        KeyCode::Insert => KeyToken::Insert,
        KeyCode::Up => KeyToken::Up,
        KeyCode::Down => KeyToken::Down,
        KeyCode::Left => KeyToken::Left,
        KeyCode::Right => KeyToken::Right,
        KeyCode::Home => KeyToken::Home,
        KeyCode::End => KeyToken::End,
        KeyCode::PageUp => KeyToken::PageUp,
        KeyCode::PageDown => KeyToken::PageDown,
        KeyCode::F(number) => KeyToken::Function(number),
        _ => return None,
    };
    let modifiers = event.modifiers;
    Some(KeyChord {
        ctrl: modifiers.contains(KeyModifiers::CONTROL),
        alt: modifiers.contains(KeyModifiers::ALT),
        shift: modifiers.contains(KeyModifiers::SHIFT),
        meta: modifiers.contains(KeyModifiers::META) || modifiers.contains(KeyModifiers::SUPER),
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_from_ctrl_a() {
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let chord = chord_from_key_event(&event).unwrap();
        assert!(chord.ctrl);
        assert_eq!(chord.key, KeyToken::Char('a'));
    }

    #[test]
    fn defaults_dispatch_known_chords() {
        // RUNTIME is uninitialized in unit tests, so this exercises the
        // default-table fallback path.
        let ctrl_o =
            chord_from_key_event(&KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
                .unwrap();
        assert_eq!(
            action_for(&[KeybindingContext::Global], ctrl_o),
            Some(KeybindingAction::ToggleTranscript)
        );
        let ctrl_a =
            chord_from_key_event(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL))
                .unwrap();
        assert_eq!(
            action_for(
                &[KeybindingContext::Chat, KeybindingContext::Global],
                ctrl_a
            ),
            Some(KeybindingAction::LineStart)
        );
    }

    #[test]
    fn plain_char_is_unbound() {
        let event = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
        let chord = chord_from_key_event(&event).unwrap();
        assert_eq!(action_for(&[KeybindingContext::Global], chord), None);
    }
}
