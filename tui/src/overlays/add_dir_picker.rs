use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AddDirPickerKeyAction {
    None,
    Close,
    AddDirectory { command: String, path: PathBuf },
}

#[derive(Clone, Debug)]
pub(crate) struct AddDirPickerState {
    pub(crate) command: String,
    pub(crate) items: Vec<AddDirPickerItem>,
    pub(crate) selected: usize,
    pub(crate) lines_cache: AddDirPickerLinesCache,
}

pub(crate) type AddDirPickerLinesCache = LinesCache<AddDirPickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddDirPickerLinesCacheKey {
    width: usize,
    selected: usize,
    items: Vec<AddDirPickerItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddDirPickerItem {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
}

impl AddDirPickerState {
    pub(crate) fn new(command: impl Into<String>, cwd: &Path) -> Self {
        let mut items = Vec::new();

        if let Some(parent) = cwd.parent() {
            items.push(AddDirPickerItem {
                label: "..".to_string(),
                path: parent.to_path_buf(),
            });
        }

        if let Ok(entries) = std::fs::read_dir(cwd) {
            let mut subdirs: Vec<AddDirPickerItem> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    let file_type = entry.file_type().ok()?;
                    if !file_type.is_dir() {
                        return None;
                    }
                    let name = entry.file_name().to_str()?.to_string();
                    if name.starts_with('.') {
                        return None;
                    }
                    Some(AddDirPickerItem { label: name, path })
                })
                .collect();
            subdirs.sort_by(|a, b| a.label.cmp(&b.label));
            items.extend(subdirs);
        }

        Self {
            command: command.into(),
            items,
            selected: 0,
            lines_cache: AddDirPickerLinesCache::default(),
        }
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = AddDirPickerLinesCacheKey {
            width,
            selected: self.selected,
            items: self.items.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || add_dir_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

pub(crate) fn apply_add_dir_picker_key(
    picker: &mut AddDirPickerState,
    key_event: &KeyEvent,
) -> AddDirPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => AddDirPickerKeyAction::Close,
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
            AddDirPickerKeyAction::None
        }
        KeyCode::Enter => match picker.items.get(picker.selected) {
            Some(item) => AddDirPickerKeyAction::AddDirectory {
                command: picker.command.clone(),
                path: item.path.clone(),
            },
            None => AddDirPickerKeyAction::None,
        },
        _ => AddDirPickerKeyAction::None,
    }
}

fn add_dir_picker_lines(picker: &AddDirPickerState, width: usize) -> Vec<StyledLine> {
    let item_width = width.saturating_sub(8).max(8);
    let mut lines = vec![
        Line::from(vec![Span::styled("Add directory", highlight_style())]),
        Line::default(),
    ];
    if picker.items.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("No subdirectories found.", subtle_style()),
        ]));
    } else {
        for (index, item) in picker.items.iter().enumerate() {
            let selected = index == picker.selected;
            let marker = if selected { ">" } else { " " };
            let text = truncate_chars(&item.label, item_width);
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
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(marker, marker_style),
                Span::raw(" "),
                Span::styled(text, text_style),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw("Enter"),
        Span::styled(" to add", subtle_style()),
        Span::styled(" · ", subtle_style()),
        Span::raw("Esc"),
        Span::styled(" to cancel", subtle_style()),
    ]));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn picker_lists_subdirectories_and_parent() {
        let temp = tempdir().unwrap();
        let cwd = temp.path();
        std::fs::create_dir(cwd.join("alpha")).unwrap();
        std::fs::create_dir(cwd.join("beta")).unwrap();
        std::fs::create_dir(cwd.join(".hidden")).unwrap();
        std::fs::write(cwd.join("file.txt"), "not a dir").unwrap();

        let state = AddDirPickerState::new("/add-dir", cwd);
        let labels: Vec<&str> = state.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels[0], "..");
        assert!(labels.contains(&"alpha"));
        assert!(labels.contains(&"beta"));
        assert!(!labels.contains(&".hidden"));
        assert!(!labels.contains(&"file.txt"));
    }

    #[test]
    fn picker_items_sorted_alphabetically() {
        let temp = tempdir().unwrap();
        let cwd = temp.path();
        std::fs::create_dir(cwd.join("zebra")).unwrap();
        std::fs::create_dir(cwd.join("apple")).unwrap();
        std::fs::create_dir(cwd.join("mango")).unwrap();

        let state = AddDirPickerState::new("/add-dir", cwd);
        let labels: Vec<&str> = state
            .items
            .iter()
            .skip(1) // skip ".."
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(labels, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn empty_directory_shows_only_parent() {
        let temp = tempdir().unwrap();
        let cwd = temp.path();

        let state = AddDirPickerState::new("/add-dir", cwd);
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].label, "..");
    }

    #[test]
    fn enter_selects_directory() {
        let temp = tempdir().unwrap();
        let cwd = temp.path();
        std::fs::create_dir(cwd.join("target")).unwrap();

        let mut state = AddDirPickerState::new("/add-dir", cwd);
        state.selected = 1; // skip "..", select "target"

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        match apply_add_dir_picker_key(&mut state, &key) {
            AddDirPickerKeyAction::AddDirectory { path, .. } => {
                assert_eq!(path, cwd.join("target"));
            }
            other => panic!("expected AddDirectory, got {other:?}"),
        }
    }
}
