use crossterm::event::KeyCode;

pub(crate) struct SelectedIndex<'a> {
    selected: &'a mut usize,
    len: usize,
}

impl<'a> SelectedIndex<'a> {
    pub(crate) fn new(selected: &'a mut usize, len: usize) -> Self {
        Self { selected, len }
    }

    pub(crate) fn apply_key(
        &mut self,
        code: KeyCode,
        page_step: Option<usize>,
        vi_keys: bool,
    ) -> bool {
        match code {
            KeyCode::Up | KeyCode::Char('k') if vi_keys => self.previous(),
            KeyCode::Up => self.previous(),
            KeyCode::Down | KeyCode::Char('j') if vi_keys => self.next(),
            KeyCode::Down => self.next(),
            KeyCode::PageUp => page_step.is_some_and(|step| self.page_up(step)),
            KeyCode::PageDown => page_step.is_some_and(|step| self.page_down(step)),
            KeyCode::Home => self.first(),
            KeyCode::End => self.last(),
            _ => false,
        }
    }

    fn previous(&mut self) -> bool {
        let previous = *self.selected;
        *self.selected = self.selected.saturating_sub(1);
        *self.selected != previous
    }

    fn next(&mut self) -> bool {
        let previous = *self.selected;
        *self.selected = self
            .selected
            .saturating_add(1)
            .min(self.len.saturating_sub(1));
        *self.selected != previous
    }

    fn page_up(&mut self, step: usize) -> bool {
        let previous = *self.selected;
        *self.selected = self.selected.saturating_sub(step);
        *self.selected != previous
    }

    fn page_down(&mut self, step: usize) -> bool {
        let previous = *self.selected;
        *self.selected = self
            .selected
            .saturating_add(step)
            .min(self.len.saturating_sub(1));
        *self.selected != previous
    }

    fn first(&mut self) -> bool {
        let previous = *self.selected;
        *self.selected = 0;
        *self.selected != previous
    }

    fn last(&mut self) -> bool {
        let previous = *self.selected;
        *self.selected = self.len.saturating_sub(1);
        *self.selected != previous
    }
}
