use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};
use ratatui::{backend::CrosstermBackend, prelude::Size};
use serde_json::json;

use crate::custom_terminal::{Terminal, TerminalDrawMetrics};
use crate::state::TuiState;

const RENDER_METRICS_ENV: &str = "ORBCODE_TUI_RENDER_METRICS";
const RENDER_METRICS_PATH_ENV: &str = "ORBCODE_TUI_RENDER_METRICS_PATH";

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub(crate) struct RenderEventCounts {
    pub terminal_events: u64,
    pub stream_events: u64,
    pub local_command_events: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderFrameContext<'a> {
    pub redraw_reasons: &'a [&'static str],
    pub event_counts: RenderEventCounts,
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub viewport_width: u16,
    pub viewport_height: u16,
    pub visible_line_count: usize,
    pub total_line_count: usize,
    pub live_tool_count: usize,
    pub live_progress_message_count: usize,
}

pub(crate) struct RenderMetricsRecorder {
    path: PathBuf,
    frame_index: u64,
    writer: BufWriter<File>,
}

impl TuiState {
    pub(crate) fn render_metrics_context<'a>(
        &self,
        terminal: &Terminal<CrosstermBackend<io::Stdout>>,
        size: Size,
        redraw_reasons: &'a [&'static str],
        event_counts: RenderEventCounts,
    ) -> RenderFrameContext<'a> {
        RenderFrameContext {
            redraw_reasons,
            event_counts,
            terminal_width: size.width,
            terminal_height: size.height,
            viewport_width: terminal.viewport_area.width,
            viewport_height: terminal.viewport_area.height,
            visible_line_count: self.transcript_ui.viewport.lines.len(),
            total_line_count: self.transcript_ui.viewport.all_lines.len(),
            live_tool_count: self.live_tool_cells.activities.len(),
            live_progress_message_count: self
                .live_tool_cells
                .activities
                .iter()
                .map(|activity| activity.progress_messages.len())
                .sum(),
        }
    }
}

impl RenderMetricsRecorder {
    pub(crate) fn from_env() -> io::Result<Option<Self>> {
        if !render_metrics_enabled(std::env::var(RENDER_METRICS_ENV).ok().as_deref()) {
            return Ok(None);
        }
        Self::new(render_metrics_path_from_env()).map(Some)
    }

    fn new(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let writer = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            frame_index: 0,
            writer: BufWriter::new(writer),
        })
    }

    pub(crate) fn record_frame(
        &mut self,
        draw: &TerminalDrawMetrics,
        context: RenderFrameContext<'_>,
    ) -> io::Result<()> {
        let payload = json!({
            "type": "tui_render_frame",
            "frame_index": self.frame_index,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "metrics_file": self.path.display().to_string(),
            "redraw_reasons": context.redraw_reasons,
            "event_counts": {
                "terminal_events": context.event_counts.terminal_events,
                "stream_events": context.event_counts.stream_events,
                "local_command_events": context.event_counts.local_command_events,
            },
            "duration_us": {
                "total": draw.total_duration_us,
                "render": draw.render_duration_us,
                "diff": draw.diff_duration_us,
                "diff_buffer_scan": draw.diff_buffer_scan_duration_us,
                "diff_command_generation": draw.diff_command_generation_duration_us,
                "terminal_write": draw.terminal_write_duration_us,
                "backend_flush": draw.backend_flush_duration_us,
            },
            "terminal": {
                "width": context.terminal_width,
                "height": context.terminal_height,
                "viewport_width": context.viewport_width,
                "viewport_height": context.viewport_height,
                "buffer_cell_count": draw.buffer_cell_count,
            },
            "transcript": {
                "visible_line_count": context.visible_line_count,
                "total_line_count": context.total_line_count,
            },
            "live_tools": {
                "count": context.live_tool_count,
                "progress_message_count": context.live_progress_message_count,
            },
            "output": {
                "draw_command_count": draw.draw_command_count,
                "terminal_cursor_move_count": draw.terminal_cursor_move_count,
                "terminal_style_command_count": draw.terminal_style_command_count,
                "terminal_print_command_count": draw.terminal_print_command_count,
                "terminal_clear_command_count": draw.terminal_clear_command_count,
                "bytes": draw.output_bytes,
                "initial_frame": draw.initial_frame,
            },
        });
        self.frame_index += 1;
        writeln!(self.writer, "{payload}")?;
        self.writer.flush()
    }
}

fn render_metrics_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

fn default_metrics_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "orbcode-tui-render-metrics-{}.jsonl",
        std::process::id()
    ))
}

fn render_metrics_path_from_env() -> PathBuf {
    std::env::var_os(RENDER_METRICS_PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(default_metrics_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn render_metrics_enabled_only_for_one() {
        assert!(render_metrics_enabled(Some("1")));
        assert!(!render_metrics_enabled(Some("true")));
        assert!(!render_metrics_enabled(Some("")));
        assert!(!render_metrics_enabled(None));
    }

    #[test]
    fn record_frame_writes_jsonl_payload() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orbcode-render-metrics-test-{}-{suffix}.jsonl",
            std::process::id()
        ));
        let mut recorder =
            RenderMetricsRecorder::new(path.clone()).expect("create metrics recorder");
        let reasons = ["initial", "stream_event"];
        let draw = TerminalDrawMetrics {
            total_duration_us: 10,
            render_duration_us: 4,
            diff_duration_us: 2,
            diff_buffer_scan_duration_us: 1,
            diff_command_generation_duration_us: 1,
            terminal_write_duration_us: 3,
            backend_flush_duration_us: 1,
            draw_command_count: 7,
            terminal_cursor_move_count: 2,
            terminal_style_command_count: 3,
            terminal_print_command_count: 4,
            terminal_clear_command_count: 1,
            output_bytes: 123,
            buffer_cell_count: 80,
            initial_frame: true,
        };
        let context = RenderFrameContext {
            redraw_reasons: &reasons,
            event_counts: RenderEventCounts {
                terminal_events: 1,
                stream_events: 2,
                local_command_events: 3,
            },
            terminal_width: 20,
            terminal_height: 8,
            viewport_width: 20,
            viewport_height: 6,
            visible_line_count: 5,
            total_line_count: 12,
            live_tool_count: 1,
            live_progress_message_count: 4,
        };

        recorder
            .record_frame(&draw, context)
            .expect("write metrics frame");
        drop(recorder);

        let content = fs::read_to_string(&path).expect("read metrics file");
        let value: Value = serde_json::from_str(content.trim()).expect("parse metrics JSON");
        assert_eq!(value["type"], "tui_render_frame");
        assert_eq!(value["frame_index"], 0);
        assert_eq!(value["redraw_reasons"][1], "stream_event");
        assert_eq!(value["duration_us"]["render"], 4);
        assert_eq!(value["duration_us"]["diff_buffer_scan"], 1);
        assert_eq!(value["duration_us"]["diff_command_generation"], 1);
        assert_eq!(value["terminal"]["buffer_cell_count"], 80);
        assert_eq!(value["live_tools"]["progress_message_count"], 4);
        assert_eq!(value["output"]["terminal_style_command_count"], 3);
        assert_eq!(value["output"]["terminal_clear_command_count"], 1);
        assert_eq!(value["output"]["bytes"], 123);

        let _ = fs::remove_file(path);
    }
}
