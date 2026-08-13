use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpOverlayAction {
    None,
    Close,
    OpenKeybindHelp,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HelpOverlayState {
    pub(crate) scroll: usize,
    pub(crate) max_scroll: usize,
    pub(crate) lines_cache: HelpOverlayLinesCache,
}

type HelpOverlayLinesCache = LinesCache<usize>;

impl HelpOverlayState {
    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let width = width.max(1);
        self.lines_cache
            .refresh(width, || help_overlay_lines(width));
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

pub(crate) fn apply_help_overlay_key(
    help: &mut HelpOverlayState,
    key_event: &KeyEvent,
) -> HelpOverlayAction {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') => HelpOverlayAction::Close,
        KeyCode::Char('?') => HelpOverlayAction::OpenKeybindHelp,
        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            HelpOverlayAction::Close
        }
        KeyCode::Up | KeyCode::Char('k') => {
            scroll_help_overlay_by(help, -1);
            HelpOverlayAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            scroll_help_overlay_by(help, 1);
            HelpOverlayAction::None
        }
        KeyCode::PageUp | KeyCode::Char('b') => {
            scroll_help_overlay_by(help, -(HELP_OVERLAY_PAGE_STEP as isize));
            HelpOverlayAction::None
        }
        KeyCode::PageDown | KeyCode::Char('f' | ' ') => {
            scroll_help_overlay_by(help, HELP_OVERLAY_PAGE_STEP as isize);
            HelpOverlayAction::None
        }
        KeyCode::Char('u') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_help_overlay_by(help, -((HELP_OVERLAY_PAGE_STEP / 2) as isize));
            HelpOverlayAction::None
        }
        KeyCode::Char('d') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_help_overlay_by(help, (HELP_OVERLAY_PAGE_STEP / 2) as isize);
            HelpOverlayAction::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            help.scroll = 0;
            HelpOverlayAction::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            help.scroll = help.max_scroll;
            HelpOverlayAction::None
        }
        _ => HelpOverlayAction::None,
    }
}

pub(crate) fn scroll_help_overlay_by(help: &mut HelpOverlayState, delta: isize) {
    let next = if delta < 0 {
        help.scroll.saturating_sub(delta.unsigned_abs())
    } else {
        help.scroll.saturating_add(delta as usize)
    };
    help.scroll = next.min(help.max_scroll);
}

pub(crate) fn sync_help_overlay_bounds(help: &mut HelpOverlayState, area: Rect) {
    let content_height = help_overlay_content_height(area);
    let line_count = help.cached_lines(area.width.max(1) as usize).len();
    help.max_scroll = line_count.saturating_sub(content_height);
    help.scroll = help.scroll.min(help.max_scroll);
}

pub(crate) fn help_overlay_content_height(area: Rect) -> usize {
    area.height.saturating_sub(1) as usize
}

pub(crate) fn draw_help_overlay(frame: &mut Frame, help: &mut HelpOverlayState, area: Rect) {
    frame.render_widget(Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }

    let content_height = help_overlay_content_height(area);
    let visible_lines = help.cached_visible_lines(area.width.max(1) as usize, content_height);
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
            Span::styled("?", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" keys · ", subtle_style()),
            Span::styled("q", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" close", subtle_style()),
        ])),
        footer_area,
    );
}

pub(crate) fn help_overlay_lines(width: usize) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    append_help_section(
        &mut lines,
        width,
        "GLOBAL",
        &[
            ("/help", "show full help"),
            ("/", "commands"),
            ("ctrl+r", "open session picker"),
            ("ctrl+o", "expand or collapse tool details"),
            ("pgup/pgdn", "scroll timeline"),
            ("home/end", "jump timeline top or bottom"),
            ("j/k", "scroll help when help is open"),
            ("space/f/b", "page help when help is open"),
            ("g/G", "jump help top or bottom"),
            ("esc", "cancel, close, or return to insert mode"),
            ("ctrl+c", "cancel active request or exit"),
            (
                "mouse drag",
                "select transcript or prompt text and copy on release",
            ),
        ],
    );
    append_help_section(
        &mut lines,
        width,
        "INPUT",
        &[
            ("enter", "submit prompt"),
            ("shift+enter", "insert newline"),
            ("alt+enter", "insert newline"),
            ("ctrl+enter", "insert newline"),
            ("tab", "complete slash command or insert spaces"),
            (
                "shift+tab",
                "cycle Ask, Approve, Full Access, and Plan modes",
            ),
            (
                "↑/↓",
                "browse slash commands, move input, or navigate history",
            ),
            ("ctrl+a", "go to line start"),
            ("ctrl+e", "go to line end"),
            ("ctrl+u", "delete current prompt"),
        ],
    );

    let all_commands = slash_commands();
    let command_rows = all_commands
        .iter()
        .filter(|command| !command.hidden && command.source != SlashCommandSource::Provider)
        .map(|command| (command.usage(), command.description.to_string()))
        .collect::<Vec<_>>();
    append_help_section_owned(&mut lines, width, "AGENT ENVIRONMENT", &command_rows);

    let tool_rows = all_commands
        .iter()
        .filter(|command| !command.hidden && command.source == SlashCommandSource::Provider)
        .map(|command| (command.usage(), command.description.to_string()))
        .collect::<Vec<_>>();
    append_help_section_owned(&mut lines, width, "TOOLS", &tool_rows);

    append_help_section(
        &mut lines,
        width,
        "MCP",
        &[
            (
                "/mcp capabilities",
                "show modeled MCP transport capabilities",
            ),
            ("/mcp servers", "list modeled MCP servers"),
            (
                "/mcp resources <server>",
                "list resources from a modeled MCP server",
            ),
            (
                "/mcp tools <server>",
                "list tools from a modeled MCP server",
            ),
            ("/mcp read <server> <uri>", "read a modeled MCP resource"),
            (
                "/mcp call <server> <tool> [input]",
                "call a modeled MCP tool",
            ),
        ],
    );
    while lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    lines
}

fn append_help_section(
    lines: &mut Vec<StyledLine>,
    width: usize,
    title: &'static str,
    rows: &[(&'static str, &'static str)],
) {
    let owned = rows
        .iter()
        .map(|(key, description)| ((*key).to_string(), (*description).to_string()))
        .collect::<Vec<_>>();
    append_help_section_owned(lines, width, title, &owned);
}

pub(super) fn append_help_section_owned(
    lines: &mut Vec<StyledLine>,
    width: usize,
    title: &'static str,
    rows: &[(String, String)],
) {
    if !lines.is_empty() {
        lines.push(Line::default());
        lines.push(Line::default());
    }
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(title, inactive_style().add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::default());

    let key_width = help_key_column_width(width);
    let gutter = if width < 30 { 2 } else { 8 };
    let description_width = width
        .saturating_sub(key_width)
        .saturating_sub(gutter)
        .max(1);
    for (key, description) in rows {
        let key = pad_or_truncate(key, key_width);
        let description = truncate_chars(description, description_width);
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(key, inactive_style()),
            Span::styled("  ", subtle_style()),
            Span::styled(description, empty_transcript_placeholder_style()),
        ]));
    }
}

pub(super) fn help_key_column_width(width: usize) -> usize {
    if width < 30 {
        width.saturating_sub(4).saturating_div(2).max(1)
    } else {
        width.saturating_mul(28).saturating_div(100).clamp(8, 34)
    }
}
