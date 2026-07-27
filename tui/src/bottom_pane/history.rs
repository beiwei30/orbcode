use crate::state::TuiState;

impl TuiState {
    pub(crate) fn navigate_prompt_up(&mut self) -> bool {
        if self.prompt_history_index.is_none() && self.move_cursor_logical_vertical(-1) {
            return true;
        }
        self.navigate_prompt_history_older()
    }

    pub(crate) fn navigate_prompt_down(&mut self) -> bool {
        if self.prompt_history_index.is_some() {
            return self.navigate_prompt_history_newer();
        }
        self.move_cursor_logical_vertical(1)
    }

    pub(crate) fn remember_prompt_history(&mut self, prompt: &str) {
        if prompt.trim().is_empty() {
            return;
        }
        self.prompt_history.retain(|entry| entry != prompt);
        self.prompt_history.insert(0, prompt.to_string());
        self.prompt_history.truncate(100);
        self.prompt_history_index = None;
    }

    pub(crate) fn navigate_prompt_history_older(&mut self) -> bool {
        if self.prompt_history.is_empty() {
            return false;
        }

        let next_index = self
            .prompt_history_index
            .map_or(0, |index| (index + 1).min(self.prompt_history.len() - 1));
        self.prompt_history_index = Some(next_index);
        self.input = self.prompt_history[next_index].clone();
        self.input_cursor = self.input.len();
        self.input_tail_pinned = false;
        self.set_status_line(format!(
            "History {}/{}",
            next_index + 1,
            self.prompt_history.len()
        ));
        true
    }

    pub(crate) fn navigate_prompt_history_newer(&mut self) -> bool {
        let Some(index) = self.prompt_history_index else {
            return false;
        };

        if index == 0 {
            self.prompt_history_index = None;
            self.input.clear();
            self.input_cursor = 0;
            self.input_tail_pinned = false;
            self.slash_command_selected = 0;
            self.set_status_line("Exited prompt history.");
        } else {
            let next_index = index - 1;
            self.prompt_history_index = Some(next_index);
            self.input = self.prompt_history[next_index].clone();
            self.input_cursor = self.input.len();
            self.input_tail_pinned = false;
            self.set_status_line(format!(
                "History {}/{}",
                next_index + 1,
                self.prompt_history.len()
            ));
        }
        true
    }
}
