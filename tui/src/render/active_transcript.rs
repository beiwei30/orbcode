use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use crate::render::background_agent_panel::render_background_task_summary_lines;
use crate::render::text_utils::{StyledLine, format_duration_short, push_blank_line_if_needed};
use crate::render::thinking::render_active_thinking_lines;
use crate::state::TuiState;
use crate::tool_cell::render::render_live_tool_activity_lines;
use crate::tui_theme::{active_palette, subtle_style};

pub(crate) struct ActiveTranscriptSnapshot {
    pub(crate) lines: Vec<StyledLine>,
    pub(crate) revision: u64,
}

impl TuiState {
    pub(crate) fn append_active_transcript_lines(
        &self,
        lines: &mut Vec<StyledLine>,
        transcript_width: usize,
        show_empty_placeholder: bool,
    ) {
        lines.extend(self.active_transcript_lines(
            transcript_width,
            show_empty_placeholder,
            !lines.is_empty(),
        ));
    }

    pub(crate) fn active_transcript_lines(
        &self,
        transcript_width: usize,
        show_empty_placeholder: bool,
        stable_has_lines: bool,
    ) -> Vec<StyledLine> {
        let mut lines = Vec::new();
        let now = Instant::now();
        let background_tasks_visible = self.transcript_task_cards.is_visible(now);
        let pending_assistant_lines = self.pending_assistant_live_lines(transcript_width);
        let pending_assistant_visible = !pending_assistant_lines.is_empty();
        let _ = show_empty_placeholder;

        let mut rendered_live_section = false;

        let should_render_active_thinking = self.active_thinking.as_ref().is_some_and(|thinking| {
            self.is_active_thinking_visible()
                && (thinking.is_streaming || !pending_assistant_visible)
        });
        if should_render_active_thinking {
            let thinking = self.active_thinking.as_ref().expect("checked above");
            push_active_transcript_separator(&mut lines, stable_has_lines);
            lines.extend(render_active_thinking_lines(
                thinking,
                false,
                self.current_request_spinner(),
                self.current_thinking_verb(),
                transcript_width,
            ));
            rendered_live_section = true;
        }

        for activity in self.live_tool_activities_to_render() {
            if rendered_live_section {
                lines.push(Line::default());
            } else {
                push_active_transcript_separator(&mut lines, stable_has_lines);
            }
            lines.extend(render_live_tool_activity_lines(
                activity,
                false,
                &self.cwd,
                self.current_tool_blink_visible(),
                transcript_width,
            ));
            rendered_live_section = true;
        }

        if background_tasks_visible {
            if rendered_live_section {
                lines.push(Line::default());
            } else {
                push_active_transcript_separator(&mut lines, stable_has_lines);
            }
            lines.extend(render_background_task_summary_lines(
                self.transcript_task_cards.rows(),
            ));
            rendered_live_section = true;
        }

        if let Some(started_at) = self.compact_started_at {
            if rendered_live_section {
                lines.push(Line::default());
            } else {
                push_active_transcript_separator(&mut lines, stable_has_lines);
            }
            lines.extend(render_compacting_lines(
                self.current_request_spinner(),
                started_at,
                self.active_request_tip_text().as_deref(),
            ));
            rendered_live_section = true;
        }

        if pending_assistant_visible {
            if rendered_live_section {
                lines.push(Line::default());
            } else {
                push_active_transcript_separator(&mut lines, stable_has_lines);
            }
            lines.extend(pending_assistant_lines);
        }

        if !self.request_in_flight
            && self.compact_started_at.is_none()
            && let Some(tip) = self.active_request_tip_text()
        {
            if stable_has_lines || !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::from(vec![Span::styled(tip, subtle_style())]));
        }

        lines
    }

    pub(crate) fn active_transcript_snapshot(
        &self,
        transcript_width: usize,
    ) -> ActiveTranscriptSnapshot {
        let revision = self.active_snapshot_revision(transcript_width);
        let mut lines = Vec::new();
        let now = Instant::now();
        let background_tasks_visible = self.transcript_task_cards.is_visible(now);
        let pending_assistant_lines = self.pending_assistant_live_lines(transcript_width);
        let pending_assistant_visible = !pending_assistant_lines.is_empty();

        let mut rendered_live_section = false;

        let should_render_active_thinking = self.active_thinking.as_ref().is_some_and(|thinking| {
            self.is_active_thinking_visible()
                && (thinking.is_streaming || !pending_assistant_visible)
        });
        if should_render_active_thinking {
            let thinking = self.active_thinking.as_ref().expect("checked above");
            lines.extend(render_active_thinking_lines(
                thinking,
                true,
                self.current_request_spinner(),
                self.current_thinking_verb(),
                transcript_width,
            ));
            rendered_live_section = true;
        }

        for activity in self.live_tool_activities_to_render() {
            if rendered_live_section {
                lines.push(Line::default());
            }
            lines.extend(render_live_tool_activity_lines(
                activity,
                true,
                &self.cwd,
                self.current_tool_blink_visible(),
                transcript_width,
            ));
            rendered_live_section = true;
        }

        if background_tasks_visible {
            if rendered_live_section {
                lines.push(Line::default());
            }
            lines.extend(render_background_task_summary_lines(
                self.transcript_task_cards.rows(),
            ));
            rendered_live_section = true;
        }

        if let Some(started_at) = self.compact_started_at {
            if rendered_live_section {
                lines.push(Line::default());
            }
            lines.extend(render_compacting_lines(
                self.current_request_spinner(),
                started_at,
                None,
            ));
            rendered_live_section = true;
        }

        if pending_assistant_visible {
            if rendered_live_section {
                lines.push(Line::default());
            }
            lines.extend(pending_assistant_lines);
        }

        ActiveTranscriptSnapshot { lines, revision }
    }

    fn active_snapshot_revision(&self, width: usize) -> u64 {
        let mut hasher = DefaultHasher::new();
        width.hash(&mut hasher);
        self.current_tool_blink_visible().hash(&mut hasher);
        if let Some(thinking) = &self.active_thinking {
            thinking.text.hash(&mut hasher);
            thinking.is_streaming.hash(&mut hasher);
        }
        self.pending_assistant.hash(&mut hasher);
        self.transcript_ui
            .emission
            .assistant_stream_emitted_line_count
            .hash(&mut hasher);
        self.transcript_ui
            .emission
            .assistant_stream_pending_line_count
            .hash(&mut hasher);
        self.compact_started_at.is_some().hash(&mut hasher);
        for activity in self.live_tool_activities_to_render() {
            activity.status_line.hash(&mut hasher);
            activity.tool_use_id.hash(&mut hasher);
            for msg in &activity.progress_messages {
                msg.to_string().hash(&mut hasher);
            }
        }
        let now = Instant::now();
        self.transcript_task_cards.is_visible(now).hash(&mut hasher);
        if self.transcript_task_cards.is_visible(now) {
            for row in self.transcript_task_cards.rows() {
                row.task_id.hash(&mut hasher);
                row.description.hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

pub(crate) fn render_compacting_lines(
    spinner: char,
    started_at: Instant,
    tip: Option<&str>,
) -> Vec<StyledLine> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            spinner.to_string(),
            Style::default()
                .fg(active_palette().claude)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!(
                "Compacting conversation... ({})",
                format_duration_short(started_at.elapsed().as_millis().min(u64::MAX as u128) as u64)
            ),
            Style::default()
                .fg(active_palette().claude)
                .add_modifier(Modifier::ITALIC),
        ),
    ])];
    if let Some(tip) = tip {
        lines.push(Line::from(vec![
            Span::styled("  └ ", subtle_style()),
            Span::styled(tip.to_string(), subtle_style()),
        ]));
    }
    lines
}

fn push_active_transcript_separator(lines: &mut Vec<StyledLine>, stable_has_lines: bool) {
    if lines.is_empty() {
        if stable_has_lines {
            lines.push(Line::default());
        }
    } else {
        push_blank_line_if_needed(lines);
    }
}
