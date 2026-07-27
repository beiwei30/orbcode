use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MemoryPickerKeyAction {
    None,
    Close,
    EditMemory { command: String, path: PathBuf },
    OpenPath { command: String, path: PathBuf },
}

#[derive(Clone, Debug)]
pub(crate) struct MemoryPickerState {
    pub(crate) command: String,
    pub(crate) items: Vec<MemoryPickerItem>,
    pub(crate) auto_memory_enabled: bool,
    pub(crate) selected: usize,
    pub(crate) lines_cache: MemoryPickerLinesCache,
}

pub(crate) type MemoryPickerLinesCache = LinesCache<MemoryPickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MemoryPickerLinesCacheKey {
    cwd: PathBuf,
    width: usize,
    selected: usize,
    auto_memory_enabled: bool,
    items: Vec<MemoryPickerItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MemoryPickerItem {
    File(MemoryFileOverview),
    OpenAutoMemoryFolder(PathBuf),
}

impl MemoryPickerState {
    pub(crate) fn new(command: impl Into<String>, overview: MemoryOverview) -> Self {
        let mut items = Vec::with_capacity(overview.project_memories.len() + 2);
        items.push(MemoryPickerItem::File(overview.user_memory));
        items.extend(
            overview
                .project_memories
                .into_iter()
                .map(MemoryPickerItem::File),
        );
        if overview.auto_memory_enabled {
            items.push(MemoryPickerItem::OpenAutoMemoryFolder(
                overview.auto_memory_dir,
            ));
        }
        Self {
            command: command.into(),
            items,
            auto_memory_enabled: overview.auto_memory_enabled,
            selected: 0,
            lines_cache: MemoryPickerLinesCache::default(),
        }
    }

    pub(crate) fn cached_lines(&mut self, cwd: &Path, width: usize) -> &[StyledLine] {
        let key = MemoryPickerLinesCacheKey {
            cwd: cwd.to_path_buf(),
            width,
            selected: self.selected,
            auto_memory_enabled: self.auto_memory_enabled,
            items: self.items.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || memory_picker_lines(self, cwd, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

pub(crate) fn apply_memory_picker_key(
    picker: &mut MemoryPickerState,
    key_event: &KeyEvent,
) -> MemoryPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => MemoryPickerKeyAction::Close,
        KeyCode::Up
        | KeyCode::Char('k' | 'j')
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            SelectedIndex::new(&mut picker.selected, picker.items.len()).apply_key(
                key_event.code,
                Some(8),
                true,
            );
            MemoryPickerKeyAction::None
        }
        KeyCode::Enter => match picker.items.get(picker.selected) {
            Some(MemoryPickerItem::File(memory)) if memory.writable => {
                MemoryPickerKeyAction::EditMemory {
                    command: picker.command.clone(),
                    path: memory.path.clone(),
                }
            }
            Some(MemoryPickerItem::File(_)) => MemoryPickerKeyAction::None,
            Some(MemoryPickerItem::OpenAutoMemoryFolder(path)) => MemoryPickerKeyAction::OpenPath {
                command: picker.command.clone(),
                path: path.clone(),
            },
            None => MemoryPickerKeyAction::None,
        },
        _ => MemoryPickerKeyAction::None,
    }
}

pub(crate) fn memory_picker_lines(
    picker: &MemoryPickerState,
    cwd: &Path,
    width: usize,
) -> Vec<StyledLine> {
    let item_width = width.saturating_sub(8).max(8);
    let mut lines = vec![
        Line::from(vec![Span::styled("Memory", highlight_style())]),
        Line::default(),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("Auto-memory: ", subtle_style()),
            Span::raw(if picker.auto_memory_enabled {
                "on"
            } else {
                "off"
            }),
        ]),
        Line::default(),
    ];
    if picker.items.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("No memory files found.", subtle_style()),
        ]));
    } else {
        for (index, item) in picker.items.iter().enumerate() {
            lines.push(format_memory_picker_item_line(
                item,
                index,
                index == picker.selected,
                item_width,
                cwd,
            ));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw("Enter"),
        Span::styled(" to confirm", subtle_style()),
        Span::styled(" · ", subtle_style()),
        Span::raw("Esc"),
        Span::styled(" to cancel", subtle_style()),
    ]));
    lines
}

fn format_memory_picker_item_line(
    item: &MemoryPickerItem,
    index: usize,
    selected: bool,
    max_width: usize,
    cwd: &Path,
) -> StyledLine {
    let marker = if selected { "❯" } else { " " };
    let text = match item {
        MemoryPickerItem::File(memory) => {
            let access = if memory.writable { "edit" } else { "read-only" };
            let mut text = format!(
                "{}. {} [{}, {}] ({})",
                index + 1,
                memory.label,
                access,
                memory.status.as_label(),
                memory_picker_display_path(memory, cwd)
            );
            if let Some(reason) = memory.skipped_reason.as_deref() {
                text.push_str(" - ");
                text.push_str(reason);
            }
            text
        }
        MemoryPickerItem::OpenAutoMemoryFolder(_) => {
            format!("{}. Open auto-memory folder", index + 1)
        }
    };
    let text = truncate_chars(&text, max_width);
    let marker_style = if selected {
        Style::default()
    } else {
        subtle_style()
    };
    let text_style = if selected {
        Style::default()
    } else {
        subtle_style()
    };
    Line::from(vec![
        Span::raw("    "),
        Span::styled(marker, marker_style),
        Span::raw(" "),
        Span::styled(text, text_style),
    ])
}

fn memory_picker_display_path(memory: &MemoryFileOverview, cwd: &Path) -> String {
    let path = memory.path.as_path();
    if let Ok(relative) = path.strip_prefix(cwd) {
        return format!("./{}", relative.display());
    }
    // Show the real resolved path (honoring `ORBCODE_HOME`/`CLAUDE_CONFIG_DIR`)
    // rather than a hardcoded `~/.claude/CLAUDE.md`, abbreviating the OS home
    // directory to `~` when the path lies under it.
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(relative) = path.strip_prefix(Path::new(&home))
    {
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
}
