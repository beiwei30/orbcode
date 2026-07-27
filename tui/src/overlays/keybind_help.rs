use orbcode_config::KeybindingContext;

use super::*;
use crate::keybindings::resolved_keybinding_entries;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeybindHelpOverlayAction {
    None,
    Close,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct KeybindHelpOverlayState {
    pub(crate) scroll: usize,
    pub(crate) max_scroll: usize,
    pub(crate) lines_cache: KeybindHelpLinesCache,
}

type KeybindHelpLinesCache = LinesCache<usize>;

impl KeybindHelpOverlayState {
    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let width = width.max(1);
        self.lines_cache
            .refresh(width, || keybind_help_overlay_lines(width));
        &self.lines_cache.lines
    }

    pub(crate) fn cached_visible_lines(
        &mut self,
        width: usize,
        content_height: usize,
    ) -> Vec<StyledLine> {
        if content_height == 0 {
            return Vec::new();
        }
        let scroll = self.scroll.min(self.max_scroll);
        self.cached_lines(width)
            .iter()
            .skip(scroll)
            .take(content_height)
            .cloned()
            .collect()
    }
}

pub(crate) fn apply_keybind_help_overlay_key(
    state: &mut KeybindHelpOverlayState,
    key_event: &KeyEvent,
) -> KeybindHelpOverlayAction {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') => KeybindHelpOverlayAction::Close,
        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            KeybindHelpOverlayAction::Close
        }
        KeyCode::Up | KeyCode::Char('k') => {
            scroll_keybind_help_by(state, -1);
            KeybindHelpOverlayAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            scroll_keybind_help_by(state, 1);
            KeybindHelpOverlayAction::None
        }
        KeyCode::PageUp | KeyCode::Char('b') => {
            scroll_keybind_help_by(state, -(HELP_OVERLAY_PAGE_STEP as isize));
            KeybindHelpOverlayAction::None
        }
        KeyCode::PageDown | KeyCode::Char('f' | ' ') => {
            scroll_keybind_help_by(state, HELP_OVERLAY_PAGE_STEP as isize);
            KeybindHelpOverlayAction::None
        }
        KeyCode::Char('u') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_keybind_help_by(state, -((HELP_OVERLAY_PAGE_STEP / 2) as isize));
            KeybindHelpOverlayAction::None
        }
        KeyCode::Char('d') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_keybind_help_by(state, (HELP_OVERLAY_PAGE_STEP / 2) as isize);
            KeybindHelpOverlayAction::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.scroll = 0;
            KeybindHelpOverlayAction::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.scroll = state.max_scroll;
            KeybindHelpOverlayAction::None
        }
        _ => KeybindHelpOverlayAction::None,
    }
}

fn scroll_keybind_help_by(state: &mut KeybindHelpOverlayState, delta: isize) {
    let next = if delta < 0 {
        state.scroll.saturating_sub(delta.unsigned_abs())
    } else {
        state.scroll.saturating_add(delta as usize)
    };
    state.scroll = next.min(state.max_scroll);
}

pub(crate) fn sync_keybind_help_overlay_bounds(state: &mut KeybindHelpOverlayState, area: Rect) {
    let content_height = keybind_help_content_height(area);
    let line_count = state.cached_lines(area.width.max(1) as usize).len();
    state.max_scroll = line_count.saturating_sub(content_height);
    state.scroll = state.scroll.min(state.max_scroll);
}

pub(crate) fn keybind_help_content_height(area: Rect) -> usize {
    area.height.saturating_sub(1) as usize
}

pub(crate) fn draw_keybind_help_overlay(
    frame: &mut Frame,
    state: &mut KeybindHelpOverlayState,
    area: Rect,
) {
    frame.render_widget(Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }

    let content_height = keybind_help_content_height(area);
    let visible_lines = state.cached_visible_lines(area.width.max(1) as usize, content_height);
    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    frame.render_widget(
        Paragraph::new(visible_lines).wrap(Wrap { trim: false }),
        content_area,
    );

    let footer_area = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("jk", inactive_style()),
            Span::styled(" scroll · ", subtle_style()),
            Span::styled("fb", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" page · ", subtle_style()),
            Span::styled("gG", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" jump · ", subtle_style()),
            Span::styled("q", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" close", subtle_style()),
        ])),
        footer_area,
    );
}

fn context_display_name(context: KeybindingContext) -> &'static str {
    match context {
        KeybindingContext::Global => "GLOBAL",
        KeybindingContext::Chat => "CHAT",
        KeybindingContext::Autocomplete => "AUTOCOMPLETE",
        KeybindingContext::Settings => "SETTINGS",
        KeybindingContext::Confirmation => "CONFIRMATION",
        KeybindingContext::ThemePicker => "THEME PICKER",
        KeybindingContext::Transcript => "TRANSCRIPT",
        KeybindingContext::Select => "SELECT",
        KeybindingContext::DiffDialog => "DIFF DIALOG",
        KeybindingContext::ModelPicker => "MODEL PICKER",
        KeybindingContext::MessageSelector => "MESSAGE SELECTOR",
    }
}

pub(crate) fn keybind_help_overlay_lines(width: usize) -> Vec<StyledLine> {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "KEYBINDINGS",
            inactive_style().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ),
    ]));

    for &context in KeybindingContext::ALL {
        let entries = resolved_keybinding_entries(context);
        if entries.is_empty() {
            continue;
        }
        append_help_section_owned(&mut lines, width, context_display_name(context), &entries);
    }

    append_help_section_owned(
        &mut lines,
        width,
        "OVERLAY NAVIGATION",
        &[
            ("j / ↓".to_string(), "scroll down".to_string()),
            ("k / ↑".to_string(), "scroll up".to_string()),
            ("f / space / pgdn".to_string(), "page down".to_string()),
            ("b / pgup".to_string(), "page up".to_string()),
            ("g / home".to_string(), "jump to top".to_string()),
            ("G / end".to_string(), "jump to bottom".to_string()),
            ("q / esc".to_string(), "close overlay".to_string()),
        ],
    );

    append_help_section_owned(
        &mut lines,
        width,
        "VIM NORMAL MODE",
        &[
            ("i".to_string(), "enter insert mode".to_string()),
            ("a".to_string(), "append after cursor".to_string()),
            ("I".to_string(), "insert at line start".to_string()),
            ("A".to_string(), "append at line end".to_string()),
            ("o / O".to_string(), "open line below / above".to_string()),
            ("h / l".to_string(), "move left / right".to_string()),
            ("j / k".to_string(), "move down / up".to_string()),
            (
                "w / b / e".to_string(),
                "word forward / back / end".to_string(),
            ),
            ("d".to_string(), "delete operator".to_string()),
            ("c".to_string(), "change operator".to_string()),
            ("y".to_string(), "yank operator".to_string()),
            ("p / P".to_string(), "paste after / before".to_string()),
            ("u".to_string(), "undo".to_string()),
            (".".to_string(), "repeat last change".to_string()),
            ("x".to_string(), "delete character".to_string()),
            ("r".to_string(), "replace character".to_string()),
            ("f / F / t / T".to_string(), "find character".to_string()),
            (
                "gg / G".to_string(),
                "jump to first / last line".to_string(),
            ),
            (
                "0 / ^ / $".to_string(),
                "line start / first-char / end".to_string(),
            ),
            ("~".to_string(), "toggle case".to_string()),
            ("J".to_string(), "join lines".to_string()),
        ],
    );

    while lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    lines
}
