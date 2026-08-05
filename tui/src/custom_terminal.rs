use std::io;
use std::io::Write;
use std::time::Duration;
use std::time::Instant;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use crossterm::terminal::DisableLineWrap;
use crossterm::terminal::EnableLineWrap;
use ratatui::backend::Backend;
use ratatui::backend::ClearType as BackendClearType;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::numeric::saturating_u16;

#[derive(Debug)]
pub struct Frame<'a> {
    cursor_position: Option<Position>,
    viewport_area: Rect,
    buffer: &'a mut Buffer,
}

impl Frame<'_> {
    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct TerminalDrawMetrics {
    pub total_duration_us: u64,
    pub render_duration_us: u64,
    pub diff_duration_us: u64,
    pub diff_buffer_scan_duration_us: u64,
    pub diff_command_generation_duration_us: u64,
    pub terminal_write_duration_us: u64,
    pub backend_flush_duration_us: u64,
    pub draw_command_count: u64,
    pub terminal_cursor_move_count: u64,
    pub terminal_style_command_count: u64,
    pub terminal_print_command_count: u64,
    pub terminal_clear_command_count: u64,
    pub output_bytes: u64,
    pub buffer_cell_count: u64,
    pub initial_frame: bool,
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
struct TerminalOutputCommandStats {
    cursor_move_count: u64,
    style_command_count: u64,
    print_command_count: u64,
    clear_command_count: u64,
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
struct TerminalDiffMetrics {
    total_duration_us: u64,
    buffer_scan_duration_us: u64,
    command_generation_duration_us: u64,
}

impl TerminalDrawMetrics {
    fn set_output_command_stats(&mut self, stats: TerminalOutputCommandStats) {
        self.terminal_cursor_move_count = stats.cursor_move_count;
        self.terminal_style_command_count = stats.style_command_count;
        self.terminal_print_command_count = stats.print_command_count;
        self.terminal_clear_command_count = stats.clear_command_count;
    }
}

#[derive(Debug, Default, Clone, Eq, PartialEq, Hash)]
pub struct Terminal<B>
where
    B: Backend + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    hidden_cursor: bool,
    has_drawn_once: bool,
    pub viewport_area: Rect,
    last_known_screen_size: Size,
    pub last_known_cursor_pos: Position,
    visible_history_rows: u16,
}

impl<B> Terminal<B>
where
    B: Backend + Write,
{
    pub fn with_options(backend: B) -> io::Result<Self> {
        let screen_size = backend.size()?;
        Ok(Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            has_drawn_once: false,
            viewport_area: Rect::new(0, 0, screen_size.width, 0),
            last_known_screen_size: screen_size,
            last_known_cursor_pos: Position { x: 0, y: 0 },
            visible_history_rows: 0,
        })
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    /// The screen size observed at the last `autoresize` (i.e. the previous
    /// frame's size). At `prepare_draw_transaction` time this is the size before
    /// the current terminal event, so comparing it with `size()` detects a
    /// resize (width and/or height) that just occurred.
    pub fn last_known_screen_size(&self) -> Size {
        self.last_known_screen_size
    }

    pub fn set_viewport_area(&mut self, area: Rect) {
        self.current_buffer_mut().resize(area);
        self.previous_buffer_mut().resize(area);
        self.viewport_area = area;
        self.visible_history_rows = self.visible_history_rows.min(area.top());
    }

    pub fn invalidate_viewport(&mut self) {
        self.previous_buffer_mut().reset();
    }

    pub fn has_drawn_once(&self) -> bool {
        self.has_drawn_once
    }

    pub fn visible_history_rows(&self) -> u16 {
        self.visible_history_rows
    }

    pub fn note_history_rows_inserted(&mut self, inserted_rows: u16) {
        self.visible_history_rows = self
            .visible_history_rows
            .saturating_add(inserted_rows)
            .min(self.viewport_area.top());
    }

    pub fn reset_visible_history(&mut self) {
        self.visible_history_rows = 0;
    }

    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }

        self.clear_after_position(Position {
            x: self.viewport_area.x,
            y: self.viewport_area.y,
        })
    }

    pub fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(BackendClearType::AfterCursor)?;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    pub fn draw<F>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.draw_inner(render_callback, false).map(|_| ())
    }

    pub fn draw_with_metrics<F>(&mut self, render_callback: F) -> io::Result<TerminalDrawMetrics>
    where
        F: FnOnce(&mut Frame),
    {
        self.draw_inner(render_callback, true)
    }

    fn draw_inner<F>(
        &mut self,
        render_callback: F,
        collect_metrics: bool,
    ) -> io::Result<TerminalDrawMetrics>
    where
        F: FnOnce(&mut Frame),
    {
        let total_start = collect_metrics.then(Instant::now);
        self.autoresize()?;
        let render_start = collect_metrics.then(Instant::now);
        let cursor_position = {
            let mut frame = Frame {
                cursor_position: None,
                viewport_area: self.viewport_area,
                buffer: self.current_buffer_mut(),
            };
            render_callback(&mut frame);
            frame.cursor_position
        };
        let render_duration_us = render_start
            .map(|start| duration_us(start.elapsed()))
            .unwrap_or_default();

        let mut metrics = if collect_metrics {
            self.flush_with_metrics()?
        } else {
            self.flush()?;
            TerminalDrawMetrics::default()
        };
        metrics.render_duration_us = render_duration_us;
        match cursor_position {
            None => self.hide_cursor()?,
            Some(position) => {
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
        }
        self.swap_buffers();
        let backend_flush_start = collect_metrics.then(Instant::now);
        Backend::flush(&mut self.backend)?;
        if let Some(start) = backend_flush_start {
            metrics.backend_flush_duration_us = duration_us(start.elapsed());
        }
        if let Some(start) = total_start {
            metrics.total_duration_us = duration_us(start.elapsed());
        }
        Ok(metrics)
    }

    fn autoresize(&mut self) -> io::Result<()> {
        let screen_size = self.size()?;
        if screen_size != self.last_known_screen_size {
            self.last_known_screen_size = screen_size;
        }
        Ok(())
    }

    fn current_buffer(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.has_drawn_once {
            let current_buffer = self.current_buffer().clone();
            if let Some(position) = draw_initial_frame(&mut self.backend, &current_buffer)? {
                self.last_known_cursor_pos = position;
            }
            self.has_drawn_once = true;
            return Ok(());
        }

        let updates = diff_buffers(self.previous_buffer(), self.current_buffer());
        let last_put = updates
            .iter()
            .rfind(|command| matches!(command, DrawCommand::Put { .. }));
        if let Some(&DrawCommand::Put { x, y, .. }) = last_put {
            self.last_known_cursor_pos = Position { x, y };
        }
        draw_commands(&mut self.backend, updates.into_iter())
    }

    fn flush_with_metrics(&mut self) -> io::Result<TerminalDrawMetrics> {
        let mut metrics = TerminalDrawMetrics {
            buffer_cell_count: self.current_buffer().content.len() as u64,
            ..TerminalDrawMetrics::default()
        };

        if !self.has_drawn_once {
            let current_buffer = self.current_buffer().clone();
            metrics.initial_frame = true;
            metrics.draw_command_count = initial_frame_draw_command_count(&current_buffer) as u64;
            metrics.set_output_command_stats(initial_frame_output_command_stats(&current_buffer));
            let (position, output_bytes) = {
                let write_start = Instant::now();
                let mut writer = CountingWriter::new(&mut self.backend);
                let position = draw_initial_frame(&mut writer, &current_buffer)?;
                metrics.terminal_write_duration_us = duration_us(write_start.elapsed());
                (position, writer.bytes_written())
            };
            metrics.output_bytes = output_bytes;
            if let Some(position) = position {
                self.last_known_cursor_pos = position;
            }
            self.has_drawn_once = true;
            return Ok(metrics);
        }

        let (updates, diff_metrics) =
            diff_buffers_with_metrics(self.previous_buffer(), self.current_buffer());
        metrics.diff_duration_us = diff_metrics.total_duration_us;
        metrics.diff_buffer_scan_duration_us = diff_metrics.buffer_scan_duration_us;
        metrics.diff_command_generation_duration_us = diff_metrics.command_generation_duration_us;
        metrics.draw_command_count = updates.len() as u64;
        metrics.set_output_command_stats(draw_commands_output_stats(&updates));
        let last_put = updates
            .iter()
            .rfind(|command| matches!(command, DrawCommand::Put { .. }));
        if let Some(&DrawCommand::Put { x, y, .. }) = last_put {
            self.last_known_cursor_pos = Position { x, y };
        }
        let output_bytes = {
            let write_start = Instant::now();
            let mut writer = CountingWriter::new(&mut self.backend);
            draw_commands(&mut writer, updates.into_iter())?;
            metrics.terminal_write_duration_us = duration_us(write_start.elapsed());
            writer.bytes_written()
        };
        metrics.output_bytes = output_bytes;
        Ok(metrics)
    }

    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    fn swap_buffers(&mut self) {
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
    }

    #[cfg(test)]
    pub fn screen_lines(&self) -> Vec<String> {
        let buffer = self.previous_buffer();
        let area = buffer.area;
        if area.is_empty() {
            return Vec::new();
        }
        let width = area.width as usize;
        (0..area.height as usize)
            .map(|row| {
                let start = row * width;
                let end = start + width;
                let row_cells = &buffer.content[start..end];
                let mut text = String::with_capacity(width);
                let mut col = 0;
                while col < row_cells.len() {
                    let cell = &row_cells[col];
                    let sym = cell.symbol();
                    text.push_str(sym);
                    let w = UnicodeWidthStr::width(sym).max(1);
                    col += w;
                }
                text.trim_end().to_string()
            })
            .collect()
    }
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

struct CountingWriter<'a, W>
where
    W: Write,
{
    writer: &'a mut W,
    bytes_written: u64,
}

impl<'a, W> CountingWriter<'a, W>
where
    W: Write,
{
    fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            bytes_written: 0,
        }
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl<W> Write for CountingWriter<'_, W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(buf)?;
        self.bytes_written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[derive(Debug, Eq, PartialEq)]
enum DrawCommand {
    Put { x: u16, y: u16, cell: Cell },
    ClearToEnd { x: u16, y: u16, bg: Color },
}

struct DiffBufferScan {
    clear_to_end: Vec<ClearToEndCommand>,
    put_indices: Vec<usize>,
}

struct ClearToEndCommand {
    x: u16,
    y: u16,
    bg: Color,
}

fn diff_buffers(previous: &Buffer, next: &Buffer) -> Vec<DrawCommand> {
    let scan = scan_buffer_diff(previous, next);
    draw_commands_from_diff_scan(next, scan)
}

fn diff_buffers_with_metrics(
    previous: &Buffer,
    next: &Buffer,
) -> (Vec<DrawCommand>, TerminalDiffMetrics) {
    let total_start = Instant::now();
    let scan_start = Instant::now();
    let scan = scan_buffer_diff(previous, next);
    let buffer_scan_duration_us = duration_us(scan_start.elapsed());

    let command_generation_start = Instant::now();
    let commands = draw_commands_from_diff_scan(next, scan);
    let command_generation_duration_us = duration_us(command_generation_start.elapsed());

    (
        commands,
        TerminalDiffMetrics {
            total_duration_us: duration_us(total_start.elapsed()),
            buffer_scan_duration_us,
            command_generation_duration_us,
        },
    )
}

fn scan_buffer_diff(previous: &Buffer, next: &Buffer) -> DiffBufferScan {
    // The row loop below indexes `previous` with `next`'s dimensions. If the two
    // buffers differ in size (some path resized only one), that would slice out
    // of bounds and panic. Fall back to a full redraw of `next`.
    if previous.area != next.area || previous.content.len() != next.content.len() {
        return DiffBufferScan {
            clear_to_end: Vec::new(),
            put_indices: (0..next.content.len()).collect(),
        };
    }
    let previous_buffer = &previous.content;
    let next_buffer = &next.content;
    let width = next.area.width as usize;

    let mut clear_to_end = vec![];
    let mut put_indices = vec![];
    for y in 0..next.area.height {
        let row_start = y as usize * width;
        let row_end = row_start + width;
        let previous_row = &previous_buffer[row_start..row_end];
        let next_row = &next_buffer[row_start..row_end];
        if previous_row == next_row {
            continue;
        }

        let bg = next_row.last().map_or(Color::Reset, |cell| cell.bg);
        let last_nonblank_column = last_nonblank_column(next_row, bg);
        if last_nonblank_column + 1 < next_row.len()
            && row_tail_needs_clear(previous_row, last_nonblank_column + 1, bg)
        {
            let x = next
                .area
                .x
                .saturating_add(saturating_u16(last_nonblank_column))
                .saturating_add(1);
            let y = next.area.y + y;
            clear_to_end.push(ClearToEndCommand { x, y, bg });
        }

        let mut invalidated: usize = 0;
        let mut to_skip: usize = 0;
        for (column, (current, previous)) in next_row.iter().zip(previous_row.iter()).enumerate() {
            if !current.skip
                && (current != previous || invalidated > 0)
                && to_skip == 0
                && column <= last_nonblank_column
            {
                put_indices.push(row_start + column);
            }

            to_skip = display_width(current.symbol()).saturating_sub(1);
            let affected_width = std::cmp::max(
                display_width(current.symbol()),
                display_width(previous.symbol()),
            );
            invalidated = std::cmp::max(affected_width, invalidated).saturating_sub(1);
        }
    }

    DiffBufferScan {
        clear_to_end,
        put_indices,
    }
}

fn draw_commands_from_diff_scan(next: &Buffer, scan: DiffBufferScan) -> Vec<DrawCommand> {
    let mut updates = Vec::with_capacity(scan.clear_to_end.len() + scan.put_indices.len());
    for command in scan.clear_to_end {
        updates.push(DrawCommand::ClearToEnd {
            x: command.x,
            y: command.y,
            bg: command.bg,
        });
    }
    for index in scan.put_indices {
        let width = usize::from(next.area.width);
        if width == 0 {
            continue;
        }
        let x = next.area.x.saturating_add(saturating_u16(index % width));
        let y = next.area.y.saturating_add(saturating_u16(index / width));
        updates.push(DrawCommand::Put {
            x,
            y,
            cell: next.content[index].clone(),
        });
    }
    updates
}

fn last_nonblank_column(row: &[Cell], bg: Color) -> usize {
    let mut last_nonblank_column = 0usize;
    let mut column = 0usize;
    while column < row.len() {
        let cell = &row[column];
        let width = display_width(cell.symbol());
        if cell.symbol() != " " || cell.bg != bg || cell.modifier != Modifier::empty() {
            last_nonblank_column = column + width.saturating_sub(1);
        }
        column += width.max(1);
    }
    last_nonblank_column
}

fn row_tail_needs_clear(previous_row: &[Cell], start: usize, bg: Color) -> bool {
    previous_row.iter().skip(start).any(|cell| {
        cell.skip || cell.symbol() != " " || cell.bg != bg || cell.modifier != Modifier::empty()
    })
}

fn draw_initial_frame(writer: &mut impl Write, buffer: &Buffer) -> io::Result<Option<Position>> {
    if buffer.area.is_empty() {
        return Ok(None);
    }
    if crate::terminal_trace::enabled() {
        let mut output = Vec::new();
        let result = draw_initial_frame_direct(&mut output, buffer);
        crate::terminal_trace::record_bytes(
            "draw_initial_frame",
            serde_json::json!({
                "area": crate::terminal_trace::rect(buffer.area),
                "cell_count": buffer.content.len(),
            }),
            &output,
        );
        writer.write_all(&output)?;
        return result;
    }
    draw_initial_frame_direct(writer, buffer)
}

fn draw_initial_frame_direct(
    writer: &mut impl Write,
    buffer: &Buffer,
) -> io::Result<Option<Position>> {
    queue!(writer, DisableLineWrap)?;
    let result = draw_initial_frame_inner(writer, buffer);
    finish_line_wrap_disabled(writer, result)
}

fn draw_initial_frame_inner(
    writer: &mut impl Write,
    buffer: &Buffer,
) -> io::Result<Option<Position>> {
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_printed = None;

    queue!(writer, MoveTo(buffer.area.x, buffer.area.y))?;

    for row_index in 0..buffer.area.height as usize {
        if row_index > 0 {
            queue!(writer, Print("\r\n"))?;
        }

        let row_start = row_index * buffer.area.width as usize;
        let row_end = row_start + buffer.area.width as usize;
        let row = &buffer.content[row_start..row_end];
        let Some(last_visible_column) = last_visible_column(row) else {
            continue;
        };

        let mut column = 0usize;
        while column <= last_visible_column && column < row.len() {
            let cell = &row[column];
            let width = display_width(cell.symbol()).max(1);
            if cell.skip {
                column += 1;
                continue;
            }

            if cell.modifier != modifier {
                ModifierDiff {
                    from: modifier,
                    to: cell.modifier,
                }
                .queue(writer)?;
                modifier = cell.modifier;
            }
            if cell.fg != fg || cell.bg != bg {
                queue!(
                    writer,
                    SetColors(Colors::new(cell.fg.into(), cell.bg.into()))
                )?;
                fg = cell.fg;
                bg = cell.bg;
            }
            queue!(writer, Print(cell.symbol()))?;
            last_printed = Some(Position {
                x: buffer
                    .area
                    .x
                    .saturating_add(saturating_u16(column.saturating_add(width - 1))),
                y: buffer.area.y.saturating_add(saturating_u16(row_index)),
            });
            column += width;
        }
    }

    queue!(
        writer,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )?;

    Ok(last_printed)
}

fn last_visible_column(row: &[Cell]) -> Option<usize> {
    let bg = row.last().map_or(Color::Reset, |cell| cell.bg);
    let mut last_visible = None;
    let mut column = 0usize;
    while column < row.len() {
        let cell = &row[column];
        let width = display_width(cell.symbol()).max(1);
        if cell.symbol() != " " || cell.bg != bg || cell.modifier != Modifier::empty() {
            last_visible = Some(column + width.saturating_sub(1));
        }
        column += width;
    }
    last_visible
}

fn initial_frame_draw_command_count(buffer: &Buffer) -> usize {
    if buffer.area.is_empty() {
        return 0;
    }

    let mut count = 3;
    for row_index in 0..buffer.area.height as usize {
        if row_index > 0 {
            count += 1;
        }

        let row_start = row_index * buffer.area.width as usize;
        let row_end = row_start + buffer.area.width as usize;
        let row = &buffer.content[row_start..row_end];
        let Some(last_visible_column) = last_visible_column(row) else {
            continue;
        };
        let mut column = 0usize;
        while column <= last_visible_column && column < row.len() {
            let cell = &row[column];
            let width = display_width(cell.symbol()).max(1);
            if !cell.skip {
                count += 1;
            }
            column += width;
        }
    }

    count + 1
}

fn initial_frame_output_command_stats(buffer: &Buffer) -> TerminalOutputCommandStats {
    if buffer.area.is_empty() {
        return TerminalOutputCommandStats::default();
    }

    let mut stats = TerminalOutputCommandStats {
        cursor_move_count: 1,
        ..TerminalOutputCommandStats::default()
    };
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();

    for row_index in 0..buffer.area.height as usize {
        if row_index > 0 {
            stats.print_command_count += 1;
        }

        let row_start = row_index * buffer.area.width as usize;
        let row_end = row_start + buffer.area.width as usize;
        let row = &buffer.content[row_start..row_end];
        let Some(last_visible_column) = last_visible_column(row) else {
            continue;
        };

        let mut column = 0usize;
        while column <= last_visible_column && column < row.len() {
            let cell = &row[column];
            let width = display_width(cell.symbol()).max(1);
            if cell.skip {
                column += 1;
                continue;
            }

            if cell.modifier != modifier {
                stats.style_command_count += modifier_diff_command_count(modifier, cell.modifier);
                modifier = cell.modifier;
            }
            if cell.fg != fg || cell.bg != bg {
                stats.style_command_count += 1;
                fg = cell.fg;
                bg = cell.bg;
            }
            stats.print_command_count += 1;
            column += width;
        }
    }

    stats.style_command_count += 3;
    stats
}

fn draw_commands_output_stats(commands: &[DrawCommand]) -> TerminalOutputCommandStats {
    let mut stats = TerminalOutputCommandStats::default();
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_pos: Option<Position> = None;

    for command in commands {
        let (x, y) = match command {
            DrawCommand::Put { x, y, .. } => (*x, *y),
            DrawCommand::ClearToEnd { x, y, .. } => (*x, *y),
        };
        if !matches!(last_pos, Some(position) if x == position.x + 1 && y == position.y) {
            stats.cursor_move_count += 1;
        }
        match command {
            DrawCommand::Put { cell, .. } => {
                if cell.modifier != modifier {
                    stats.style_command_count +=
                        modifier_diff_command_count(modifier, cell.modifier);
                    modifier = cell.modifier;
                }
                if cell.fg != fg || cell.bg != bg {
                    stats.style_command_count += 1;
                    fg = cell.fg;
                    bg = cell.bg;
                }
                stats.print_command_count += 1;
                let width = display_width(cell.symbol()).max(1) as u16;
                last_pos = Some(Position {
                    x: x.saturating_add(width).saturating_sub(1),
                    y,
                });
            }
            DrawCommand::ClearToEnd { bg: clear_bg, .. } => {
                stats.style_command_count += 2;
                modifier = Modifier::empty();
                bg = *clear_bg;
                stats.clear_command_count += 1;
                last_pos = Some(Position { x, y });
            }
        }
    }

    stats.style_command_count += 3;
    stats
}

fn modifier_diff_command_count(from: Modifier, to: Modifier) -> u64 {
    let removed = from - to;
    let added = to - from;
    let mut count = 0;

    if removed.contains(Modifier::REVERSED) {
        count += 1;
    }
    if removed.contains(Modifier::BOLD) {
        count += 1;
        if to.contains(Modifier::DIM) {
            count += 1;
        }
    }
    if removed.contains(Modifier::ITALIC) {
        count += 1;
    }
    if removed.contains(Modifier::UNDERLINED) {
        count += 1;
    }
    if removed.contains(Modifier::DIM) {
        count += 1;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        count += 1;
    }
    if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
        count += 1;
    }

    if added.contains(Modifier::REVERSED) {
        count += 1;
    }
    if added.contains(Modifier::BOLD) {
        count += 1;
    }
    if added.contains(Modifier::ITALIC) {
        count += 1;
    }
    if added.contains(Modifier::UNDERLINED) {
        count += 1;
    }
    if added.contains(Modifier::DIM) {
        count += 1;
    }
    if added.contains(Modifier::CROSSED_OUT) {
        count += 1;
    }
    if added.contains(Modifier::SLOW_BLINK) {
        count += 1;
    }
    if added.contains(Modifier::RAPID_BLINK) {
        count += 1;
    }

    count
}

fn draw_commands<I>(writer: &mut impl Write, commands: I) -> io::Result<()>
where
    I: Iterator<Item = DrawCommand>,
{
    if crate::terminal_trace::enabled() {
        let commands = commands.collect::<Vec<_>>();
        let command_count = commands.len();
        let mut output = Vec::new();
        let result = draw_commands_direct(&mut output, commands.into_iter());
        crate::terminal_trace::record_bytes(
            "draw_commands",
            serde_json::json!({
                "command_count": command_count,
            }),
            &output,
        );
        writer.write_all(&output)?;
        return result;
    }
    draw_commands_direct(writer, commands)
}

fn draw_commands_direct<I>(writer: &mut impl Write, commands: I) -> io::Result<()>
where
    I: Iterator<Item = DrawCommand>,
{
    queue!(writer, DisableLineWrap)?;
    let result = draw_commands_inner(writer, commands);
    finish_line_wrap_disabled(writer, result)
}

fn draw_commands_inner<I>(writer: &mut impl Write, commands: I) -> io::Result<()>
where
    I: Iterator<Item = DrawCommand>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_pos: Option<Position> = None;

    for command in commands {
        let (x, y) = match command {
            DrawCommand::Put { x, y, .. } => (x, y),
            DrawCommand::ClearToEnd { x, y, .. } => (x, y),
        };
        if !matches!(last_pos, Some(position) if x == position.x + 1 && y == position.y) {
            queue!(writer, MoveTo(x, y))?;
        }
        match command {
            DrawCommand::Put { cell, .. } => {
                if cell.modifier != modifier {
                    ModifierDiff {
                        from: modifier,
                        to: cell.modifier,
                    }
                    .queue(writer)?;
                    modifier = cell.modifier;
                }
                if cell.fg != fg || cell.bg != bg {
                    queue!(
                        writer,
                        SetColors(Colors::new(cell.fg.into(), cell.bg.into()))
                    )?;
                    fg = cell.fg;
                    bg = cell.bg;
                }
                queue!(writer, Print(cell.symbol()))?;
                let width = display_width(cell.symbol()).max(1) as u16;
                last_pos = Some(Position {
                    x: x.saturating_add(width).saturating_sub(1),
                    y,
                });
            }
            DrawCommand::ClearToEnd { bg: clear_bg, .. } => {
                queue!(writer, SetAttribute(crossterm::style::Attribute::Reset))?;
                modifier = Modifier::empty();
                queue!(writer, SetBackgroundColor(clear_bg.into()))?;
                bg = clear_bg;
                queue!(writer, Clear(ClearType::UntilNewLine))?;
                last_pos = Some(Position { x, y });
            }
        }
    }

    queue!(
        writer,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )?;

    Ok(())
}

fn finish_line_wrap_disabled<T>(writer: &mut impl Write, result: io::Result<T>) -> io::Result<T> {
    let restore_result = queue!(writer, EnableLineWrap);
    match (result, restore_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn display_width(symbol: &str) -> usize {
    if !symbol.contains('\x1B') {
        return symbol.width();
    }

    let mut visible = String::with_capacity(symbol.len());
    let mut chars = symbol.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1B' && chars.clone().next() == Some(']') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\x07' {
                    break;
                }
            }
            continue;
        }
        visible.push(ch);
    }
    visible.width()
}

struct ModifierDiff {
    from: Modifier,
    to: Modifier,
}

impl ModifierDiff {
    fn queue(self, writer: &mut impl Write) -> io::Result<()> {
        use crossterm::style::Attribute as CrosstermAttribute;

        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(writer, SetAttribute(CrosstermAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(writer, SetAttribute(CrosstermAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(CrosstermAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(writer, SetAttribute(CrosstermAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(writer, SetAttribute(CrosstermAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(writer, SetAttribute(CrosstermAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(writer, SetAttribute(CrosstermAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(writer, SetAttribute(CrosstermAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(CrosstermAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(writer, SetAttribute(CrosstermAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::RapidBlink))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;
    use ratatui::widgets::Paragraph;

    #[test]
    fn initial_frame_draw_avoids_per_line_cursor_moves_and_line_clears() {
        let area = Rect::new(0, 0, 12, 3);
        let mut buffer = Buffer::empty(area);
        Paragraph::new("alpha\nbeta").render(area, &mut buffer);

        let mut output = Vec::new();
        let last = draw_initial_frame(&mut output, &buffer).expect("initial frame should render");
        let output = String::from_utf8(output).expect("ANSI output should be valid UTF-8");

        assert_eq!(last, Some(Position { x: 3, y: 1 }));
        assert!(output.starts_with("\x1b[?7l"));
        assert!(output.ends_with("\x1b[?7h"));
        assert!(!output.contains("\u{1b}[K"));
        assert_eq!(output.bytes().filter(|byte| *byte == b'H').count(), 1);
    }

    #[test]
    fn initial_frame_draw_steps_over_wide_char_continuation_cells() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buffer = Buffer::empty(area);
        Paragraph::new("可以看").render(area, &mut buffer);

        let mut output = Vec::new();
        draw_initial_frame(&mut output, &buffer).expect("initial frame should render");
        let output = String::from_utf8(output).expect("ANSI output should be valid UTF-8");

        assert!(output.contains("可以看"), "{output:?}");
        assert!(!output.contains("可 以 看"), "{output:?}");
        assert_eq!(
            initial_frame_output_command_stats(&buffer).print_command_count,
            3
        );
    }

    #[test]
    fn initial_frame_cursor_tracks_trailing_cell_of_wide_glyph() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buffer = Buffer::empty(area);
        Paragraph::new("A可").render(area, &mut buffer);

        let mut output = Vec::new();
        let last = draw_initial_frame(&mut output, &buffer).expect("initial frame should render");

        assert_eq!(last, Some(Position { x: 2, y: 0 }));
    }

    #[test]
    fn counting_writer_measures_initial_frame_output_bytes() {
        let area = Rect::new(0, 0, 12, 2);
        let mut buffer = Buffer::empty(area);
        Paragraph::new("alpha").render(area, &mut buffer);

        let mut output = Vec::new();
        let bytes_written = {
            let mut writer = CountingWriter::new(&mut output);
            draw_initial_frame(&mut writer, &buffer).expect("initial frame should render");
            writer.bytes_written()
        };

        assert_eq!(bytes_written as usize, output.len());
        assert!(bytes_written > 0);
    }

    #[test]
    fn output_command_stats_split_initial_and_incremental_commands() {
        let area = Rect::new(0, 0, 12, 2);
        let mut initial = Buffer::empty(area);
        set_test_cell(
            &mut initial,
            0,
            0,
            "A",
            Color::Red,
            Color::Reset,
            Modifier::BOLD,
        );
        set_test_cell(
            &mut initial,
            1,
            0,
            "B",
            Color::Blue,
            Color::Reset,
            Modifier::ITALIC,
        );

        let initial_stats = initial_frame_output_command_stats(&initial);
        assert_eq!(initial_stats.cursor_move_count, 1);
        assert!(initial_stats.style_command_count > 3);
        assert!(initial_stats.print_command_count >= 3);
        assert_eq!(initial_stats.clear_command_count, 0);

        let mut next = Buffer::empty(area);
        set_test_cell(
            &mut next,
            0,
            0,
            "C",
            Color::Green,
            Color::Reset,
            Modifier::BOLD,
        );
        let updates = diff_buffers(&initial, &next);
        let update_stats = draw_commands_output_stats(&updates);

        assert!(update_stats.cursor_move_count > 0);
        assert!(update_stats.style_command_count > 0);
        assert!(update_stats.print_command_count > 0);
        assert!(update_stats.clear_command_count > 0);
    }

    #[test]
    fn initial_frame_metrics_match_full_buffer_shape() {
        let area = Rect::new(0, 0, 12, 3);
        let mut buffer = Buffer::empty(area);
        fill_test_grid(&mut buffer, 0);

        let command_count = initial_frame_draw_command_count(&buffer);
        let stats = initial_frame_output_command_stats(&buffer);

        assert_eq!(
            command_count,
            buffer_cell_count(area) + area.height as usize + 3
        );
        assert_eq!(stats.cursor_move_count, 1);
        assert_eq!(
            stats.print_command_count,
            (buffer_cell_count(area) + area.height as usize - 1) as u64
        );
        assert!(stats.style_command_count > 3);
        assert_eq!(stats.clear_command_count, 0);
    }

    #[test]
    fn incremental_draw_disables_line_wrap_around_updates() {
        let area = Rect::new(0, 0, 4, 1);
        let previous = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        next.set_string(0, 0, "----", Style::default());
        let updates = diff_buffers(&previous, &next);

        let mut output = Vec::new();
        draw_commands(&mut output, updates.into_iter()).expect("draw commands");
        let output = String::from_utf8(output).expect("ANSI output should be valid UTF-8");

        assert!(output.starts_with("\x1b[?7l"), "{output:?}");
        assert!(output.ends_with("\x1b[?7h"), "{output:?}");
    }

    #[test]
    fn incremental_draw_treats_next_cell_after_wide_char_as_adjacent() {
        let commands = vec![
            DrawCommand::Put {
                x: 0,
                y: 0,
                cell: Cell::new("可"),
            },
            DrawCommand::Put {
                x: 2,
                y: 0,
                cell: Cell::new("A"),
            },
        ];
        let stats = draw_commands_output_stats(&commands);

        let mut output = Vec::new();
        draw_commands(&mut output, commands.into_iter()).expect("draw commands");
        let output = String::from_utf8(output).expect("ANSI output should be valid UTF-8");

        assert!(!output.contains("\x1b[1;3H"), "{output:?}");
        assert_eq!(stats.cursor_move_count, 1);
    }

    #[test]
    fn diff_buffer_metrics_split_scan_and_command_generation() {
        let area = Rect::new(0, 0, 12, 2);
        let mut previous = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        set_test_cell(
            &mut previous,
            0,
            0,
            "A",
            Color::Red,
            Color::Reset,
            Modifier::BOLD,
        );
        set_test_cell(
            &mut next,
            0,
            0,
            "B",
            Color::Green,
            Color::Reset,
            Modifier::ITALIC,
        );

        let expected = diff_buffers(&previous, &next);
        let (actual, metrics) = diff_buffers_with_metrics(&previous, &next);

        assert_eq!(actual, expected);
        assert!(!actual.is_empty());
        assert!(metrics.total_duration_us >= metrics.buffer_scan_duration_us);
        assert!(metrics.total_duration_us >= metrics.command_generation_duration_us);
    }

    #[test]
    fn diff_buffers_skip_unchanged_rows_and_only_clear_dirty_tails() {
        let area = Rect::new(0, 0, 8, 3);
        let mut previous = Buffer::empty(area);
        set_test_cell(
            &mut previous,
            0,
            0,
            "A",
            Color::Red,
            Color::Reset,
            Modifier::BOLD,
        );
        set_test_cell(
            &mut previous,
            0,
            1,
            "B",
            Color::Blue,
            Color::Reset,
            Modifier::ITALIC,
        );
        let mut next = previous.clone();

        assert!(diff_buffers(&previous, &next).is_empty());

        set_test_cell(
            &mut next,
            0,
            0,
            "C",
            Color::Green,
            Color::Reset,
            Modifier::BOLD,
        );
        let updates = diff_buffers(&previous, &next);
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            updates.first(),
            Some(DrawCommand::Put { x: 0, y: 0, .. })
        ));

        let cleared = Buffer::empty(area);
        set_test_cell(
            &mut previous,
            5,
            0,
            "D",
            Color::Yellow,
            Color::Reset,
            Modifier::empty(),
        );
        let updates = diff_buffers(&previous, &cleared);
        assert!(
            updates
                .iter()
                .any(|command| matches!(command, DrawCommand::ClearToEnd { x: 1, y: 0, .. }))
        );
    }

    #[test]
    fn diff_buffers_overlay_open_close_only_touches_overlay_cells() {
        let area = Rect::new(0, 0, 16, 6);
        let overlay = Rect::new(4, 2, 6, 3);
        let mut base = Buffer::empty(area);
        fill_test_grid(&mut base, 0);
        let mut opened = base.clone();
        apply_test_overlay(&mut opened, overlay);

        let open_updates = diff_buffers(&base, &opened);
        let close_updates = diff_buffers(&opened, &base);
        let overlay_cell_count = overlay.width as usize * overlay.height as usize;

        assert_eq!(open_updates.len(), overlay_cell_count);
        assert_eq!(close_updates.len(), overlay_cell_count);
        assert!(open_updates.iter().all(|command| matches!(
            command,
            DrawCommand::Put { x, y, .. }
                if *x >= overlay.x
                    && *x < overlay.x + overlay.width
                    && *y >= overlay.y
                    && *y < overlay.y + overlay.height
        )));
        assert!(close_updates.iter().all(|command| matches!(
            command,
            DrawCommand::Put { x, y, .. }
                if *x >= overlay.x
                    && *x < overlay.x + overlay.width
                    && *y >= overlay.y
                    && *y < overlay.y + overlay.height
        )));
    }

    #[test]
    fn diff_buffers_after_resize_stays_within_next_viewport_cells() {
        let small_area = Rect::new(0, 0, 16, 6);
        let large_area = Rect::new(0, 0, 24, 10);
        let mut small = Buffer::empty(small_area);
        let mut large = Buffer::empty(large_area);
        fill_test_grid(&mut small, 0);
        fill_test_grid(&mut large, 0);

        let previous_grown = resized_test_buffer(&small, large_area);
        let grow_updates = diff_buffers(&previous_grown, &large);
        assert!(!grow_updates.is_empty());
        assert!(grow_updates.len() <= buffer_cell_count(large_area));
        assert!(
            grow_updates
                .iter()
                .all(|command| matches!(command, DrawCommand::Put { .. }))
        );

        let previous_shrunk = resized_test_buffer(&large, small_area);
        let shrink_updates = diff_buffers(&previous_shrunk, &small);
        assert!(shrink_updates.len() <= buffer_cell_count(small_area));
        assert!(
            shrink_updates
                .iter()
                .all(|command| matches!(command, DrawCommand::Put { .. }))
        );
    }

    #[test]
    fn regression_budget_terminal_diff_updates_only_changed_regions() {
        let area = Rect::new(0, 0, 24, 8);
        let mut base = Buffer::empty(area);
        fill_test_grid(&mut base, 0);

        assert!(diff_buffers(&base, &base.clone()).is_empty());

        let mut one_cell = base.clone();
        set_test_cell(
            &mut one_cell,
            3,
            2,
            "#",
            Color::Yellow,
            Color::Reset,
            Modifier::REVERSED,
        );
        let one_cell_updates = diff_buffers(&base, &one_cell);
        let one_cell_stats = draw_commands_output_stats(&one_cell_updates);
        assert_eq!(one_cell_updates.len(), 1);
        assert_eq!(one_cell_stats.print_command_count, 1);

        const HIGHLIGHT_LEN: usize = 6;
        let mut old_selection = base.clone();
        let mut new_selection = base.clone();
        apply_test_highlight(&mut old_selection, 2, 1, HIGHLIGHT_LEN);
        apply_test_highlight(&mut new_selection, 10, 4, HIGHLIGHT_LEN);
        let selection_updates = diff_buffers(&old_selection, &new_selection);
        let selection_stats = draw_commands_output_stats(&selection_updates);
        assert_eq!(selection_updates.len(), HIGHLIGHT_LEN * 2);
        assert_eq!(
            selection_stats.print_command_count,
            selection_updates.len() as u64
        );

        let overlay = Rect::new(5, 2, 8, 3);
        let mut opened = base.clone();
        apply_test_overlay(&mut opened, overlay);
        let open_updates = diff_buffers(&base, &opened);
        let close_updates = diff_buffers(&opened, &base);
        let open_stats = draw_commands_output_stats(&open_updates);
        let overlay_cell_count = overlay.width as usize * overlay.height as usize;

        assert_eq!(open_updates.len(), overlay_cell_count);
        assert_eq!(close_updates.len(), overlay_cell_count);
        assert_eq!(open_stats.print_command_count, overlay_cell_count as u64);
        assert!(open_stats.cursor_move_count <= overlay.height as u64);
    }

    #[test]
    #[ignore = "manual stress test for terminal output command metrics"]
    fn terminal_output_command_metrics_stress_counts_style_heavy_updates() {
        const FRAME_COUNT: usize = 1_000;
        let area = Rect::new(0, 0, 120, 40);
        let mut previous = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        fill_test_grid(&mut previous, 0);
        fill_test_grid(&mut next, 1);

        let started = Instant::now();
        let mut last_update_count = 0;
        let mut last_stats = TerminalOutputCommandStats::default();
        for frame in 0..FRAME_COUNT {
            fill_test_grid(&mut previous, frame);
            fill_test_grid(&mut next, frame + 1);
            let updates = diff_buffers(&previous, &next);
            last_stats = draw_commands_output_stats(&updates);
            last_update_count = updates.len();
        }
        let duration = started.elapsed();

        assert!(last_update_count > 0);
        assert!(last_stats.cursor_move_count > 0);
        assert!(last_stats.style_command_count > 0);
        assert!(last_stats.print_command_count > 0);
        eprintln!(
            "frames={FRAME_COUNT} cells={} logical_updates={last_update_count} cursor_moves={} style_commands={} print_commands={} clear_commands={} loop_us={}",
            area.width as usize * area.height as usize,
            last_stats.cursor_move_count,
            last_stats.style_command_count,
            last_stats.print_command_count,
            last_stats.clear_command_count,
            duration.as_micros()
        );
    }

    #[test]
    #[ignore = "manual stress test for terminal initial draw metrics"]
    fn terminal_initial_draw_metrics_stress_records_full_frame_baseline() {
        const FRAME_COUNT: usize = 1_000;
        let area = Rect::new(0, 0, 120, 40);
        let mut buffer = Buffer::empty(area);

        let started = Instant::now();
        let mut last_command_count = 0;
        let mut last_stats = TerminalOutputCommandStats::default();
        let mut last_bytes = 0;
        let mut last_position = None;
        for frame in 0..FRAME_COUNT {
            fill_test_grid(&mut buffer, frame);
            last_command_count = initial_frame_draw_command_count(&buffer);
            last_stats = initial_frame_output_command_stats(&buffer);
            let mut output = Vec::new();
            let mut writer = CountingWriter::new(&mut output);
            last_position =
                draw_initial_frame(&mut writer, &buffer).expect("initial frame should render");
            last_bytes = writer.bytes_written();
        }
        let duration = started.elapsed();

        assert_eq!(
            last_command_count,
            buffer_cell_count(area) + area.height as usize + 1
        );
        assert_eq!(last_stats.cursor_move_count, 1);
        assert_eq!(
            last_stats.print_command_count,
            (buffer_cell_count(area) + area.height as usize - 1) as u64
        );
        assert!(last_stats.style_command_count > 3);
        assert_eq!(last_stats.clear_command_count, 0);
        assert!(last_bytes > buffer_cell_count(area) as u64);
        assert_eq!(
            last_position,
            Some(Position {
                x: area.width - 1,
                y: area.height - 1,
            })
        );
        eprintln!(
            "frames={FRAME_COUNT} cells={} command_count={last_command_count} cursor_moves={} style_commands={} print_commands={} clear_commands={} bytes={last_bytes} loop_us={}",
            buffer_cell_count(area),
            last_stats.cursor_move_count,
            last_stats.style_command_count,
            last_stats.print_command_count,
            last_stats.clear_command_count,
            duration.as_micros()
        );
    }

    #[test]
    #[ignore = "manual stress test for unchanged-row terminal diff skipping"]
    fn terminal_diff_unchanged_row_skip_stress_reduces_small_update_work() {
        const FRAME_COUNT: usize = 1_000;
        let area = Rect::new(0, 0, 120, 40);
        let mut previous = Buffer::empty(area);
        let mut full_next = Buffer::empty(area);
        fill_test_grid(&mut previous, 0);
        fill_test_grid(&mut full_next, 1);

        let mut total_full_scan_us = 0;
        let mut total_small_scan_us = 0;
        let mut last_full_update_count = 0;
        let mut last_small_update_count = 0;
        for frame in 0..FRAME_COUNT {
            let (full_updates, full_metrics) = diff_buffers_with_metrics(&previous, &full_next);

            let mut small_next = previous.clone();
            set_test_cell(
                &mut small_next,
                frame % area.width as usize,
                frame % area.height as usize,
                "#",
                Color::Yellow,
                Color::Reset,
                Modifier::REVERSED,
            );
            let (small_updates, small_metrics) = diff_buffers_with_metrics(&previous, &small_next);

            total_full_scan_us += full_metrics.buffer_scan_duration_us;
            total_small_scan_us += small_metrics.buffer_scan_duration_us;
            last_full_update_count = full_updates.len();
            last_small_update_count = small_updates.len();
        }

        assert!(last_full_update_count > area.width as usize);
        assert!(last_small_update_count < area.width as usize);
        assert!(total_full_scan_us > 0);
        assert!(total_small_scan_us > 0);
        assert!(total_small_scan_us < total_full_scan_us);
        eprintln!(
            "frames={FRAME_COUNT} cells={} full_updates={last_full_update_count} small_updates={last_small_update_count} full_scan_us={total_full_scan_us} small_scan_us={total_small_scan_us}",
            area.width as usize * area.height as usize,
        );
    }

    #[test]
    #[ignore = "manual stress test for terminal diff overlay open and close"]
    fn terminal_diff_overlay_open_close_stress_keeps_updates_bounded() {
        const FRAME_COUNT: usize = 1_000;
        let area = Rect::new(0, 0, 120, 40);
        let overlay = Rect::new(30, 10, 50, 14);
        let overlay_cell_count = overlay.width as usize * overlay.height as usize;
        let mut base = Buffer::empty(area);
        let mut full_next = Buffer::empty(area);
        fill_test_grid(&mut base, 0);
        fill_test_grid(&mut full_next, 1);

        let mut total_full_scan_us = 0;
        let mut total_overlay_scan_us = 0;
        let mut total_overlay_generation_us = 0;
        let mut last_full_update_count = 0;
        let mut last_open_update_count = 0;
        let mut last_close_update_count = 0;
        let mut last_open_stats = TerminalOutputCommandStats::default();
        for _ in 0..FRAME_COUNT {
            let (full_updates, full_metrics) = diff_buffers_with_metrics(&base, &full_next);
            let mut opened = base.clone();
            apply_test_overlay(&mut opened, overlay);

            let (open_updates, open_metrics) = diff_buffers_with_metrics(&base, &opened);
            let (close_updates, close_metrics) = diff_buffers_with_metrics(&opened, &base);
            last_open_stats = draw_commands_output_stats(&open_updates);

            total_full_scan_us += full_metrics.buffer_scan_duration_us * 2;
            total_overlay_scan_us +=
                open_metrics.buffer_scan_duration_us + close_metrics.buffer_scan_duration_us;
            total_overlay_generation_us += open_metrics.command_generation_duration_us
                + close_metrics.command_generation_duration_us;
            last_full_update_count = full_updates.len();
            last_open_update_count = open_updates.len();
            last_close_update_count = close_updates.len();
        }

        assert!(last_full_update_count > overlay_cell_count);
        assert_eq!(last_open_update_count, overlay_cell_count);
        assert_eq!(last_close_update_count, overlay_cell_count);
        assert!(last_open_stats.cursor_move_count <= overlay.height as u64);
        assert_eq!(
            last_open_stats.print_command_count,
            overlay_cell_count as u64
        );
        assert!(total_overlay_scan_us > 0);
        assert!(total_overlay_generation_us > 0);
        assert!(total_overlay_scan_us < total_full_scan_us);
        eprintln!(
            "frames={FRAME_COUNT} cells={} overlay_cells={overlay_cell_count} full_updates={last_full_update_count} open_updates={last_open_update_count} close_updates={last_close_update_count} open_cursor_moves={} open_style_commands={} open_print_commands={} full_scan_us={total_full_scan_us} overlay_scan_us={total_overlay_scan_us} overlay_generation_us={total_overlay_generation_us}",
            area.width as usize * area.height as usize,
            last_open_stats.cursor_move_count,
            last_open_stats.style_command_count,
            last_open_stats.print_command_count,
        );
    }

    #[test]
    #[ignore = "manual stress test for terminal diff resize"]
    fn terminal_diff_resize_stress_keeps_updates_inside_next_viewport() {
        const FRAME_COUNT: usize = 1_000;
        let small_area = Rect::new(0, 0, 80, 24);
        let large_area = Rect::new(0, 0, 120, 40);
        let mut small = Buffer::empty(small_area);
        let mut large = Buffer::empty(large_area);
        fill_test_grid(&mut small, 0);
        fill_test_grid(&mut large, 0);

        let mut total_grow_scan_us = 0;
        let mut total_shrink_scan_us = 0;
        let mut total_grow_generation_us = 0;
        let mut total_shrink_generation_us = 0;
        let mut last_grow_update_count = 0;
        let mut last_shrink_update_count = 0;
        let mut last_grow_stats = TerminalOutputCommandStats::default();
        for _ in 0..FRAME_COUNT {
            let previous_grown = resized_test_buffer(&small, large_area);
            let (grow_updates, grow_metrics) = diff_buffers_with_metrics(&previous_grown, &large);
            last_grow_stats = draw_commands_output_stats(&grow_updates);

            let previous_shrunk = resized_test_buffer(&large, small_area);
            let (shrink_updates, shrink_metrics) =
                diff_buffers_with_metrics(&previous_shrunk, &small);

            total_grow_scan_us += grow_metrics.buffer_scan_duration_us;
            total_shrink_scan_us += shrink_metrics.buffer_scan_duration_us;
            total_grow_generation_us += grow_metrics.command_generation_duration_us;
            total_shrink_generation_us += shrink_metrics.command_generation_duration_us;
            last_grow_update_count = grow_updates.len();
            last_shrink_update_count = shrink_updates.len();
        }

        assert!(last_grow_update_count > 0);
        assert!(last_grow_update_count <= buffer_cell_count(large_area));
        assert!(last_shrink_update_count <= buffer_cell_count(small_area));
        assert_eq!(
            last_grow_stats.print_command_count,
            last_grow_update_count as u64
        );
        assert!(total_grow_scan_us > 0);
        assert!(total_shrink_scan_us > 0);
        assert!(total_grow_generation_us > 0);
        eprintln!(
            "frames={FRAME_COUNT} small_cells={} large_cells={} grow_updates={last_grow_update_count} shrink_updates={last_shrink_update_count} grow_cursor_moves={} grow_style_commands={} grow_print_commands={} grow_scan_us={total_grow_scan_us} shrink_scan_us={total_shrink_scan_us} grow_generation_us={total_grow_generation_us} shrink_generation_us={total_shrink_generation_us}",
            buffer_cell_count(small_area),
            buffer_cell_count(large_area),
            last_grow_stats.cursor_move_count,
            last_grow_stats.style_command_count,
            last_grow_stats.print_command_count,
        );
    }

    #[test]
    #[ignore = "manual stress test for terminal diff selection highlight changes"]
    fn terminal_diff_selection_highlight_stress_keeps_updates_bounded() {
        const FRAME_COUNT: usize = 1_000;
        const HIGHLIGHT_LEN: usize = 24;
        let area = Rect::new(0, 0, 120, 40);
        let mut base = Buffer::empty(area);
        let mut full_next = Buffer::empty(area);
        fill_test_grid(&mut base, 0);
        fill_test_grid(&mut full_next, 1);

        let mut total_full_scan_us = 0;
        let mut total_selection_scan_us = 0;
        let mut total_selection_generation_us = 0;
        let mut last_full_update_count = 0;
        let mut last_selection_update_count = 0;
        let mut last_selection_stats = TerminalOutputCommandStats::default();
        for frame in 0..FRAME_COUNT {
            let (full_updates, full_metrics) = diff_buffers_with_metrics(&base, &full_next);

            let old_y = frame % area.height as usize;
            let new_y = (frame + 1) % area.height as usize;
            let old_x = (frame * 3) % (area.width as usize - HIGHLIGHT_LEN);
            let new_x = (frame * 5 + 7) % (area.width as usize - HIGHLIGHT_LEN);
            let mut previous = base.clone();
            let mut next = base.clone();
            apply_test_highlight(&mut previous, old_x, old_y, HIGHLIGHT_LEN);
            apply_test_highlight(&mut next, new_x, new_y, HIGHLIGHT_LEN);

            let (selection_updates, selection_metrics) =
                diff_buffers_with_metrics(&previous, &next);
            last_selection_stats = draw_commands_output_stats(&selection_updates);

            total_full_scan_us += full_metrics.buffer_scan_duration_us;
            total_selection_scan_us += selection_metrics.buffer_scan_duration_us;
            total_selection_generation_us += selection_metrics.command_generation_duration_us;
            last_full_update_count = full_updates.len();
            last_selection_update_count = selection_updates.len();
        }

        assert!(last_full_update_count > area.width as usize);
        assert!(last_selection_update_count <= HIGHLIGHT_LEN * 2);
        assert!(last_selection_stats.print_command_count <= (HIGHLIGHT_LEN * 2) as u64);
        assert!(total_selection_scan_us > 0);
        assert!(total_selection_generation_us > 0);
        assert!(total_selection_scan_us < total_full_scan_us);
        eprintln!(
            "frames={FRAME_COUNT} cells={} highlight_len={HIGHLIGHT_LEN} full_updates={last_full_update_count} selection_updates={last_selection_update_count} selection_cursor_moves={} selection_style_commands={} selection_print_commands={} full_scan_us={total_full_scan_us} selection_scan_us={total_selection_scan_us} selection_generation_us={total_selection_generation_us}",
            area.width as usize * area.height as usize,
            last_selection_stats.cursor_move_count,
            last_selection_stats.style_command_count,
            last_selection_stats.print_command_count,
        );
    }

    #[test]
    #[ignore = "manual stress test for terminal diff stage metrics"]
    fn terminal_diff_stage_metrics_stress_splits_scan_and_generation() {
        const FRAME_COUNT: usize = 1_000;
        let area = Rect::new(0, 0, 120, 40);
        let mut previous = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        fill_test_grid(&mut previous, 0);
        fill_test_grid(&mut next, 1);

        let mut total_diff_us = 0;
        let mut total_scan_us = 0;
        let mut total_generation_us = 0;
        let mut last_update_count = 0;
        for frame in 0..FRAME_COUNT {
            fill_test_grid(&mut previous, frame);
            fill_test_grid(&mut next, frame + 1);
            let (updates, metrics) = diff_buffers_with_metrics(&previous, &next);
            assert!(metrics.total_duration_us >= metrics.buffer_scan_duration_us);
            assert!(metrics.total_duration_us >= metrics.command_generation_duration_us);
            total_diff_us += metrics.total_duration_us;
            total_scan_us += metrics.buffer_scan_duration_us;
            total_generation_us += metrics.command_generation_duration_us;
            last_update_count = updates.len();
        }

        assert!(last_update_count > 0);
        assert!(total_scan_us > 0);
        assert!(total_generation_us > 0);
        eprintln!(
            "frames={FRAME_COUNT} cells={} logical_updates={last_update_count} diff_us={total_diff_us} scan_us={total_scan_us} generation_us={total_generation_us}",
            area.width as usize * area.height as usize,
        );
    }

    fn resized_test_buffer(buffer: &Buffer, area: Rect) -> Buffer {
        let mut resized = buffer.clone();
        resized.resize(area);
        resized
    }

    fn buffer_cell_count(area: Rect) -> usize {
        area.width as usize * area.height as usize
    }

    fn apply_test_highlight(buffer: &mut Buffer, x: usize, y: usize, len: usize) {
        let width = buffer.area.width as usize;
        let row_start = y * width;
        for index in row_start + x..row_start + x + len {
            buffer.content[index].modifier |= Modifier::REVERSED;
        }
    }

    fn apply_test_overlay(buffer: &mut Buffer, area: Rect) {
        for y in area.y as usize..(area.y + area.height) as usize {
            for x in area.x as usize..(area.x + area.width) as usize {
                set_test_cell(
                    buffer,
                    x,
                    y,
                    "@",
                    Color::White,
                    Color::Black,
                    Modifier::BOLD | Modifier::REVERSED,
                );
            }
        }
    }

    fn fill_test_grid(buffer: &mut Buffer, offset: usize) {
        let width = buffer.area.width as usize;
        for y in 0..buffer.area.height as usize {
            for x in 0..width {
                let index = y * width + x;
                let value = (x + y + offset) % 26;
                let symbol = char::from(b'a' + value as u8).to_string();
                let fg = match (x + offset) % 3 {
                    0 => Color::Red,
                    1 => Color::Green,
                    _ => Color::Blue,
                };
                let modifier = if (x + y + offset).is_multiple_of(2) {
                    Modifier::BOLD
                } else {
                    Modifier::ITALIC
                };
                buffer.content[index]
                    .set_symbol(&symbol)
                    .set_fg(fg)
                    .set_bg(Color::Reset);
                buffer.content[index].modifier = modifier;
            }
        }
    }

    fn set_test_cell(
        buffer: &mut Buffer,
        x: usize,
        y: usize,
        symbol: &str,
        fg: Color,
        bg: Color,
        modifier: Modifier,
    ) {
        let index = y * buffer.area.width as usize + x;
        buffer.content[index]
            .set_symbol(symbol)
            .set_fg(fg)
            .set_bg(bg);
        buffer.content[index].modifier = modifier;
    }
}
