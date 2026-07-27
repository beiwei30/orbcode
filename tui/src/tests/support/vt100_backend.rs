use super::*;

use std::fmt;

pub struct VT100Backend {
    parser: vt100::Parser,
}

impl VT100Backend {
    pub fn new(width: u16, height: u16) -> Self {
        crossterm::style::force_color_output(true);
        Self {
            parser: vt100::Parser::new(height, width, 10_000),
        }
    }

    pub fn vt100(&self) -> &vt100::Parser {
        &self.parser
    }

    pub fn screen_lines(&self) -> Vec<String> {
        let (_, cols) = self.vt100().screen().size();
        self.vt100()
            .screen()
            .rows(0, cols)
            .map(|line| line.trim_end().to_string())
            .collect()
    }

    pub fn screen_contents(&self) -> String {
        self.vt100().screen().contents()
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.parser.set_size(height, width);
    }
}

impl Write for VT100Backend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.parser.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.parser.flush()
    }
}

impl fmt::Display for VT100Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.screen_contents())
    }
}

impl Backend for VT100Backend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        for (x, y, cell) in content {
            self.set_cursor_position(Position { x, y })?;
            self.write_all(cell.symbol().as_bytes())?;
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.write_all(b"\x1b[?25l")
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.write_all(b"\x1b[?25h")
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.vt100().screen().cursor_position().into())
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        write!(self, "\x1b[{};{}H", position.y + 1, position.x + 1)?;
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.write_all(b"\x1b[2J")
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let code = match clear_type {
            ClearType::All => "\x1b[2J",
            ClearType::AfterCursor => "\x1b[0J",
            ClearType::BeforeCursor => "\x1b[1J",
            ClearType::CurrentLine => "\x1b[2K",
            ClearType::UntilNewLine => "\x1b[0K",
        };
        self.write_all(code.as_bytes())
    }

    fn append_lines(&mut self, line_count: u16) -> io::Result<()> {
        for _ in 0..line_count {
            self.write_all(b"\n")?;
        }
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        let (rows, cols) = self.vt100().screen().size();
        Ok(Size::new(cols, rows))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size()?,
            pixels: Size {
                width: 640,
                height: 480,
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.parser.flush()
    }
}

pub struct Vt100TransitionResult {
    pub viewport_area: Rect,
    pub buffer_screen: Vec<String>,
    pub terminal_screen: Vec<String>,
    pub terminal_contents: String,
}

pub fn run_prompt_transition_vt100(
    state: &mut TuiState,
    terminal: &mut Terminal<VT100Backend>,
) -> Vt100TransitionResult {
    prepare_draw_transaction(terminal, state, false).expect("prepare draw transaction");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw frame");
    let terminal_screen = terminal.backend_mut().screen_lines();
    let terminal_contents = terminal.backend_mut().screen_contents();
    Vt100TransitionResult {
        viewport_area: terminal.viewport_area,
        buffer_screen: terminal.screen_lines(),
        terminal_screen,
        terminal_contents,
    }
}

/// Fallback ANSI subset used only for scrollback assertions that vt100 does not expose directly.
pub struct TerminalScreenModel {
    width: usize,
    height: usize,
    rows: Vec<Vec<char>>,
    scrollback: Vec<Vec<char>>,
    cursor_row: usize,
    cursor_col: usize,
    scroll_top: usize,
    scroll_bottom: usize,
}

impl TerminalScreenModel {
    pub fn new(width: u16, height: u16) -> Self {
        let w = width as usize;
        let h = height as usize;
        Self {
            width: w,
            height: h,
            rows: vec![vec![' '; w]; h],
            scrollback: Vec::new(),
            cursor_row: 0,
            cursor_col: 0,
            scroll_top: 0,
            scroll_bottom: h.saturating_sub(1),
        }
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            if data[i] == 0x1b {
                i += 1;
                if i >= data.len() {
                    break;
                }
                match data[i] {
                    b'[' => {
                        i += 1;
                        i = self.parse_csi(data, i);
                    }
                    b'M' => {
                        i += 1;
                        self.reverse_index();
                    }
                    _ => {
                        i += 1;
                    }
                }
            } else if data[i] == b'\r' {
                self.cursor_col = 0;
                i += 1;
            } else if data[i] == b'\n' {
                if self.cursor_row >= self.scroll_bottom {
                    self.scroll_up_one();
                } else {
                    self.cursor_row = (self.cursor_row + 1).min(self.height - 1);
                }
                i += 1;
            } else if data[i] >= 0x20 {
                let start = i;
                while i < data.len() && data[i] >= 0x20 && data[i] != 0x1b {
                    i += 1;
                }
                let text = String::from_utf8_lossy(&data[start..i]);
                for ch in text.chars() {
                    if self.cursor_col < self.width && self.cursor_row < self.height {
                        self.rows[self.cursor_row][self.cursor_col] = ch;
                        self.cursor_col += 1;
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    fn parse_csi(&mut self, data: &[u8], start: usize) -> usize {
        let mut i = start;
        let mut params = Vec::new();
        let mut current_param = String::new();
        while i < data.len() {
            let b = data[i];
            if b.is_ascii_digit() {
                current_param.push(b as char);
                i += 1;
            } else if b == b';' {
                params.push(current_param.parse::<usize>().unwrap_or(0));
                current_param.clear();
                i += 1;
            } else if b == b'?' {
                i += 1;
            } else {
                break;
            }
        }
        if !current_param.is_empty() {
            params.push(current_param.parse::<usize>().unwrap_or(0));
        }
        if i >= data.len() {
            return i;
        }
        let cmd = data[i];
        i += 1;
        match cmd {
            b'H' | b'f' => {
                let row = params.first().copied().unwrap_or(1).saturating_sub(1);
                let col = params.get(1).copied().unwrap_or(1).saturating_sub(1);
                self.cursor_row = row.min(self.height.saturating_sub(1));
                self.cursor_col = col.min(self.width.saturating_sub(1));
            }
            b'J' => {
                let mode = params.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        for c in self.cursor_col..self.width {
                            self.rows[self.cursor_row][c] = ' ';
                        }
                        for row in self.cursor_row + 1..self.height {
                            self.rows[row] = vec![' '; self.width];
                        }
                    }
                    1 => {
                        for row in 0..self.cursor_row {
                            self.rows[row] = vec![' '; self.width];
                        }
                        for c in 0..=self.cursor_col.min(self.width - 1) {
                            self.rows[self.cursor_row][c] = ' ';
                        }
                    }
                    2 => {
                        for row in &mut self.rows {
                            *row = vec![' '; self.width];
                        }
                    }
                    3 => {
                        self.scrollback.clear();
                        for row in &mut self.rows {
                            *row = vec![' '; self.width];
                        }
                    }
                    _ => {}
                }
            }
            b'K' => {
                let mode = params.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        for c in self.cursor_col..self.width {
                            self.rows[self.cursor_row][c] = ' ';
                        }
                    }
                    1 => {
                        for c in 0..=self.cursor_col.min(self.width - 1) {
                            self.rows[self.cursor_row][c] = ' ';
                        }
                    }
                    2 => {
                        self.rows[self.cursor_row] = vec![' '; self.width];
                    }
                    _ => {}
                }
            }
            b'r' => {
                if params.len() >= 2 {
                    self.scroll_top = params[0].saturating_sub(1).min(self.height - 1);
                    self.scroll_bottom = params[1].saturating_sub(1).min(self.height - 1);
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.height.saturating_sub(1);
                }
            }
            b'm' => {}
            b'h' | b'l' => {}
            b'A' => {
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            b'B' => {
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_row = (self.cursor_row + n).min(self.height - 1);
            }
            b'C' => {
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_col = (self.cursor_col + n).min(self.width - 1);
            }
            b'D' => {
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_col = self.cursor_col.saturating_sub(n);
            }
            _ => {}
        }
        i
    }

    fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down_one();
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    fn scroll_up_one(&mut self) {
        if self.scroll_top < self.scroll_bottom && self.scroll_bottom < self.height {
            let scrolled_off = self.rows.remove(self.scroll_top);
            if self.scroll_top == 0 {
                self.scrollback.push(scrolled_off);
            }
            self.rows.insert(self.scroll_bottom, vec![' '; self.width]);
        }
    }

    fn scroll_down_one(&mut self) {
        if self.scroll_top < self.scroll_bottom && self.scroll_bottom < self.height {
            self.rows.remove(self.scroll_bottom);
            self.rows.insert(self.scroll_top, vec![' '; self.width]);
        }
    }

    pub fn screen_lines(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| {
                let s: String = row.iter().collect();
                s.trim_end().to_string()
            })
            .collect()
    }

    pub fn scrollback_lines(&self) -> Vec<String> {
        self.scrollback
            .iter()
            .map(|row| {
                let s: String = row.iter().collect();
                s.trim_end().to_string()
            })
            .collect()
    }

    pub fn full_contents(&self) -> Vec<String> {
        let mut lines = self.scrollback_lines();
        lines.extend(self.screen_lines());
        lines
    }
}

pub struct TransitionResult {
    pub viewport_area: Rect,
    pub buffer_screen: Vec<String>,
    pub full_terminal_screen: Vec<String>,
}

pub fn run_prompt_transition(
    state: &mut TuiState,
    terminal: &mut Terminal<RenderFixtureBackend>,
    screen_model: &mut TerminalScreenModel,
) -> TransitionResult {
    prepare_draw_transaction(terminal, state, false).expect("prepare draw transaction");
    terminal
        .draw(|frame| state.draw(frame))
        .expect("draw frame");
    screen_model.process_bytes(terminal.backend_mut().output.as_slice());
    TransitionResult {
        viewport_area: terminal.viewport_area,
        buffer_screen: terminal.screen_lines(),
        full_terminal_screen: screen_model.full_contents(),
    }
}

pub fn input_chrome_count(screen: &[String]) -> usize {
    screen
        .iter()
        .filter(|line| line.starts_with("› ") || line.starts_with("> ") || line.starts_with("❯"))
        .count()
}

pub fn input_chrome_is_at_bottom(screen: &[String]) -> bool {
    let input_pos = screen.iter().rposition(|line| {
        line.starts_with("› ") || line.starts_with("> ") || line.starts_with("❯")
    });
    match input_pos {
        Some(inp) => {
            let after_input = &screen[inp + 1..];
            !after_input.iter().any(|line| {
                let t = line.trim();
                t.starts_with("●") || t.starts_with("∴") || t.starts_with("└")
            })
        }
        _ => false,
    }
}

pub fn is_chrome_line(line: &str) -> bool {
    let t = line.trim();
    let is_divider = !t.is_empty() && t.chars().all(|c| c == '─');
    let is_input = line.starts_with("› ") || line.starts_with("> ") || line.starts_with("❯");
    let is_bare_input = t == "›" || t == ">" || t == "❯";
    let is_footer = line.contains("-- NORMAL --") || line.contains("-- INSERT --");
    let is_spinner = is_spinner_status_line(t);
    is_divider || is_input || is_bare_input || is_footer || is_spinner
}

pub fn is_scrollback_chrome(line: &str) -> bool {
    let t = line.trim();
    let is_divider = !t.is_empty() && t.chars().all(|c| c == '─');
    let is_bare_input = t == "›" || t == ">" || t == "❯";
    let is_footer = line.contains("-- NORMAL --") || line.contains("-- INSERT --");
    let is_spinner = is_spinner_status_line(t);
    is_divider || is_bare_input || is_footer || is_spinner
}

fn is_spinner_status_line(line: &str) -> bool {
    let Some(first) = line.chars().next() else {
        return false;
    };
    matches!(first, '·' | '✢' | '✳' | '✶' | '✻' | '*') && line.contains("...(")
}

pub fn no_chrome_between_committed_cells(screen: &[String]) -> bool {
    let first_content = screen
        .iter()
        .position(|line| !line.trim().is_empty() && !is_chrome_line(line));
    let last_content = screen
        .iter()
        .rposition(|line| !line.trim().is_empty() && !is_chrome_line(line));
    let (Some(first), Some(last)) = (first_content, last_content) else {
        return true;
    };
    for line in &screen[first..=last] {
        if is_chrome_line(line) {
            return false;
        }
    }
    true
}
