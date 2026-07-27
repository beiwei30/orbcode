//! Keybinding schema parsing, default table, merge, and conflict validation.
//!
//! The TUI dispatches a small set of configurable chords through the table
//! produced here. User overrides live in `<home>/keybindings.json` (the same
//! file written by `/keybindings edit`) and are merged on top of the built-in
//! defaults. Parsing is deliberately permissive: malformed entries become
//! readable warnings rather than hard failures so a single typo never disables
//! the whole keymap.

use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

/// Built-in keymap. Mirrors the `keybindings.json` template written by the app
/// server, with three Orb Code readline additions in the `Chat` context
/// (`chat:lineStart`, `chat:lineEnd`, `chat:clearInput`).
const DEFAULT_KEYBINDINGS_JSON: &str = r#"{
  "bindings": [
    {
      "context": "Global",
      "bindings": {
        "ctrl+l": "app:redraw",
        "ctrl+t": "app:toggleTodos",
        "ctrl+o": "app:toggleTranscript",
        "ctrl+j": "app:toggleBackgroundJobs",
        "ctrl+r": "history:search"
      }
    },
    {
      "context": "Chat",
      "bindings": {
        "escape": "chat:cancel",
        "ctrl+x ctrl+k": "chat:killAgents",
        "shift+tab": "chat:cycleMode",
        "meta+p": "chat:modelPicker",
        "meta+o": "chat:fastMode",
        "meta+t": "chat:thinkingToggle",
        "enter": "chat:submit",
        "up": "history:previous",
        "down": "history:next",
        "ctrl+a": "chat:lineStart",
        "ctrl+e": "chat:lineEnd",
        "ctrl+u": "chat:clearInput",
        "ctrl+_": "chat:undo",
        "ctrl+shift+-": "chat:undo",
        "ctrl+x ctrl+e": "chat:externalEditor",
        "ctrl+g": "chat:externalEditor",
        "ctrl+s": "chat:stash",
        "ctrl+v": "chat:imagePaste"
      }
    },
    {
      "context": "Autocomplete",
      "bindings": {
        "tab": "autocomplete:accept",
        "escape": "autocomplete:dismiss",
        "up": "autocomplete:previous",
        "down": "autocomplete:next"
      }
    },
    {
      "context": "Settings",
      "bindings": {
        "escape": "confirm:no",
        "up": "select:previous",
        "down": "select:next",
        "k": "select:previous",
        "j": "select:next",
        "ctrl+p": "select:previous",
        "ctrl+n": "select:next",
        "space": "select:accept",
        "enter": "settings:close",
        "/": "settings:search",
        "r": "settings:retry"
      }
    },
    {
      "context": "Confirmation",
      "bindings": {
        "y": "confirm:yes",
        "n": "confirm:no",
        "enter": "confirm:yes",
        "escape": "confirm:no",
        "up": "confirm:previous",
        "down": "confirm:next",
        "tab": "confirm:nextField",
        "space": "confirm:toggle",
        "shift+tab": "confirm:cycleMode",
        "ctrl+e": "confirm:toggleExplanation",
        "ctrl+d": "permission:toggleDebug"
      }
    },
    {
      "context": "ThemePicker",
      "bindings": {
        "ctrl+t": "theme:toggleSyntaxHighlighting"
      }
    },
    {
      "context": "Transcript",
      "bindings": {
        "ctrl+e": "transcript:toggleShowAll",
        "escape": "transcript:exit",
        "q": "transcript:exit"
      }
    },
    {
      "context": "Select",
      "bindings": {
        "up": "select:previous",
        "down": "select:next",
        "j": "select:next",
        "k": "select:previous",
        "ctrl+n": "select:next",
        "ctrl+p": "select:previous",
        "enter": "select:accept",
        "escape": "select:cancel"
      }
    },
    {
      "context": "DiffDialog",
      "bindings": {
        "escape": "diff:dismiss",
        "left": "diff:previousSource",
        "right": "diff:nextSource",
        "up": "diff:previousFile",
        "down": "diff:nextFile",
        "enter": "diff:viewDetails"
      }
    },
    {
      "context": "ModelPicker",
      "bindings": {
        "left": "modelPicker:decreaseEffort",
        "right": "modelPicker:increaseEffort"
      }
    },
    {
      "context": "MessageSelector",
      "bindings": {
        "up": "messageSelector:up",
        "down": "messageSelector:down",
        "k": "messageSelector:up",
        "j": "messageSelector:down",
        "ctrl+p": "messageSelector:up",
        "ctrl+n": "messageSelector:down",
        "enter": "messageSelector:select"
      }
    }
  ]
}
"#;

/// Action strings recognized by the keymap. Used to surface a readable warning
/// when a user binds a key to an action Orb Code does not understand.
const KNOWN_ACTIONS: &[&str] = &[
    "app:redraw",
    "app:toggleTodos",
    "app:toggleTranscript",
    "app:toggleBackgroundJobs",
    "history:search",
    "history:previous",
    "history:next",
    "chat:cancel",
    "chat:killAgents",
    "chat:cycleMode",
    "chat:modelPicker",
    "chat:fastMode",
    "chat:thinkingToggle",
    "chat:submit",
    "chat:undo",
    "chat:externalEditor",
    "chat:stash",
    "chat:imagePaste",
    "chat:lineStart",
    "chat:lineEnd",
    "chat:clearInput",
    "autocomplete:accept",
    "autocomplete:dismiss",
    "autocomplete:previous",
    "autocomplete:next",
    "confirm:yes",
    "confirm:no",
    "confirm:previous",
    "confirm:next",
    "confirm:nextField",
    "confirm:toggle",
    "confirm:cycleMode",
    "confirm:toggleExplanation",
    "permission:toggleDebug",
    "select:previous",
    "select:next",
    "select:accept",
    "select:cancel",
    "settings:close",
    "settings:search",
    "settings:retry",
    "theme:toggleSyntaxHighlighting",
    "transcript:toggleShowAll",
    "transcript:exit",
    "diff:dismiss",
    "diff:previousSource",
    "diff:nextSource",
    "diff:previousFile",
    "diff:nextFile",
    "diff:viewDetails",
    "modelPicker:decreaseEffort",
    "modelPicker:increaseEffort",
    "messageSelector:up",
    "messageSelector:down",
    "messageSelector:select",
];

/// A keymap context. Bindings only apply within their context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeybindingContext {
    Global,
    Chat,
    Autocomplete,
    Settings,
    Confirmation,
    ThemePicker,
    Transcript,
    Select,
    DiffDialog,
    ModelPicker,
    MessageSelector,
}

impl KeybindingContext {
    /// Contexts in their canonical display order.
    pub const ALL: &'static [KeybindingContext] = &[
        KeybindingContext::Global,
        KeybindingContext::Chat,
        KeybindingContext::Autocomplete,
        KeybindingContext::Settings,
        KeybindingContext::Confirmation,
        KeybindingContext::ThemePicker,
        KeybindingContext::Transcript,
        KeybindingContext::Select,
        KeybindingContext::DiffDialog,
        KeybindingContext::ModelPicker,
        KeybindingContext::MessageSelector,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            KeybindingContext::Global => "Global",
            KeybindingContext::Chat => "Chat",
            KeybindingContext::Autocomplete => "Autocomplete",
            KeybindingContext::Settings => "Settings",
            KeybindingContext::Confirmation => "Confirmation",
            KeybindingContext::ThemePicker => "ThemePicker",
            KeybindingContext::Transcript => "Transcript",
            KeybindingContext::Select => "Select",
            KeybindingContext::DiffDialog => "DiffDialog",
            KeybindingContext::ModelPicker => "ModelPicker",
            KeybindingContext::MessageSelector => "MessageSelector",
        }
    }

    pub fn parse(value: &str) -> Option<KeybindingContext> {
        KeybindingContext::ALL
            .iter()
            .copied()
            .find(|context| context.as_str().eq_ignore_ascii_case(value))
    }
}

/// The non-modifier portion of a chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyToken {
    Char(char),
    Enter,
    Escape,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Function(u8),
}

impl KeyToken {
    fn display(self) -> String {
        match self {
            KeyToken::Char(' ') | KeyToken::Space => "space".to_string(),
            KeyToken::Char(character) => character.to_string(),
            KeyToken::Enter => "enter".to_string(),
            KeyToken::Escape => "escape".to_string(),
            KeyToken::Tab => "tab".to_string(),
            KeyToken::BackTab => "backtab".to_string(),
            KeyToken::Backspace => "backspace".to_string(),
            KeyToken::Delete => "delete".to_string(),
            KeyToken::Insert => "insert".to_string(),
            KeyToken::Up => "up".to_string(),
            KeyToken::Down => "down".to_string(),
            KeyToken::Left => "left".to_string(),
            KeyToken::Right => "right".to_string(),
            KeyToken::Home => "home".to_string(),
            KeyToken::End => "end".to_string(),
            KeyToken::PageUp => "pageup".to_string(),
            KeyToken::PageDown => "pagedown".to_string(),
            KeyToken::Function(number) => format!("f{number}"),
        }
    }
}

/// A single key chord: a base key plus modifier flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyChord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub key: KeyToken,
}

impl KeyChord {
    pub fn new(key: KeyToken) -> KeyChord {
        KeyChord {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key,
        }
    }

    /// Canonical, lower-cased rendering of the chord (e.g. `ctrl+a`).
    pub fn display(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("ctrl".to_string());
        }
        if self.alt {
            parts.push("alt".to_string());
        }
        if self.shift {
            parts.push("shift".to_string());
        }
        if self.meta {
            parts.push("meta".to_string());
        }
        parts.push(self.key.display());
        parts.join("+")
    }
}

/// Failure to parse a chord specification such as `ctrl+frob`.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ChordParseError {
    #[error("empty chord specification")]
    Empty,
    #[error("unknown key `{0}`")]
    UnknownKey(String),
}

/// Parse a single chord such as `ctrl+shift+a`.
pub fn parse_chord(spec: &str) -> Result<KeyChord, ChordParseError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(ChordParseError::Empty);
    }

    let parts: Vec<&str> = spec.split('+').collect();
    let mut chord = KeyChord {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
        key: KeyToken::Space,
    };
    let mut key: Option<KeyToken> = None;

    for (index, raw) in parts.iter().enumerate() {
        let token = raw.trim();
        let is_last = index + 1 == parts.len();
        // A trailing empty segment is the literal `+` key (e.g. `ctrl++`).
        if token.is_empty() {
            if is_last {
                key = Some(KeyToken::Char('+'));
                continue;
            }
            return Err(ChordParseError::UnknownKey(String::new()));
        }

        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => chord.ctrl = true,
            "alt" | "option" | "opt" => chord.alt = true,
            "shift" => chord.shift = true,
            "meta" | "cmd" | "command" | "super" | "win" => chord.meta = true,
            other => {
                key = Some(parse_key_token(other)?);
            }
        }
    }

    chord.key = key.ok_or_else(|| ChordParseError::UnknownKey(spec.to_string()))?;
    Ok(chord)
}

fn parse_key_token(token: &str) -> Result<KeyToken, ChordParseError> {
    let value = match token {
        "enter" | "return" | "cr" => KeyToken::Enter,
        "escape" | "esc" => KeyToken::Escape,
        "tab" => KeyToken::Tab,
        "backtab" => KeyToken::BackTab,
        "space" | "spacebar" => KeyToken::Space,
        "backspace" | "bs" => KeyToken::Backspace,
        "delete" | "del" => KeyToken::Delete,
        "insert" | "ins" => KeyToken::Insert,
        "up" => KeyToken::Up,
        "down" => KeyToken::Down,
        "left" => KeyToken::Left,
        "right" => KeyToken::Right,
        "home" => KeyToken::Home,
        "end" => KeyToken::End,
        "pageup" | "pgup" => KeyToken::PageUp,
        "pagedown" | "pgdn" => KeyToken::PageDown,
        _ => {
            if let Some(number) = token.strip_prefix('f')
                && let Ok(number) = number.parse::<u8>()
                && (1..=24).contains(&number)
            {
                return Ok(KeyToken::Function(number));
            }
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some(single), None) => KeyToken::Char(single.to_ascii_lowercase()),
                _ => return Err(ChordParseError::UnknownKey(token.to_string())),
            }
        }
    };
    Ok(value)
}

/// Parse a whitespace-separated chord sequence such as `ctrl+x ctrl+e`.
pub fn parse_chord_sequence(spec: &str) -> Result<Vec<KeyChord>, ChordParseError> {
    let chords: Vec<&str> = spec.split_whitespace().collect();
    if chords.is_empty() {
        return Err(ChordParseError::Empty);
    }
    chords.iter().map(|chord| parse_chord(chord)).collect()
}

fn display_chord_sequence(chords: &[KeyChord]) -> String {
    chords
        .iter()
        .map(|chord| chord.display())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Default)]
struct ContextBindings {
    lookup: HashMap<Vec<KeyChord>, String>,
    /// (display chord, action) pairs, kept sorted for stable overlay rendering.
    entries: Vec<(String, String)>,
}

impl ContextBindings {
    fn rebuild_entries(&mut self) {
        let mut entries: Vec<(String, String)> = self
            .lookup
            .iter()
            .map(|(chords, action)| (display_chord_sequence(chords), action.clone()))
            .collect();
        entries.sort();
        self.entries = entries;
    }
}

/// The merged keymap plus any warnings collected while loading it.
#[derive(Clone)]
pub struct ResolvedKeybindings {
    contexts: BTreeMap<KeybindingContext, ContextBindings>,
    warnings: Vec<String>,
}

impl ResolvedKeybindings {
    /// The built-in keymap with no user overrides applied.
    pub fn defaults() -> ResolvedKeybindings {
        let mut resolved = ResolvedKeybindings {
            contexts: BTreeMap::new(),
            warnings: Vec::new(),
        };
        // The default JSON is well-formed and uses only known
        // actions/contexts, so this never produces warnings. Defensive: if it
        // ever did, surface them rather than silently dropping bindings.
        resolved.apply_json(DEFAULT_KEYBINDINGS_JSON);
        resolved.rebuild_entries();
        resolved
    }

    /// The action bound to `chords` within `context`, if any.
    pub fn action(&self, context: KeybindingContext, chords: &[KeyChord]) -> Option<&str> {
        self.contexts
            .get(&context)
            .and_then(|bindings| bindings.lookup.get(chords))
            .map(String::as_str)
    }

    /// Sorted `(chord, action)` pairs for `context`, for overlay rendering.
    pub fn entries(&self, context: KeybindingContext) -> &[(String, String)] {
        self.contexts
            .get(&context)
            .map_or(&[], |bindings| bindings.entries.as_slice())
    }

    /// Human-readable load/merge warnings (conflicts, unknown keys, etc.).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn rebuild_entries(&mut self) {
        for bindings in self.contexts.values_mut() {
            bindings.rebuild_entries();
        }
    }

    fn apply_json(&mut self, contents: &str) {
        let parsed: KeybindingsFileRaw = match serde_json::from_str(contents) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.warnings
                    .push(format!("Ignored keybindings: invalid JSON ({error})."));
                return;
            }
        };

        for block in parsed.bindings {
            let Some(context) = KeybindingContext::parse(&block.context) else {
                self.warnings.push(format!(
                    "Ignored unknown keybinding context `{}`.",
                    block.context
                ));
                continue;
            };

            // Track chords seen within this block so two specs that normalize
            // to the same chord (e.g. `esc` and `escape`) surface as conflicts.
            let mut seen: HashMap<Vec<KeyChord>, String> = HashMap::new();
            for (spec, value) in block.bindings {
                let action = match value {
                    serde_json::Value::String(action) => action,
                    _ => {
                        self.warnings.push(format!(
                            "Ignored keybinding `{spec}` in {}: action must be a string.",
                            context.as_str()
                        ));
                        continue;
                    }
                };

                let chords = match parse_chord_sequence(&spec) {
                    Ok(chords) => chords,
                    Err(error) => {
                        self.warnings.push(format!(
                            "Ignored keybinding `{spec}` in {}: {error}.",
                            context.as_str()
                        ));
                        continue;
                    }
                };

                if !KNOWN_ACTIONS.contains(&action.as_str()) {
                    self.warnings.push(format!(
                        "Keybinding `{}` in {} uses unknown action `{action}`.",
                        display_chord_sequence(&chords),
                        context.as_str()
                    ));
                }

                if let Some(existing) = seen.get(&chords)
                    && existing != &action
                {
                    self.warnings.push(format!(
                        "Conflict in {}: `{}` is bound to both `{existing}` and `{action}`.",
                        context.as_str(),
                        display_chord_sequence(&chords),
                    ));
                    continue;
                }
                seen.insert(chords.clone(), action.clone());

                self.contexts
                    .entry(context)
                    .or_default()
                    .lookup
                    .insert(chords, action);
            }
        }
    }
}

#[derive(Deserialize)]
struct KeybindingsFileRaw {
    #[serde(default)]
    bindings: Vec<ContextBlockRaw>,
}

#[derive(Deserialize)]
struct ContextBlockRaw {
    context: String,
    #[serde(default)]
    bindings: serde_json::Map<String, serde_json::Value>,
}

/// Load the keymap for `home_dir`, merging `<home>/keybindings.json` (if any)
/// over the built-in defaults. Never fails: read/parse problems become warnings.
pub fn load_keybindings(home_dir: &Path) -> ResolvedKeybindings {
    let mut resolved = ResolvedKeybindings::defaults();
    let path = home_dir.join("keybindings.json");
    match std::fs::read_to_string(&path) {
        Ok(contents) => resolved.apply_json(&contents),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => resolved
            .warnings
            .push(format!("Could not read {}: {error}.", path.display())),
    }
    resolved.rebuild_entries();
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_and_modified_chords() {
        assert_eq!(
            parse_chord("a").unwrap(),
            KeyChord::new(KeyToken::Char('a'))
        );
        let ctrl_a = parse_chord("ctrl+a").unwrap();
        assert!(ctrl_a.ctrl);
        assert_eq!(ctrl_a.key, KeyToken::Char('a'));
        assert_eq!(parse_chord("escape").unwrap().key, KeyToken::Escape);
        assert_eq!(parse_chord("esc").unwrap().key, KeyToken::Escape);
        assert_eq!(parse_chord("f5").unwrap().key, KeyToken::Function(5));
        assert_eq!(parse_chord("space").unwrap().key, KeyToken::Space);
    }

    #[test]
    fn chord_parsing_is_case_insensitive() {
        assert_eq!(
            parse_chord("Ctrl+A").unwrap(),
            parse_chord("ctrl+a").unwrap()
        );
        assert_eq!(
            parse_chord("CONTROL+a").unwrap(),
            parse_chord("ctrl+a").unwrap()
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(matches!(
            parse_chord("ctrl+frobnicate"),
            Err(ChordParseError::UnknownKey(_))
        ));
        assert!(matches!(parse_chord(""), Err(ChordParseError::Empty)));
    }

    #[test]
    fn parses_multi_chord_sequence() {
        let chords = parse_chord_sequence("ctrl+x ctrl+e").unwrap();
        assert_eq!(chords.len(), 2);
        assert!(chords[0].ctrl);
        assert_eq!(chords[0].key, KeyToken::Char('x'));
        assert_eq!(chords[1].key, KeyToken::Char('e'));
    }

    #[test]
    fn defaults_are_warning_free_and_populated() {
        let resolved = ResolvedKeybindings::defaults();
        assert!(
            resolved.warnings().is_empty(),
            "default keymap should not warn: {:?}",
            resolved.warnings()
        );
        let ctrl_o = parse_chord_sequence("ctrl+o").unwrap();
        assert_eq!(
            resolved.action(KeybindingContext::Global, &ctrl_o),
            Some("app:toggleTranscript")
        );
        let ctrl_a = parse_chord_sequence("ctrl+a").unwrap();
        assert_eq!(
            resolved.action(KeybindingContext::Chat, &ctrl_a),
            Some("chat:lineStart")
        );
    }

    #[test]
    fn user_binding_overrides_default() {
        let mut resolved = ResolvedKeybindings::defaults();
        resolved.apply_json(
            r#"{ "bindings": [ { "context": "Global", "bindings": { "ctrl+o": "app:toggleTodos" } } ] }"#,
        );
        let ctrl_o = parse_chord_sequence("ctrl+o").unwrap();
        assert_eq!(
            resolved.action(KeybindingContext::Global, &ctrl_o),
            Some("app:toggleTodos")
        );
        assert!(resolved.warnings().is_empty());
    }

    #[test]
    fn detects_normalized_conflict() {
        let mut resolved = ResolvedKeybindings {
            contexts: BTreeMap::new(),
            warnings: Vec::new(),
        };
        resolved.apply_json(
            r#"{ "bindings": [ { "context": "Chat", "bindings": { "esc": "chat:cancel", "escape": "chat:submit" } } ] }"#,
        );
        assert_eq!(resolved.warnings().len(), 1);
        assert!(resolved.warnings()[0].contains("Conflict"));
        assert!(resolved.warnings()[0].contains("escape"));
    }

    #[test]
    fn warns_on_unknown_action_and_context() {
        let mut resolved = ResolvedKeybindings {
            contexts: BTreeMap::new(),
            warnings: Vec::new(),
        };
        resolved.apply_json(
            r#"{ "bindings": [
                { "context": "Nonsense", "bindings": { "ctrl+a": "chat:lineStart" } },
                { "context": "Chat", "bindings": { "ctrl+q": "chat:doTheThing" } }
            ] }"#,
        );
        assert!(
            resolved
                .warnings()
                .iter()
                .any(|w| w.contains("unknown keybinding context"))
        );
        assert!(
            resolved
                .warnings()
                .iter()
                .any(|w| w.contains("unknown action"))
        );
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = load_keybindings(dir.path());
        assert!(resolved.warnings().is_empty());
        let ctrl_o = parse_chord_sequence("ctrl+o").unwrap();
        assert_eq!(
            resolved.action(KeybindingContext::Global, &ctrl_o),
            Some("app:toggleTranscript")
        );
    }

    #[test]
    fn load_invalid_json_warns_and_keeps_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keybindings.json"), "{ not json").unwrap();
        let resolved = load_keybindings(dir.path());
        assert_eq!(resolved.warnings().len(), 1);
        assert!(resolved.warnings()[0].contains("invalid JSON"));
        let ctrl_o = parse_chord_sequence("ctrl+o").unwrap();
        assert_eq!(
            resolved.action(KeybindingContext::Global, &ctrl_o),
            Some("app:toggleTranscript")
        );
    }

    #[test]
    fn entries_are_sorted_and_include_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("keybindings.json"),
            r#"{ "bindings": [ { "context": "Global", "bindings": { "f8": "app:toggleTranscript" } } ] }"#,
        )
        .unwrap();
        let resolved = load_keybindings(dir.path());
        let entries = resolved.entries(KeybindingContext::Global);
        assert!(
            entries
                .iter()
                .any(|(chord, action)| chord == "f8" && action == "app:toggleTranscript")
        );
        let sorted = {
            let mut clone = entries.to_vec();
            clone.sort();
            clone
        };
        assert_eq!(entries, sorted.as_slice());
    }
}
