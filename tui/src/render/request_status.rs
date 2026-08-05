use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use std::time::Instant;

use crate::embedded_progress::normalize_progress_label;
use crate::numeric::saturating_u16;
use crate::overlays::{OverlayState, overlay_request_panel};
use crate::render::task_panel::{TaskPanelLayout, render_task_panel_lines};
use crate::render::text_utils::{StyledLine, format_duration_short};
use crate::state::TuiState;
use crate::tui_theme::active_palette;
#[cfg(test)]
use crate::tui_theme::subtle_style;

pub(crate) const SPINNER_TICK_MS: u64 = 120;

const TOOL_BLINK_INTERVAL_MS: u64 = 600;
const WAITING_SPINNER_FRAMES: [char; 10] = ['·', '✢', '✳', '✶', '✻', '*', '✻', '✶', '✳', '✢'];
pub(crate) const WAITING_VERBS: [&str; 10] = [
    "Combobulating",
    "Thinking",
    "Percolating",
    "Synthesizing",
    "Contemplating",
    "Processing",
    "Pondering",
    "Brewing",
    "Reasoning",
    "Cooking",
];
const THINKING_VERBS: [&str; 10] = [
    "Pontificating",
    "Percolating",
    "Pondering",
    "Reasoning",
    "Synthesizing",
    "Contemplating",
    "Philosophising",
    "Processing",
    "Perusing",
    "Combobulating",
];
pub(crate) const WAITING_COMPLETION_VERBS: [&str; 10] = [
    "Combobulated",
    "Thought",
    "Percolated",
    "Synthesized",
    "Contemplated",
    "Processed",
    "Pondered",
    "Brewed",
    "Reasoned",
    "Cooked",
];
const ACTIVE_REQUEST_TIPS: [&str; 16] = [
    "Press Esc to interrupt the active turn.",
    "Use /clear to start fresh when switching topics.",
    "Use /resume to revisit earlier sessions.",
    "Use /permissions to pre-approve and pre-deny bash, edit, and MCP tools.",
    "Press Ctrl+R to browse resumable sessions.",
    "Press Ctrl+O to expand or collapse tool details.",
    "Use /status to inspect session, model, provider, sandbox, and tools.",
    "Use /context --full to inspect context window diagnostics.",
    "Use /usage to review token usage for this session.",
    "Use /cost to review cumulative session cost.",
    "Use /doctor to check auth, sandbox, and toolchain health.",
    "Use /diff to inspect the current workspace diff.",
    "Use /files to list recently referenced files and working directories.",
    "Use /fork to branch this conversation into a new session.",
    "Use /rewind to restore the conversation to a previous user turn.",
    "Use /mcp status to inspect configured MCP servers.",
];

impl TuiState {
    pub(crate) fn current_request_elapsed_ms(&self) -> Option<u64> {
        self.request_started_at
            .map(|started_at| started_at.elapsed().as_millis().min(u64::MAX as u128) as u64)
    }

    fn estimated_stream_tokens(&self) -> u64 {
        self.streamed_response_chars
            .saturating_add(3)
            .saturating_div(4) as u64
    }

    fn estimated_request_status_tokens(&self) -> u64 {
        self.estimated_stream_tokens()
    }

    fn current_request_status_label(&self) -> String {
        if let Some(activity) = self.latest_active_live_tool_activity() {
            return normalize_progress_label(&activity.status_line);
        }

        if self
            .active_thinking
            .as_ref()
            .is_some_and(|thinking| thinking.is_streaming)
        {
            return "Thinking...".to_string();
        }

        if let Some(label) = self.task_panel.active_task_status_label() {
            return label;
        }

        format!(
            "{}...",
            WAITING_VERBS[self.spinner_verb_index % WAITING_VERBS.len()]
        )
    }

    pub(crate) fn current_request_spinner(&self) -> char {
        WAITING_SPINNER_FRAMES[self.spinner_frame % WAITING_SPINNER_FRAMES.len()]
    }

    pub(crate) fn current_thinking_verb(&self) -> &'static str {
        THINKING_VERBS[self.spinner_verb_index % THINKING_VERBS.len()]
    }

    pub(crate) fn current_tool_blink_visible(&self) -> bool {
        let frames_per_phase = (TOOL_BLINK_INTERVAL_MS / SPINNER_TICK_MS).max(1) as usize;
        (self.spinner_frame / frames_per_phase).is_multiple_of(2)
    }

    fn should_animate(&self) -> bool {
        self.request_in_flight
            && self.pending_assistant.trim().is_empty()
            && !self.is_active_thinking_visible()
    }

    fn advance_animation(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % WAITING_SPINNER_FRAMES.len();
    }

    pub(crate) fn needs_periodic_tick(&self) -> bool {
        self.request_in_flight
            || self.compact_started_at.is_some()
            || self.should_animate()
            || self
                .active_thinking
                .as_ref()
                .and_then(|thinking| thinking.completed_at)
                .is_some()
    }

    pub(crate) fn on_tick(&mut self) {
        if self.request_in_flight || self.compact_started_at.is_some() {
            self.advance_animation();
        }
        self.prune_active_thinking();
        self.prune_status_line();
    }

    fn active_request_status_text(&self) -> String {
        let label = self.current_request_status_label();
        self.status_text_with_elapsed(&label)
    }

    #[cfg(test)]
    fn global_spinner_status_text(&self) -> String {
        let label = format!(
            "{}...",
            WAITING_VERBS[self.spinner_verb_index % WAITING_VERBS.len()]
        );
        self.status_text_with_elapsed(&label)
    }

    fn status_text_with_elapsed(&self, label: &str) -> String {
        let elapsed_ms = self.current_request_elapsed_ms().unwrap_or(0);
        let elapsed = format_duration_short(elapsed_ms);
        let estimated_tokens = self.estimated_request_status_tokens();
        let direction = self.request_token_direction.glyph();
        let estimated_tokens = format_token_estimate(estimated_tokens);
        let separator = if label.ends_with('…') { " " } else { "" };
        format!("{label}{separator}({elapsed} · {direction} {estimated_tokens} tokens)")
    }

    pub(crate) fn active_request_tip_text(&self) -> Option<String> {
        if self.overlay.is_some() {
            return None;
        }
        if self.compact_started_at.is_some() {
            let index = self.request_count.saturating_sub(1) % ACTIVE_REQUEST_TIPS.len();
            return Some(format!("Tip: {}", ACTIVE_REQUEST_TIPS[index]));
        }
        if !self.request_in_flight {
            return None;
        }

        if self.has_live_tool_activity() {
            return Some("Tip: Press Ctrl+O to open full transcript.".to_string());
        }

        let index = self.request_count.saturating_sub(1) % ACTIVE_REQUEST_TIPS.len();
        Some(format!("Tip: {}", ACTIVE_REQUEST_TIPS[index]))
    }

    pub(crate) fn request_status_height_for_layout(&mut self, width: usize) -> u16 {
        let cwd = self.cwd.clone();
        if matches!(
            self.overlay,
            Some(OverlayState::PermissionRequest(_) | OverlayState::AskUserQuestion(_))
        ) {
            return 0;
        }
        if let Some(panel) = overlay_request_panel(self.overlay.as_mut(), &cwd, width) {
            return panel.height();
        }
        match self.overlay {
            Some(
                OverlayState::Help(_)
                | OverlayState::KeybindHelp(_)
                | OverlayState::Diff(_)
                | OverlayState::BackgroundJobs(_)
                | OverlayState::TranscriptPager(_),
            ) => saturating_u16(self.request_status_lines().len()),
            Some(
                OverlayState::AddDirPicker(_)
                | OverlayState::SessionPicker(_)
                | OverlayState::ModelPicker(_)
                | OverlayState::ThemePicker(_)
                | OverlayState::OutputStylePicker(_)
                | OverlayState::ConfigPicker(_)
                | OverlayState::SandboxPicker(_)
                | OverlayState::MemoryPicker(_)
                | OverlayState::PermissionPicker(_)
                | OverlayState::RewindPicker(_)
                | OverlayState::PermissionRequest(_)
                | OverlayState::AskUserQuestion(_),
            ) => unreachable!("overlay request panel should be handled before fallback"),
            None if !self.request_in_flight => {
                let standalone_panel = self.request_status_lines_for_width(width).len();
                if standalone_panel > 0 {
                    saturating_u16(standalone_panel)
                } else {
                    saturating_u16(self.cached_slash_command_suggestion_lines(width).len())
                }
            }
            None => saturating_u16(self.request_status_lines_for_width(width).len()),
        }
    }

    pub(crate) fn request_status_lines(&self) -> Vec<StyledLine> {
        self.request_status_lines_for_width(usize::MAX)
    }

    pub(crate) fn request_status_lines_for_width(&self, width: usize) -> Vec<StyledLine> {
        let mut lines = Vec::new();
        let now = Instant::now();
        if self.request_in_flight {
            lines.push(Line::from(vec![
                Span::styled(
                    self.current_request_spinner().to_string(),
                    Style::default()
                        .fg(active_palette().claude)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    self.active_request_status_text(),
                    Style::default()
                        .fg(active_palette().claude)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
            let panel_lines =
                render_task_panel_lines(&self.task_panel, TaskPanelLayout::Nested, width, now);
            lines.extend(panel_lines);
        } else {
            let panel_lines =
                render_task_panel_lines(&self.task_panel, TaskPanelLayout::Standalone, width, now);
            lines.extend(panel_lines);
        }
        lines
    }

    #[cfg(test)]
    pub(crate) fn render_waiting_assistant_lines(&self, include_tip: bool) -> Vec<StyledLine> {
        let spinner = WAITING_SPINNER_FRAMES[self.spinner_frame % WAITING_SPINNER_FRAMES.len()];
        let mut lines = vec![Line::from(vec![
            Span::styled(
                spinner.to_string(),
                Style::default()
                    .fg(active_palette().claude)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                self.global_spinner_status_text(),
                Style::default()
                    .fg(active_palette().claude)
                    .add_modifier(Modifier::ITALIC),
            ),
        ])];

        if include_tip
            && let Some(tip) = self.active_request_tip_text()
            && !lines.is_empty()
        {
            lines.push(Line::from(vec![
                Span::styled("  └ ", subtle_style()),
                Span::styled(tip, subtle_style()),
            ]));
        }

        lines
    }
}

pub(crate) fn format_token_estimate(tokens: u64) -> String {
    const UNITS: &[(u64, &str)] = &[
        (1_000_000_000_000, "t"),
        (1_000_000_000, "b"),
        (1_000_000, "m"),
        (1_000, "k"),
    ];

    for (factor, suffix) in UNITS {
        if tokens >= *factor {
            let value = tokens as f64 / *factor as f64;
            let formatted = format!("{value:.1}").replace(".0", "");
            return format!("{formatted}{suffix}");
        }
    }

    tokens.to_string()
}
