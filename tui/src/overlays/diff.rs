use super::*;
use crate::numeric::saturating_u16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffOverlayAction {
    None,
    Close,
}

#[derive(Clone, Debug)]
pub(crate) struct DiffOverlayState {
    pub(crate) diff: WorkspaceDiff,
    pub(crate) mode: DiffOverlayMode,
    pub(crate) selected_file: usize,
    pub(crate) line_scroll: usize,
    pub(crate) max_line_scroll: usize,
    pub(crate) files_cache: DiffOverlayFilesCache,
    pub(crate) file_content_cache: DiffOverlayFileContentCache,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DiffOverlayFilesCache {
    pub(crate) unstaged_files: Option<Vec<DiffFile>>,
    pub(crate) staged_files: Option<Vec<DiffFile>>,
    #[cfg(test)]
    pub(crate) hits: u64,
    #[cfg(test)]
    pub(crate) misses: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DiffOverlayFileContentCache {
    key: Option<DiffOverlayFileContentCacheKey>,
    line_no_width: usize,
    extension: String,
    syntax_lines: Option<Vec<Vec<Span<'static>>>>,
    #[cfg(test)]
    pub(crate) hits: u64,
    #[cfg(test)]
    pub(crate) misses: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffOverlayFileContentCacheKey {
    mode: DiffOverlayMode,
    selected_file: usize,
    path: String,
    line_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffOverlayMode {
    Unstaged,
    Staged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffFile {
    pub(crate) path: String,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) lines: Vec<DiffRenderLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffRenderLine {
    pub(crate) old_line: Option<usize>,
    pub(crate) new_line: Option<usize>,
    pub(crate) marker: char,
    pub(crate) content: String,
    pub(crate) kind: DiffLineKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffLineKind {
    Context,
    Added,
    Removed,
    Separator,
    Note,
}

impl DiffOverlayState {
    pub(crate) fn new(diff: WorkspaceDiff) -> Self {
        let mut state = Self {
            mode: if diff.unstaged_diff.trim().is_empty() && !diff.staged_diff.trim().is_empty() {
                DiffOverlayMode::Staged
            } else {
                DiffOverlayMode::Unstaged
            },
            diff,
            selected_file: 0,
            line_scroll: 0,
            max_line_scroll: 0,
            files_cache: DiffOverlayFilesCache::default(),
            file_content_cache: DiffOverlayFileContentCache::default(),
        };
        state.clamp_selection();
        state
    }

    #[cfg(test)]
    pub(crate) fn files(&self) -> Vec<DiffFile> {
        diff_files_for_mode(&self.diff, self.mode)
    }

    pub(crate) fn cached_files(&mut self) -> &[DiffFile] {
        self.ensure_cached_files();
        match self.mode {
            DiffOverlayMode::Unstaged => self.files_cache.unstaged_files.as_deref().unwrap_or(&[]),
            DiffOverlayMode::Staged => self.files_cache.staged_files.as_deref().unwrap_or(&[]),
        }
    }

    fn ensure_cached_files(&mut self) {
        let needs_build = match self.mode {
            DiffOverlayMode::Unstaged => self.files_cache.unstaged_files.is_none(),
            DiffOverlayMode::Staged => self.files_cache.staged_files.is_none(),
        };
        if needs_build {
            let files = diff_files_for_mode(&self.diff, self.mode);
            match self.mode {
                DiffOverlayMode::Unstaged => self.files_cache.unstaged_files = Some(files),
                DiffOverlayMode::Staged => self.files_cache.staged_files = Some(files),
            }
            #[cfg(test)]
            {
                self.files_cache.misses += 1;
            }
        } else {
            #[cfg(test)]
            {
                self.files_cache.hits += 1;
            }
        }
    }

    fn cached_file_count(&mut self) -> usize {
        self.cached_files().len()
    }

    pub(crate) fn cached_visible_lines(&mut self, area: Rect) -> Vec<StyledLine> {
        if area.height == 0 {
            return Vec::new();
        }

        let visible_height = area.height as usize;
        let available_width = area.width as usize;
        let mode = self.mode;
        let selected_file = self.selected_file;
        let line_scroll = self.line_scroll;
        self.ensure_cached_files();

        let files = match mode {
            DiffOverlayMode::Unstaged => self.files_cache.unstaged_files.as_deref().unwrap_or(&[]),
            DiffOverlayMode::Staged => self.files_cache.staged_files.as_deref().unwrap_or(&[]),
        };
        if files.is_empty() {
            return vec![Line::from(Span::styled(
                "No changes in this diff.",
                subtle_style(),
            ))];
        }

        let selected_file = selected_file.min(files.len().saturating_sub(1));
        let file = &files[selected_file];
        let key = DiffOverlayFileContentCacheKey {
            mode,
            selected_file,
            path: file.path.clone(),
            line_count: file.lines.len(),
        };
        if self.file_content_cache.key.as_ref() != Some(&key) {
            self.file_content_cache.line_no_width = diff_file_line_no_width(file);
            self.file_content_cache.extension = diff_file_extension(file);
            self.file_content_cache.syntax_lines =
                diff_file_syntax_lines(file, &self.file_content_cache.extension);
            self.file_content_cache.key = Some(key);
            #[cfg(test)]
            {
                self.file_content_cache.misses += 1;
            }
        } else {
            #[cfg(test)]
            {
                self.file_content_cache.hits += 1;
            }
        }

        file.lines
            .iter()
            .skip(line_scroll)
            .take(visible_height)
            .enumerate()
            .map(|(index, line)| {
                let line_index = line_scroll.saturating_add(index);
                render_diff_line_with_syntax(
                    line,
                    index == 0,
                    self.file_content_cache.line_no_width,
                    available_width,
                    self.file_content_cache
                        .syntax_lines
                        .as_ref()
                        .and_then(|lines| lines.get(line_index).map(Vec::as_slice)),
                    &self.file_content_cache.extension,
                )
            })
            .collect()
    }

    pub(crate) fn mode_title(&self) -> &'static str {
        match self.mode {
            DiffOverlayMode::Unstaged => "Unstaged changes",
            DiffOverlayMode::Staged => "Staged changes",
        }
    }

    pub(crate) fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            DiffOverlayMode::Unstaged => DiffOverlayMode::Staged,
            DiffOverlayMode::Staged => DiffOverlayMode::Unstaged,
        };
        self.selected_file = 0;
        self.line_scroll = 0;
        self.clamp_selection();
    }

    pub(crate) fn move_file(&mut self, delta: isize) {
        let file_count = self.cached_file_count();
        if file_count == 0 {
            self.selected_file = 0;
            return;
        }
        let max = file_count.saturating_sub(1);
        self.selected_file = self.selected_file.saturating_add_signed(delta).min(max);
        self.line_scroll = 0;
    }

    pub(crate) fn scroll_lines(&mut self, delta: isize) {
        self.line_scroll = self
            .line_scroll
            .saturating_add_signed(delta)
            .min(self.max_line_scroll);
    }

    pub(crate) fn selected_line_count(&mut self) -> usize {
        let selected_file = self.selected_file;
        self.ensure_cached_files();
        match self.mode {
            DiffOverlayMode::Unstaged => self.files_cache.unstaged_files.as_deref().unwrap_or(&[]),
            DiffOverlayMode::Staged => self.files_cache.staged_files.as_deref().unwrap_or(&[]),
        }
        .get(selected_file)
        .map_or(0, |file| file.lines.len())
    }

    pub(crate) fn clamp_selection(&mut self) {
        let file_count = self.cached_file_count();
        if file_count == 0 {
            self.selected_file = 0;
            self.line_scroll = 0;
            self.max_line_scroll = 0;
            return;
        }
        self.selected_file = self.selected_file.min(file_count.saturating_sub(1));
        self.line_scroll = self.line_scroll.min(self.max_line_scroll);
    }
}

pub(crate) fn parse_unified_diff_files(diff: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;
    let mut old_line = 0usize;
    let mut new_line = 0usize;

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(DiffFile {
                path: parse_diff_git_path(line),
                added: 0,
                removed: 0,
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("+++ b/") {
            if let Some(file) = current.as_mut() {
                file.path = path.to_string();
            }
            continue;
        }

        if let Some((old_start, new_start)) = parse_diff_hunk_header(line) {
            old_line = old_start;
            new_line = new_start;
            if let Some(file) = current.as_mut()
                && !file.lines.is_empty()
            {
                file.lines.push(DiffRenderLine {
                    old_line: None,
                    new_line: None,
                    marker: ' ',
                    content: String::new(),
                    kind: DiffLineKind::Separator,
                });
            }
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };
        if line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
        {
            continue;
        }

        if let Some(content) = line.strip_prefix('+') {
            file.added += 1;
            file.lines.push(DiffRenderLine {
                old_line: None,
                new_line: Some(new_line),
                marker: '+',
                content: content.to_string(),
                kind: DiffLineKind::Added,
            });
            new_line += 1;
        } else if let Some(content) = line.strip_prefix('-') {
            file.removed += 1;
            file.lines.push(DiffRenderLine {
                old_line: Some(old_line),
                new_line: None,
                marker: '-',
                content: content.to_string(),
                kind: DiffLineKind::Removed,
            });
            old_line += 1;
        } else if let Some(content) = line.strip_prefix(' ') {
            file.lines.push(DiffRenderLine {
                old_line: Some(old_line),
                new_line: Some(new_line),
                marker: ' ',
                content: content.to_string(),
                kind: DiffLineKind::Context,
            });
            old_line += 1;
            new_line += 1;
        } else if line.starts_with("\\ No newline") {
            file.lines.push(DiffRenderLine {
                old_line: None,
                new_line: None,
                marker: ' ',
                content: line.to_string(),
                kind: DiffLineKind::Note,
            });
        }
    }

    if let Some(file) = current {
        files.push(file);
    }

    files
}

#[cfg(test)]
pub(crate) fn diff_files_for_overlay(diff: &DiffOverlayState) -> Vec<DiffFile> {
    diff.files()
}

fn diff_files_for_mode(diff: &WorkspaceDiff, mode: DiffOverlayMode) -> Vec<DiffFile> {
    let mut files = match mode {
        DiffOverlayMode::Unstaged => parse_unified_diff_files(&diff.unstaged_diff),
        DiffOverlayMode::Staged => parse_unified_diff_files(&diff.staged_diff),
    };
    if mode == DiffOverlayMode::Unstaged {
        for path in &diff.untracked_files {
            if files.iter().any(|file| file.path == *path) {
                continue;
            }
            files.push(DiffFile {
                path: path.clone(),
                added: 0,
                removed: 0,
                lines: vec![DiffRenderLine {
                    old_line: None,
                    new_line: None,
                    marker: ' ',
                    content: "Untracked file. Contents are not shown until staged.".to_string(),
                    kind: DiffLineKind::Note,
                }],
            });
        }
    }
    files
}

fn parse_diff_git_path(line: &str) -> String {
    line.split_whitespace()
        .nth(3)
        .and_then(|path| path.strip_prefix("b/"))
        .unwrap_or("unknown")
        .to_string()
}

fn parse_diff_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let mut parts = rest[..end].split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((parse_hunk_start(old), parse_hunk_start(new)))
}

fn parse_hunk_start(value: &str) -> usize {
    value
        .split(',')
        .next()
        .and_then(|line| line.parse::<usize>().ok())
        .unwrap_or(0)
}

pub(crate) fn apply_diff_overlay_key(
    diff: &mut DiffOverlayState,
    key_event: &KeyEvent,
) -> DiffOverlayAction {
    let control = key_event.modifiers.contains(KeyModifiers::CONTROL);
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') => DiffOverlayAction::Close,
        KeyCode::Left | KeyCode::Char('h') => {
            diff.move_file(-1);
            DiffOverlayAction::None
        }
        KeyCode::Right | KeyCode::Char('l') => {
            diff.move_file(1);
            DiffOverlayAction::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            diff.scroll_lines(-1);
            DiffOverlayAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            diff.scroll_lines(1);
            DiffOverlayAction::None
        }
        KeyCode::PageUp => {
            diff.scroll_lines(-12);
            DiffOverlayAction::None
        }
        KeyCode::Char('b') if control => {
            diff.scroll_lines(-12);
            DiffOverlayAction::None
        }
        KeyCode::PageDown | KeyCode::Char('f') => {
            diff.scroll_lines(12);
            DiffOverlayAction::None
        }
        KeyCode::Char('b') => {
            diff.toggle_mode();
            DiffOverlayAction::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            diff.line_scroll = 0;
            DiffOverlayAction::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            diff.line_scroll = diff.max_line_scroll;
            DiffOverlayAction::None
        }
        _ => DiffOverlayAction::None,
    }
}

pub(crate) fn sync_diff_overlay_bounds(diff: &mut DiffOverlayState, area: Rect) {
    let visible_height = diff_overlay_content_height(area);
    let line_count = diff.selected_line_count();
    diff.max_line_scroll = line_count.saturating_sub(visible_height);
    diff.clamp_selection();
}

pub(crate) fn diff_overlay_content_height(area: Rect) -> usize {
    area.height.saturating_sub(8) as usize
}

pub(crate) fn draw_diff_overlay(frame: &mut Frame, diff: &mut DiffOverlayState, area: Rect) {
    frame.render_widget(Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }

    let title_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(diff_overlay_title_line(diff)), title_area);

    let tab_area = Rect {
        x: area.x,
        y: area.y.saturating_add(2),
        width: area.width,
        height: 1,
    };
    let tab_line = {
        let selected_file = diff.selected_file;
        let files = diff.cached_files();
        diff_overlay_tab_line_for_selection(selected_file, files, tab_area.width as usize)
    };
    frame.render_widget(Paragraph::new(tab_line), tab_area);

    let panel_top = area.y.saturating_add(4);
    let panel_height = area.height.saturating_sub(6);
    let panel_area = Rect {
        x: area.x.saturating_add(2),
        y: panel_top,
        width: area.width.saturating_sub(4),
        height: panel_height,
    };
    if panel_area.width > 0 && panel_area.height > 0 {
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(subtle_style()),
            panel_area,
        );
        let content_area = Rect {
            x: panel_area.x.saturating_add(2),
            y: panel_area.y.saturating_add(1),
            width: panel_area.width.saturating_sub(6),
            height: panel_area.height.saturating_sub(2),
        };
        let lines = diff.cached_visible_lines(content_area);
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            content_area,
        );
        draw_diff_scrollbar(frame, diff, panel_area, content_area.height as usize);
    }

    let footer_area = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::styled("←→", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" switch files · ", subtle_style()),
            Span::styled("↑↓", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" navigate lines · ", subtle_style()),
            Span::styled("b", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" switch branch diff · ", subtle_style()),
            Span::styled("Esc", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" exit", subtle_style()),
        ])])
        .wrap(Wrap { trim: false }),
        footer_area,
    );
}

pub(crate) fn diff_overlay_title_line(diff: &DiffOverlayState) -> StyledLine {
    Line::from(vec![
        Span::styled("Diff Mode", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" — ", subtle_style()),
        Span::styled(diff.mode_title(), subtle_style()),
    ])
}

#[cfg(test)]
pub(crate) fn diff_overlay_tab_line(
    diff: &DiffOverlayState,
    files: &[DiffFile],
    terminal_width: usize,
) -> StyledLine {
    diff_overlay_tab_line_for_selection(diff.selected_file, files, terminal_width)
}

pub(crate) fn diff_overlay_tab_line_for_selection(
    selected_file: usize,
    files: &[DiffFile],
    terminal_width: usize,
) -> StyledLine {
    if files.is_empty() {
        return Line::from(Span::styled("No changes", subtle_style()));
    }

    let selected = selected_file.min(files.len().saturating_sub(1));
    let selected_file = &files[selected];
    let suffix = format!(
        "[{}/{}]  +{} -{}",
        selected + 1,
        files.len(),
        selected_file.added,
        selected_file.removed
    );
    let suffix_width = display_width_str(&suffix);
    let tab_budget = terminal_width.saturating_sub(suffix_width.saturating_add(3));
    let mut used = 0usize;
    let mut spans = Vec::new();
    let labels = diff_tab_labels(files);
    let start = diff_tab_window_start(&labels, selected, tab_budget);

    for (index, label) in labels.iter().enumerate().skip(start) {
        let label = truncate_chars(label, 28);
        let padded = format!(" {label} ");
        let width = display_width_str(&padded).saturating_add(2);
        if used > 0 && used.saturating_add(width) > tab_budget {
            break;
        }
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
            used += 2;
        }
        let style = if index == selected {
            inactive_style().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            subtle_style()
        };
        spans.push(Span::styled(padded, style));
        used += width.saturating_sub(2);
    }

    let padding = tab_budget.saturating_sub(used);
    spans.push(Span::raw(" ".repeat(padding.saturating_add(1))));
    spans.push(Span::styled(
        format!("[{}/{}]", selected + 1, files.len()),
        inactive_style().add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled("  ", subtle_style()));
    spans.push(Span::styled(
        format!("+{}", selected_file.added),
        Style::default()
            .fg(active_palette().success)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" ", subtle_style()));
    spans.push(Span::styled(
        format!("-{}", selected_file.removed),
        warning_style().add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

fn diff_tab_window_start(labels: &[String], selected: usize, tab_budget: usize) -> usize {
    if labels.is_empty() || selected == 0 {
        return 0;
    }
    let mut start = selected;
    let mut used = display_width_str(&format!(" {} ", labels[selected].as_str()));
    while start > 0 {
        let previous_width = display_width_str(&format!(" {} ", labels[start - 1].as_str()));
        let separator_width = 2;
        if used
            .saturating_add(previous_width)
            .saturating_add(separator_width)
            > tab_budget
        {
            break;
        }
        start -= 1;
        used = used
            .saturating_add(previous_width)
            .saturating_add(separator_width);
    }
    start
}

pub(crate) fn diff_tab_labels(files: &[DiffFile]) -> Vec<String> {
    let basenames = files
        .iter()
        .map(|file| diff_path_basename(&file.path))
        .collect::<Vec<_>>();
    let mut labels = basenames.clone();
    for component_count in 2..=8 {
        let mut changed = false;
        for index in 0..files.len() {
            if basenames
                .iter()
                .enumerate()
                .any(|(other_index, other)| other_index != index && other == &basenames[index])
            {
                labels[index] = diff_path_tail(&files[index].path, component_count);
                changed = true;
            }
        }
        if !changed || labels_are_unique(&labels) {
            break;
        }
    }
    labels
}

fn labels_are_unique(labels: &[String]) -> bool {
    let mut seen = HashSet::new();
    labels.iter().all(|label| seen.insert(label))
}

fn diff_path_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn diff_path_tail(path: &str, components: usize) -> String {
    let parts = Path::new(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str().map(str::to_string),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.len() <= components {
        path.to_string()
    } else {
        parts[parts.len().saturating_sub(components)..].join("/")
    }
}

#[cfg(test)]
pub(crate) fn diff_overlay_visible_lines(
    diff: &DiffOverlayState,
    files: &[DiffFile],
    area: Rect,
) -> Vec<StyledLine> {
    if area.height == 0 {
        return Vec::new();
    }
    if files.is_empty() {
        return vec![Line::from(Span::styled(
            "No changes in this diff.",
            subtle_style(),
        ))];
    }

    let file = &files[diff.selected_file.min(files.len().saturating_sub(1))];
    let line_no_width = diff_file_line_no_width(file);
    let extension = diff_file_extension(file);
    let syntax_lines = diff_file_syntax_lines(file, &extension);

    file.lines
        .iter()
        .skip(diff.line_scroll)
        .take(area.height as usize)
        .enumerate()
        .map(|(index, line)| {
            let line_index = diff.line_scroll.saturating_add(index);
            render_diff_line_with_syntax(
                line,
                index == 0,
                line_no_width,
                area.width as usize,
                syntax_lines
                    .as_ref()
                    .and_then(|lines| lines.get(line_index).map(Vec::as_slice)),
                &extension,
            )
        })
        .collect()
}

pub(crate) fn diff_file_line_no_width(file: &DiffFile) -> usize {
    file.lines
        .iter()
        .filter_map(|line| line.new_line.or(line.old_line))
        .max()
        .unwrap_or(1)
        .to_string()
        .chars()
        .count()
        .max(2)
}

pub(crate) fn diff_file_extension(file: &DiffFile) -> String {
    Path::new(&file.path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn diff_file_syntax_lines(
    file: &DiffFile,
    extension: &str,
) -> Option<Vec<Vec<Span<'static>>>> {
    let mut content = String::new();
    let mut line_count = 0usize;
    for line in &file.lines {
        line_count += 1;
        if matches!(
            line.kind,
            DiffLineKind::Added | DiffLineKind::Removed | DiffLineKind::Context
        ) {
            content.push_str(&line.content);
        }
        content.push('\n');
    }
    if syntax_highlight::exceeds_highlight_limits(content.len(), line_count) {
        return None;
    }
    syntax_highlight::highlight_code_to_styled_spans(&content, extension)
}

#[cfg(test)]
pub(crate) fn render_diff_line(
    line: &DiffRenderLine,
    selected: bool,
    line_no_width: usize,
    extension: &str,
) -> StyledLine {
    render_diff_line_with_syntax(line, selected, line_no_width, 100, None, extension)
}

pub(crate) fn render_diff_line_with_syntax(
    line: &DiffRenderLine,
    selected: bool,
    line_no_width: usize,
    available_width: usize,
    syntax_spans: Option<&[Span<'static>]>,
    extension: &str,
) -> StyledLine {
    render_diff_line_with_palette(
        line,
        selected,
        line_no_width,
        available_width,
        syntax_spans,
        extension,
        active_palette(),
    )
}

pub(crate) fn render_diff_line_with_palette(
    line: &DiffRenderLine,
    selected: bool,
    line_no_width: usize,
    available_width: usize,
    syntax_spans: Option<&[Span<'static>]>,
    extension: &str,
    palette: TuiPalette,
) -> StyledLine {
    if line.kind == DiffLineKind::Separator {
        return Line::from(vec![Span::styled(
            "─".repeat(available_width.max(1)),
            subtle_style(),
        )]);
    }

    let line_number = line.new_line.or(line.old_line).map_or_else(
        || " ".repeat(line_no_width),
        |line| format!("{line:>line_no_width$}"),
    );
    let row_style = diff_line_row_style_with_palette(line.kind, palette);
    let indicator = if selected { "›" } else { " " };
    let marker_style = match line.kind {
        DiffLineKind::Added => Style::default()
            .fg(palette.success)
            .bg(palette.diff_added_bg)
            .add_modifier(Modifier::BOLD),
        DiffLineKind::Removed => warning_style()
            .fg(palette.warning)
            .bg(palette.diff_removed_bg)
            .add_modifier(Modifier::BOLD),
        _ => subtle_style(),
    };
    let mut spans = vec![
        Span::styled(indicator, row_style),
        Span::styled("  ", row_style),
        Span::styled(
            line_number,
            apply_diff_line_bg_with_palette(subtle_style(), line.kind, palette),
        ),
        Span::styled(" ", row_style),
        Span::styled(line.marker.to_string(), marker_style),
        Span::styled("  ", row_style),
    ];
    if line.kind == DiffLineKind::Note {
        spans.push(Span::styled(line.content.clone(), subtle_style()));
    } else {
        let content_spans = syntax_spans.map_or_else(
            || highlight_code_line(&line.content, extension),
            <[ratatui::prelude::Span<'_>]>::to_vec,
        );
        spans.extend(content_spans.into_iter().map(|span| {
            Span::styled(
                span.content,
                apply_diff_line_bg_with_palette(span.style, line.kind, palette),
            )
        }));
    }
    Line::from(spans)
}

fn diff_line_row_style_with_palette(kind: DiffLineKind, palette: TuiPalette) -> Style {
    match kind {
        DiffLineKind::Added => Style::default().bg(palette.diff_added_bg),
        DiffLineKind::Removed => Style::default().bg(palette.diff_removed_bg),
        _ => Style::default(),
    }
}

fn apply_diff_line_bg_with_palette(style: Style, kind: DiffLineKind, palette: TuiPalette) -> Style {
    match kind {
        DiffLineKind::Added => style.bg(palette.diff_added_bg),
        DiffLineKind::Removed => style.bg(palette.diff_removed_bg),
        _ => style,
    }
}

fn draw_diff_scrollbar(
    frame: &mut Frame,
    diff: &DiffOverlayState,
    panel_area: Rect,
    visible_height: usize,
) {
    if diff.max_line_scroll == 0 || visible_height == 0 || panel_area.height <= 2 {
        return;
    }
    let track_height = panel_area.height.saturating_sub(2) as usize;
    let thumb_len = (track_height * visible_height)
        .div_ceil(visible_height.saturating_add(diff.max_line_scroll))
        .clamp(1, track_height);
    let max_thumb_start = track_height.saturating_sub(thumb_len);
    let thumb_start = (diff.line_scroll * max_thumb_start + diff.max_line_scroll / 2)
        .checked_div(diff.max_line_scroll)
        .unwrap_or(0);
    let x = panel_area.right().saturating_sub(2);
    for row in 0..track_height {
        let y = panel_area
            .y
            .saturating_add(1)
            .saturating_add(saturating_u16(row));
        let active = row >= thumb_start && row < thumb_start.saturating_add(thumb_len);
        frame.render_widget(
            Paragraph::new(Span::styled(
                "│",
                if active {
                    inactive_style()
                } else {
                    subtle_style()
                },
            )),
            Rect {
                x,
                y,
                width: 1,
                height: 1,
            },
        );
    }
}
