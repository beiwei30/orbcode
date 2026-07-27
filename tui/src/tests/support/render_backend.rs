use super::*;

#[derive(Debug)]
pub struct RenderFixtureBackend {
    size: Size,
    cursor: Position,
    pub output: Vec<u8>,
    hidden_cursor: bool,
}

impl RenderFixtureBackend {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            size: Size { width, height },
            cursor: Position { x: 0, y: 0 },
            output: Vec::new(),
            hidden_cursor: false,
        }
    }

    pub fn output_string(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = Size { width, height };
    }
}

impl Write for RenderFixtureBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Backend for RenderFixtureBackend {
    fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.hidden_cursor = true;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.hidden_cursor = false;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let pos = position.into();
        self.cursor = pos;
        use std::fmt::Write as FmtWrite;
        let mut ansi = String::new();
        let _ = write!(ansi, "\x1b[{};{}H", pos.y + 1, pos.x + 1);
        self.output.extend_from_slice(ansi.as_bytes());
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let code = match clear_type {
            ClearType::All => "\x1b[2J",
            ClearType::AfterCursor => "\x1b[0J",
            ClearType::BeforeCursor => "\x1b[1J",
            ClearType::CurrentLine => "\x1b[2K",
            ClearType::UntilNewLine => "\x1b[0K",
        };
        self.output.extend_from_slice(code.as_bytes());
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size {
                width: self.size.width.saturating_mul(8),
                height: self.size.height.saturating_mul(16),
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct RenderMetricsFixture {
    terminal: Terminal<RenderFixtureBackend>,
}

impl RenderMetricsFixture {
    pub fn new(width: u16, height: u16) -> Self {
        let backend = RenderFixtureBackend::new(width, height);
        let mut terminal = Terminal::with_options(backend).expect("create render metrics terminal");
        terminal.set_viewport_area(Rect::new(0, 0, width, height));
        Self { terminal }
    }

    pub fn draw(&mut self, state: &mut TuiState) -> custom_terminal::TerminalDrawMetrics {
        self.terminal
            .draw_with_metrics(|frame| state.draw(frame))
            .expect("draw render metrics fixture")
    }

    pub fn draw_and_capture(&mut self, state: &mut TuiState) -> Vec<String> {
        self.terminal
            .draw(|frame| state.draw(frame))
            .expect("draw frame capture");
        self.terminal.screen_lines()
    }

    pub fn cursor_position(&mut self) -> Position {
        self.terminal.backend_mut().cursor
    }
}

pub fn draw_at_content_height(
    state: &mut TuiState,
    width: u16,
    terminal_height: u16,
) -> Vec<String> {
    let desired = state.desired_viewport_height(width, terminal_height);
    let height = desired.min(terminal_height).max(1);
    let mut fixture = RenderMetricsFixture::new(width, height);
    fixture.draw_and_capture(state)
}

pub fn max_blank_gap(screen: &[String]) -> usize {
    let mut max_gap = 0;
    let mut current_gap = 0;
    let mut found_content = false;
    for line in screen {
        if line.trim().is_empty() {
            if found_content {
                current_gap += 1;
            }
        } else {
            found_content = true;
            max_gap = max_gap.max(current_gap);
            current_gap = 0;
        }
    }
    max_gap
}

pub fn screen_has_input_chrome(screen: &[String]) -> bool {
    screen
        .iter()
        .any(|line| line.starts_with("› ") || line.starts_with("> ") || line.starts_with("❯"))
}

pub fn screen_has_divider(screen: &[String]) -> bool {
    screen.iter().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && trimmed.chars().all(|c| c == '─')
    })
}

pub fn assert_render_metrics_update_bounded(
    label: &str,
    mut state: TuiState,
    mutate: impl FnOnce(&mut TuiState),
) {
    let mut fixture = RenderMetricsFixture::new(120, 32);
    let first = fixture.draw(&mut state);
    mutate(&mut state);
    let second = fixture.draw(&mut state);

    assert!(
        first.initial_frame,
        "{label} initial draw should be full frame"
    );
    assert!(
        !second.initial_frame,
        "{label} update should be incremental"
    );
    assert!(second.output_bytes > 0, "{label} update should redraw");
    assert!(
        second.draw_command_count < second.buffer_cell_count,
        "{label} update should stay below full-frame command budget"
    );
}
