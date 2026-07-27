use ratatui::{
    prelude::{Modifier, Style},
    style::Color,
    text::{Line, Span},
};

use crate::render::slash::stable_slash_command_tip;
use crate::render::text_utils::{
    StyledLine, styled_line_display_width, truncate_chars, truncate_path_tail,
};
use crate::state::TuiState;
use crate::tui_theme::{inactive_style, subtle_style};

fn hash_seed(seed: &str) -> [u8; 16] {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h1);
    let v1 = h1.finish();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    (seed, 0x9e3779b9_u64).hash(&mut h2);
    let v2 = h2.finish();
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&v1.to_le_bytes());
    bytes[8..].copy_from_slice(&v2.to_le_bytes());
    bytes
}

fn identicon_grid(hash: &[u8; 16]) -> [bool; 25] {
    let mut nibble_index = 0usize;
    let nibble = |idx: usize| -> u8 {
        let byte = hash[idx / 2];
        if idx.is_multiple_of(2) {
            (byte >> 4) & 0x0f
        } else {
            byte & 0x0f
        }
    };
    let mut grid = [false; 25];
    for col in (0..3).rev() {
        for row in 0..5 {
            let paint = nibble(nibble_index) % 2 == 0;
            nibble_index += 1;
            grid[row * 5 + col] = paint;
            grid[row * 5 + (4 - col)] = paint;
        }
    }
    grid
}

fn identicon_color(hash: &[u8; 16]) -> Color {
    let h = (((hash[12] & 0x0f) as u32) << 8) | hash[13] as u32;
    let hue = h as f64 * 360.0 / 4095.0;
    let sat = 65.0 - hash[14] as f64 * 20.0 / 255.0;
    let lum = 75.0 - hash[15] as f64 * 20.0 / 255.0;

    let s = sat / 100.0;
    let l = lum / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = hue / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::Rgb(
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}

fn half_block(top: bool, bot: bool) -> &'static str {
    match (top, bot) {
        (true, true) => "█",
        (true, false) => "▀",
        (false, true) => "▄",
        (false, false) => " ",
    }
}

pub(crate) fn avatar_lines(session_id: &str) -> Vec<StyledLine> {
    let hash = hash_seed(session_id);
    let grid = identicon_grid(&hash);
    let fg = identicon_color(&hash);
    let style_on = Style::default().fg(fg);
    let style_off = Style::default();
    let row_pairs: [(usize, Option<usize>); 3] = [(0, Some(1)), (2, Some(3)), (4, None)];

    let mut lines: Vec<StyledLine> = Vec::with_capacity(3);
    for (top_row, bot_row) in row_pairs {
        let mut spans = vec![Span::raw(" ")];
        for col in 0..5 {
            let top = grid[top_row * 5 + col];
            let bot = bot_row.is_some_and(|br| grid[br * 5 + col]);
            let ch = half_block(top, bot);
            spans.push(Span::styled(
                ch,
                if top || bot { style_on } else { style_off },
            ));
        }
        spans.push(Span::raw(" "));
        lines.push(Line::from(spans));
    }
    lines
}

fn format_token_count(tokens: u32) -> String {
    let s = tokens.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

impl TuiState {
    pub(crate) fn header_info_lines(
        &self,
        available_width: usize,
        cwd_width: usize,
    ) -> Vec<StyledLine> {
        let version_prefix = "Orb Code v";
        let version_room = available_width.saturating_sub(version_prefix.chars().count());
        let truncated_version = truncate_chars(&self.ui_version, version_room.max(6));
        let version_line = Line::from(vec![
            Span::styled("Orb Code", inactive_style().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(format!("v{truncated_version}"), subtle_style()),
        ]);
        let combined = format!(
            "{} · {}",
            self.model_display_name, self.default_provider_label
        );
        let model_line = if combined.chars().count() <= available_width {
            vec![Line::from(vec![
                Span::styled(self.model_display_name.clone(), inactive_style()),
                Span::styled(" · ", subtle_style()),
                Span::styled(self.default_provider_label.clone(), inactive_style()),
            ])]
        } else {
            vec![
                Line::from(Span::styled(
                    truncate_chars(&self.model_display_name, available_width.max(8)),
                    inactive_style(),
                )),
                Line::from(Span::styled(
                    truncate_chars(&self.default_provider_label, available_width.max(8)),
                    inactive_style(),
                )),
            ]
        };
        let cwd_line = Line::from(Span::styled(
            truncate_path_tail(&self.cwd_display, cwd_width.max(10)),
            inactive_style(),
        ));
        let mut lines = vec![version_line];
        lines.extend(model_line);
        lines.push(cwd_line);
        lines
    }

    pub(crate) fn intro_banner_lines(&self, available_width: usize) -> Vec<StyledLine> {
        let logo = avatar_lines(&self.session_id);
        let logo_width = logo
            .iter()
            .map(styled_line_display_width)
            .max()
            .unwrap_or(0);
        let column_gap = if available_width < 40 { 0 } else { 2 };
        let info_width = available_width
            .saturating_sub(logo_width.saturating_add(column_gap))
            .max(8);
        let info = self.header_info_lines(info_width, available_width);
        if available_width < 40 {
            let mut lines = logo;
            lines.extend(info);
            return lines;
        }

        let total_lines = logo.len().max(info.len());
        let mut lines = Vec::with_capacity(total_lines);
        for index in 0..total_lines {
            let mut spans = Vec::new();
            if let Some(left) = logo.get(index) {
                spans.extend(left.spans.clone());
                let left_width = styled_line_display_width(left);
                let pad = logo_width.saturating_sub(left_width);
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
            }
            if logo.get(index).is_some() && info.get(index).is_some() {
                spans.push(Span::raw(" ".repeat(column_gap)));
            }
            if let Some(right) = info.get(index) {
                spans.extend(right.spans.clone());
            }
            lines.push(Line::from(spans));
        }
        lines
    }

    pub(crate) fn intro_banner_cell(&self, available_width: usize) -> Vec<StyledLine> {
        let banner = self.intro_banner_lines(available_width);
        if banner.is_empty() {
            return banner;
        }

        let mut cell = Vec::with_capacity(banner.len() + 8);
        cell.push(Line::default());
        cell.extend(banner);
        cell.push(Line::default());
        cell.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Tip:", inactive_style().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(stable_slash_command_tip(&self.session_id), subtle_style()),
        ]));
        cell.push(Line::default());

        if let Some(info) = &self.clear_session_info {
            if let Some(usage) = &info.usage {
                let total = usage.component_total_tokens();
                let input = usage.input_tokens;
                let cached = usage.cache_read_input_tokens;
                let output = usage.output_tokens;
                let usage_text = format!(
                    "Token usage: total={} input={} (+ {} cached) output={}",
                    format_token_count(total),
                    format_token_count(input),
                    format_token_count(cached),
                    format_token_count(output),
                );
                cell.push(Line::from(Span::styled(usage_text, subtle_style())));
            }
            let resume_text = format!(
                "To continue this session, run orbcode resume {}",
                info.session_id
            );
            cell.push(Line::from(Span::styled(resume_text, subtle_style())));
            cell.push(Line::default());
        }

        cell
    }
}
