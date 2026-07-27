use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use crate::history_cell::local_note::slash_command_feedback_for_line;
use crate::render::markdown::{
    parse_heading_line, parse_ordered_list_line, render_markdown_body_lines,
};
use crate::render::styled_wrap::{
    render_prefixed_wrapped_spans, transcript_content_width, wrap_styled_line,
};
use crate::render::text_utils::{
    StyledLine, compact_blank_lines, display_width_str, format_duration_short,
};
use crate::render::user::fill_user_bar_line;
use crate::slash_commands::SlashCommandDeferredFeedback;
use crate::tui_theme::{
    empty_transcript_placeholder_style, inactive_style, stats_heatmap_color, subtle_style,
    user_bar_style,
};

const SLASH_COMMAND_TIPS: [&str; 8] = [
    "Type / and a few letters to filter commands.",
    "Press Tab to complete slash commands and arguments.",
    "Use ↑↓ to move through slash command suggestions.",
    "Run /help to browse available commands.",
    "Use /allowed-tools as an alias for /permissions.",
    "Use /ctx as a shortcut for /context.",
    "Use /trace to inspect the latest LLM/tool/hook debug trace.",
    "Use /mcp tools <server> to inspect MCP tools.",
];

pub(crate) fn render_stats_panel_lines(detail: &str) -> Vec<StyledLine> {
    detail.lines().map(render_stats_panel_line).collect()
}

pub(crate) fn render_context_compacted_lines(
    duration_ms: Option<u64>,
    summary: Option<&str>,
    transcript_width: usize,
    expanded: bool,
) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    if expanded && let Some(duration_ms) = duration_ms {
        lines.push(Line::from(vec![
            Span::styled("✻", inactive_style()),
            Span::raw(" "),
            Span::styled(
                format!("Crunched for {}", format_duration_short(duration_ms)),
                inactive_style(),
            ),
        ]));
        lines.push(Line::default());
    }
    lines.push(Line::from(vec![
        Span::styled("✻", inactive_style()),
        Span::raw(" "),
        Span::styled("Conversation compacted", inactive_style()),
    ]));
    if expanded && let Some(summary) = summary.filter(|summary| !summary.trim().is_empty()) {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("⏺", inactive_style()),
            Span::raw(" "),
            Span::styled(
                "Compact summary",
                inactive_style().add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.extend(render_compact_summary_markdown_lines(
            summary,
            transcript_width.max(1),
        ));
    }
    lines
}

pub(crate) fn render_slash_command_output_lines(
    command: &str,
    summary: &str,
    detail_markdown: Option<&str>,
    deferred_feedback: SlashCommandDeferredFeedback,
    transcript_width: usize,
    show_tool_details: bool,
) -> Vec<StyledLine> {
    let width = transcript_width.max(1);
    let summary = if command == "/compact" && show_tool_details {
        summary.replace(
            "(ctrl+o to see full summary)",
            "(ctrl+o to hide full summary)",
        )
    } else {
        summary.to_string()
    };
    let mut lines = render_slash_command_prompt_lines(command, width);
    let show_summary_line = slash_command_feedback_for_line(command).show_summary;
    if show_summary_line {
        lines.extend(render_prefixed_wrapped_spans(
            vec![Span::styled(
                summary.clone(),
                slash_command_feedback_line_style(),
            )],
            "  └  ",
            "     ",
            subtle_style(),
            width,
        ));
    } else if let Some(tip) = slash_command_tip_for_line(command) {
        lines.extend(render_prefixed_wrapped_spans(
            vec![Span::styled(tip, slash_command_feedback_line_style())],
            "  └  ",
            "     ",
            subtle_style(),
            width,
        ));
    }

    let detail = match deferred_feedback {
        SlashCommandDeferredFeedback::Hidden => None,
        SlashCommandDeferredFeedback::Quoted | SlashCommandDeferredFeedback::Direct => {
            detail_markdown
                .filter(|detail| !detail.trim().is_empty())
                .map(|detail| {
                    compact_slash_detail_for_rendering(command, detail, show_tool_details)
                })
        }
    };
    if let Some(detail) = detail.as_deref().filter(|detail| !detail.trim().is_empty()) {
        if command == "/compact" {
            lines.extend(render_compact_slash_detail_lines(detail, width));
        } else if command == "/stats" {
            lines.push(Line::default());
            lines.extend(render_stats_panel_lines(detail));
        } else if deferred_feedback == SlashCommandDeferredFeedback::Direct {
            lines.push(Line::default());
            lines.extend(render_slash_command_direct_detail_lines(
                command, detail, width,
            ));
        } else {
            lines.push(Line::default());
            lines.extend(render_slash_command_detail_markdown_lines(detail, width));
        }
    }

    lines
}

fn compact_slash_detail_for_rendering(
    command: &str,
    detail: &str,
    show_tool_details: bool,
) -> String {
    if command != "/compact" || show_tool_details {
        return detail.to_string();
    }
    detail
        .split_once("\n\nFull summary:\n")
        .map_or_else(|| detail.to_string(), |(preview, _)| preview.to_string())
}

fn render_compact_summary_markdown_lines(summary: &str, width: usize) -> Vec<StyledLine> {
    let body_width = transcript_content_width(width).saturating_sub(5).max(1);
    let body_lines = render_markdown_body_lines(summary.trim(), inactive_style(), body_width);
    let mut saw_content = false;
    let mut rendered = Vec::new();

    for line in body_lines {
        if line.spans.is_empty() {
            rendered.push(Line::default());
            continue;
        }

        let prefix = if saw_content { "     " } else { "  └  " };
        saw_content = true;
        let mut spans = vec![Span::styled(prefix.to_string(), subtle_style())];
        spans.extend(line.spans);
        rendered.push(Line::from(spans));
    }

    compact_blank_lines(rendered)
}

fn render_slash_command_prompt_lines(command: &str, width: usize) -> Vec<StyledLine> {
    let content_width = transcript_content_width(width);
    let command = if command.trim().is_empty() {
        "/".to_string()
    } else if command.trim_start().starts_with('/') {
        command.trim().to_string()
    } else {
        format!("/{}", command.trim())
    };
    let wrapped = wrap_styled_line(
        &Line::from(vec![
            Span::styled("❯ ".to_string(), user_bar_style()),
            Span::styled(command, user_bar_style()),
        ]),
        content_width,
    );
    if wrapped.is_empty() {
        return vec![fill_user_bar_line(Line::default(), width)];
    }
    wrapped
        .into_iter()
        .map(|line| fill_user_bar_line(line, width))
        .collect()
}

fn render_slash_command_direct_detail_lines(
    command: &str,
    markdown: &str,
    width: usize,
) -> Vec<StyledLine> {
    if matches!(command, "/trace" | "/last-request" | "/llm-request") {
        return render_recent_activity_detail_lines(markdown, width);
    }

    let prefix = "   ";
    let body_width = transcript_content_width(width)
        .saturating_sub(display_width_str(prefix))
        .max(1);
    let markdown = strip_markdown_quote_prefixes(markdown);
    let body_lines = render_markdown_body_lines(markdown.trim(), inactive_style(), body_width);
    let mut lines = Vec::new();

    for line in body_lines {
        if line.spans.is_empty() {
            lines.push(Line::default());
            continue;
        }
        let mut spans = vec![Span::styled(prefix.to_string(), subtle_style())];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }

    compact_blank_lines(lines)
}

pub(crate) fn render_recent_activity_detail_lines(detail: &str, width: usize) -> Vec<StyledLine> {
    let mut lines = Vec::new();

    for raw_line in detail.lines() {
        if raw_line.trim().is_empty() {
            lines.push(Line::default());
            continue;
        }
        lines.extend(render_recent_activity_json_line(raw_line, width));
    }

    compact_blank_lines(lines)
}

fn render_recent_activity_json_line(line: &str, width: usize) -> Vec<StyledLine> {
    if let Some(title) = line.strip_prefix("● ") {
        return render_prefixed_wrapped_spans(
            vec![
                Span::styled("●", subtle_style()),
                Span::raw(" "),
                Span::styled(title.to_string(), inactive_style()),
            ],
            "   ",
            "   ",
            subtle_style(),
            width,
        );
    }

    render_prefixed_wrapped_spans(
        vec![Span::styled(line.to_string(), inactive_style())],
        "    ",
        "    ",
        subtle_style(),
        width,
    )
}

fn strip_markdown_quote_prefixes(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let indent_len = line.len().saturating_sub(trimmed.len());
            let indent = &line[..indent_len];
            if let Some(rest) = trimmed
                .strip_prefix("> ")
                .or_else(|| trimmed.strip_prefix('>').map(str::trim_start))
            {
                format!("{indent}{rest}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_slash_command_detail_markdown_lines(markdown: &str, width: usize) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    for raw_line in markdown.lines() {
        let trimmed = raw_line.trim_start();
        if trimmed.is_empty() {
            lines.push(Line::from(vec![Span::styled("     ▎ ", subtle_style())]));
            continue;
        }

        let content = if let Some(rest) = trimmed
            .strip_prefix("> ")
            .or_else(|| trimmed.strip_prefix('>').map(str::trim_start))
        {
            rest
        } else {
            raw_line
        };
        lines.extend(render_prefixed_wrapped_spans(
            render_slash_command_detail_line_spans(content),
            "     ▎ ",
            "     ▎ ",
            subtle_style(),
            width,
        ));
    }

    compact_blank_lines(lines)
}

fn render_compact_slash_detail_lines(markdown: &str, width: usize) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    for raw_line in markdown.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines.extend(render_prefixed_wrapped_spans(
            render_slash_command_detail_line_spans(trimmed),
            "  └  ",
            "     ",
            subtle_style(),
            width,
        ));
    }

    compact_blank_lines(lines)
}

fn render_slash_command_detail_line_spans(line: &str) -> Vec<Span<'static>> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len().saturating_sub(trimmed.len())];
    let mut spans = Vec::new();

    if !indent.is_empty() {
        spans.push(Span::styled(indent.to_string(), inactive_style()));
    }

    if let Some((_level, rest)) = parse_heading_line(trimmed) {
        spans.extend(render_inline_markdown_plain_spans(
            rest,
            inactive_style().add_modifier(Modifier::BOLD),
        ));
        return spans;
    }

    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        spans.push(Span::styled("• ", inactive_style()));
        spans.extend(render_inline_markdown_plain_spans(rest, inactive_style()));
        return spans;
    }

    if let Some((marker, rest)) = parse_ordered_list_line(trimmed) {
        spans.push(Span::styled(marker, inactive_style()));
        spans.extend(render_inline_markdown_plain_spans(rest, inactive_style()));
        return spans;
    }

    spans.extend(render_inline_markdown_plain_spans(
        trimmed,
        inactive_style(),
    ));
    spans
}

fn render_inline_markdown_plain_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chars = text.chars().peekable();
    let mut buffer = String::new();
    let mut bold = false;

    while let Some(ch) = chars.next() {
        if ch == '*' && chars.peek() == Some(&'*') {
            chars.next();
            if !buffer.is_empty() {
                spans.push(Span::styled(
                    buffer.clone(),
                    plain_inline_markdown_style(base_style, bold),
                ));
                buffer.clear();
            }
            bold = !bold;
            continue;
        }

        if ch == '`' {
            if !buffer.is_empty() {
                spans.push(Span::styled(
                    buffer.clone(),
                    plain_inline_markdown_style(base_style, bold),
                ));
                buffer.clear();
            }
            continue;
        }

        buffer.push(ch);
    }

    if !buffer.is_empty() {
        spans.push(Span::styled(
            buffer,
            plain_inline_markdown_style(base_style, bold),
        ));
    }

    if spans.is_empty() {
        vec![Span::styled(String::new(), inactive_style())]
    } else {
        spans
    }
}

fn plain_inline_markdown_style(base_style: Style, bold: bool) -> Style {
    if bold {
        base_style.add_modifier(Modifier::BOLD)
    } else {
        base_style
    }
}

fn render_stats_panel_line(line: &str) -> StyledLine {
    let mut spans = Vec::new();
    for ch in line.chars() {
        if is_stats_heatmap_cell(ch) {
            spans.push(Span::styled("■".to_string(), stats_heatmap_cell_style(ch)));
        } else {
            spans.push(Span::styled(ch.to_string(), stats_panel_text_style()));
        }
    }
    Line::from(spans)
}

fn is_stats_heatmap_cell(ch: char) -> bool {
    matches!(ch, '▪' | '░' | '▒' | '▓' | '■')
}

fn stats_heatmap_cell_index(ch: char) -> Option<usize> {
    match ch {
        '▪' => Some(0),
        '░' => Some(1),
        '▒' => Some(2),
        '▓' => Some(3),
        '■' => Some(4),
        _ => None,
    }
}

fn stats_panel_text_style() -> Style {
    empty_transcript_placeholder_style()
}

fn stats_heatmap_cell_style(ch: char) -> Style {
    let Some(index) = stats_heatmap_cell_index(ch) else {
        return stats_panel_text_style();
    };
    Style::default().fg(stats_heatmap_color(index))
}

pub(crate) fn slash_command_feedback_line_style() -> Style {
    subtle_style()
}

pub(crate) fn stable_slash_command_tip(value: &str) -> &'static str {
    let index = stable_tip_index(value, SLASH_COMMAND_TIPS.len());
    SLASH_COMMAND_TIPS[index]
}

pub(crate) fn slash_command_tip_for_line(command: &str) -> Option<String> {
    let command_name = command
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("");
    if command_name.is_empty() {
        return None;
    }
    match command_name {
        "help" | "?" => Some("Help: ↑↓ scroll, Esc close.".to_string()),
        _ => Some(format!("Tip: {}", stable_slash_command_tip(command_name))),
    }
}

fn stable_tip_index(value: &str, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    value.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as usize)
    }) % count
}
