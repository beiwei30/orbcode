use std::time::Instant;

use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use crate::overlays::overlay_should_hide_footer;
use crate::render::text_utils::{StyledLine, truncate_chars};
use crate::state::TuiState;
use crate::tui_theme::{accent_style, active_palette, inactive_style, subtle_style, warning_style};

const MAX_CUSTOM_CMD_OUTPUT_LEN: usize = 40;

pub(crate) const FOOTER_STATUS_TIMEOUT_MS: u64 = 5_000;

impl TuiState {
    pub(crate) fn set_status_line(&mut self, status: impl Into<String>) {
        self.status_line = status.into();
        self.status_line_set_at = Some(Instant::now());
    }

    pub(crate) fn clear_status_line(&mut self) {
        self.status_line.clear();
        self.status_line_set_at = None;
    }

    pub(crate) fn prune_status_line(&mut self) {
        let should_clear = self.status_line_set_at.is_some_and(|set_at| {
            set_at.elapsed().as_millis() >= FOOTER_STATUS_TIMEOUT_MS as u128
                && !self.status_line_should_persist()
        });
        if should_clear {
            self.status_line.clear();
            self.status_line_set_at = None;
        }
    }

    pub(crate) fn status_line_should_persist(&self) -> bool {
        let lower = self.status_line.to_ascii_lowercase();
        lower.contains("error")
            || lower.contains("failed")
            || lower.contains("cancel")
            || lower.contains("context warning")
            || lower.contains("context critical")
            || lower.contains("context limit")
            || lower.contains("auto-compact")
    }

    pub(crate) fn overlay_hides_footer(&self) -> bool {
        overlay_should_hide_footer(self.overlay.as_ref())
    }

    pub(crate) fn footer_left_line(&self) -> StyledLine {
        // The mode indicators (vim `-- INSERT --` / `-- NORMAL --` and
        // `-- HISTORY --`) are intentionally omitted. Only the in-flight
        // request hint remains.
        let mut spans: Vec<Span> = Vec::new();
        if self.request_in_flight {
            if self.input.trim().is_empty() {
                spans.push(Span::styled("esc", inactive_style()));
                spans.push(Span::styled(" to interrupt", subtle_style()));
            } else {
                spans.push(Span::styled("tab", accent_style()));
                spans.push(Span::styled(" to queue message", subtle_style()));
            }
        }

        Line::from(spans)
    }

    /// Plain-text mirror of [`Self::footer_right_line`], used by tests to assert
    /// status-bar content without walking styled spans. Production rendering
    /// uses the styled line directly.
    #[cfg(test)]
    pub(crate) fn footer_right_text(&self) -> String {
        if self.show_update_notice {
            "Update available! Run: brew upgrade claude-code".to_string()
        } else if let Some(transient) = self.active_transient_status() {
            transient
        } else {
            self.status_bar_text()
        }
    }

    pub(crate) fn footer_right_line(&self) -> StyledLine {
        if self.show_update_notice {
            return Line::from(vec![
                Span::styled("Update available! Run: ", warning_style()),
                Span::styled(
                    "brew upgrade claude-code",
                    Style::default()
                        .fg(active_palette().warning)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
        }

        if let Some(transient) = self.active_transient_status() {
            return Line::from(Span::styled(transient, self.footer_status_style()));
        }

        self.status_bar_line()
    }

    fn active_transient_status(&self) -> Option<String> {
        if self.request_in_flight {
            return None;
        }
        let text = self.transient_footer_status();
        if text.is_empty() { None } else { Some(text) }
    }

    /// Model (with its runtime effort) label shown at the head of the status
    /// line, e.g. `claude-opus-4-8 high`.
    fn status_model_label(&self) -> String {
        let mut label = short_model_name(&self.model_display_name);
        if let Some(effort) = self.status.effort {
            label.push(' ');
            label.push_str(effort.as_str());
        }
        label
    }

    /// Active warning labels (context nearly exhausted, rate-limit, auth). These
    /// are appended to the status line only while a warning condition holds, so
    /// the default status stays `<model> · <cwd> · <mode>`. Test-only: the styled
    /// renderer ([`Self::status_bar_line`]) inlines the same conditions.
    #[cfg(test)]
    fn status_warning_labels(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(pct) = self.status.context_percent_left
            && pct <= 25
        {
            let used = 100u32.saturating_sub(pct);
            warnings.push(format!("ctx:{used}%"));
        }
        if self.status.has_rate_limit_warning {
            warnings.push("rate-limit".to_string());
        }
        if self.status.has_auth_warning {
            warnings.push("auth-err".to_string());
        }
        warnings
    }

    #[cfg(test)]
    fn status_bar_text(&self) -> String {
        let mut parts: Vec<String> = vec![
            self.status_model_label(),
            self.cwd_display.clone(),
            self.status.permission_mode.label().to_string(),
        ];
        parts.extend(self.status_warning_labels());
        if let Some(ref output) = self.status.custom_command_output {
            parts.push(truncate_chars(output, MAX_CUSTOM_CMD_OUTPUT_LEN));
        }
        parts.join(" \u{b7} ")
    }

    fn status_bar_line(&self) -> StyledLine {
        let palette = active_palette();
        let sep = Span::styled(" \u{b7} ", subtle_style());
        // The cwd shares the model's style; only the ` · ` separator is dimmed.
        let mut spans: Vec<Span> = vec![
            Span::styled(self.status_model_label(), inactive_style()),
            sep.clone(),
            Span::styled(self.cwd_display.clone(), inactive_style()),
            sep.clone(),
            Span::styled(self.status.permission_mode.label(), inactive_style()),
        ];

        if let Some(pct) = self.status.context_percent_left
            && pct <= 25
        {
            let used = 100u32.saturating_sub(pct);
            let ctx_style = if pct <= 10 {
                warning_style()
            } else {
                Style::default().fg(palette.warning)
            };
            spans.push(sep.clone());
            spans.push(Span::styled(format!("ctx:{used}%"), ctx_style));
        }
        if self.status.has_rate_limit_warning {
            spans.push(sep.clone());
            spans.push(Span::styled("rate-limit", warning_style()));
        }
        if self.status.has_auth_warning {
            spans.push(sep.clone());
            spans.push(Span::styled("auth-err", warning_style()));
        }

        if let Some(ref output) = self.status.custom_command_output {
            spans.push(sep);
            spans.push(Span::styled(
                truncate_chars(output, MAX_CUSTOM_CMD_OUTPUT_LEN),
                subtle_style(),
            ));
        }

        Line::from(spans)
    }

    pub(crate) fn footer_status_style(&self) -> Style {
        let lower = self.status_line.to_ascii_lowercase();
        if lower.contains("error")
            || lower.contains("failed")
            || lower.contains("cancel")
            || lower.contains("context warning")
            || lower.contains("context critical")
            || lower.contains("context limit")
            || lower.contains("auto-compact")
        {
            warning_style()
        } else {
            subtle_style()
        }
    }

    pub(crate) fn transient_footer_status(&self) -> String {
        if self.status_line.is_empty() {
            return String::new();
        }

        if matches!(
            self.status_line.as_str(),
            "Ready."
                | "New session ready. Enter submits. /help shows shell commands."
                | "Session resumed. Enter submits. /help shows shell commands."
        ) {
            String::new()
        } else {
            truncate_chars(&self.status_line, 80)
        }
    }
}

fn short_model_name(display_name: &str) -> String {
    // Drop the redundant vendor prefix but show the full name — no width cap.
    // The footer is a single row, so anything wider than the terminal is
    // clipped at the edge rather than shortened with an ellipsis.
    display_name
        .trim_start_matches("Claude ")
        .trim_start_matches("claude-")
        .to_string()
}
