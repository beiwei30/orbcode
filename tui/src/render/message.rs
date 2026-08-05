use std::path::Path;

use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use ratatui::{
    prelude::Style,
    text::{Line, Span},
};
use serde_json::Value;

use crate::history_cell::cells::is_plain_user_text_message;
use crate::history_cell::hook_note::parse_hook_transcript_note;
use crate::history_cell::local_note::parse_local_transcript_note;
use crate::render::assistant::{
    assistant_bullet, render_assistant_fallback_lines, render_assistant_markdown_lines,
    render_assistant_metadata_line,
};
use crate::render::hook::render_hook_transcript_note_lines;
use crate::render::local_note::render_local_transcript_note_lines;
use crate::render::styled_wrap::{
    render_prefixed_wrapped_spans, transcript_content_width, wrap_styled_line,
};
use crate::render::text_utils::{StyledLine, compact_blank_lines};
use crate::render::thinking::{
    render_collapsed_committed_thinking_lines, render_thinking_block_lines,
};
use crate::render::user::render_user_message_lines;
use crate::tool_cell::summary::{format_tool_card_status_line, format_tool_use_summary};
use crate::tui_theme::{active_palette, inactive_style, subtle_style};

pub(crate) fn render_message_lines(
    message: &TranscriptMessage,
    cwd: &Path,
    show_tool_details: bool,
    last_thinking_block: Option<&(String, usize)>,
    transcript_width: usize,
    model_display_name: &str,
    show_metadata: bool,
) -> Vec<StyledLine> {
    render_message_lines_with_hook_progress(
        message,
        cwd,
        show_tool_details,
        last_thinking_block,
        transcript_width,
        model_display_name,
        show_metadata,
        &[],
    )
}

pub(crate) fn render_message_lines_with_hook_progress(
    message: &TranscriptMessage,
    cwd: &Path,
    show_tool_details: bool,
    last_thinking_block: Option<&(String, usize)>,
    transcript_width: usize,
    model_display_name: &str,
    show_metadata: bool,
    hook_progress: &[Value],
) -> Vec<StyledLine> {
    if let Some(note) = parse_local_transcript_note(message) {
        return render_local_transcript_note_lines(note, transcript_width, show_tool_details);
    }

    if matches!(message.role, MessageRole::Assistant)
        && !message.blocks.is_empty()
        && message
            .blocks
            .iter()
            .all(|block| matches!(block, TranscriptBlock::Thinking { .. }))
    {
        let thinking_text = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                TranscriptBlock::Thinking { text, .. } if !text.trim().is_empty() => {
                    Some(text.trim().to_string())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if thinking_text.is_empty() {
            return Vec::new();
        }
        return if show_tool_details {
            render_thinking_block_lines(&thinking_text, transcript_width)
        } else {
            render_collapsed_committed_thinking_lines(&thinking_text, transcript_width)
        };
    }

    if let Some(note) = parse_hook_transcript_note(message) {
        return render_hook_transcript_note_lines(note, hook_progress, transcript_width);
    }

    if matches!(message.role, MessageRole::User) && is_plain_user_text_message(message) {
        return render_user_message_lines(message, transcript_width);
    }

    let blocks = if message.blocks.is_empty() {
        let legacy_blocks = parse_legacy_flattened_tool_blocks(&message.content);
        if !legacy_blocks.is_empty() {
            legacy_blocks
        } else if message.content.is_empty() {
            Vec::new()
        } else {
            vec![TranscriptBlock::Text {
                text: message.content.clone(),
            }]
        }
    } else {
        message
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                show_tool_details || !matches!(block, TranscriptBlock::Thinking { .. })
            })
            .filter(|(index, block)| match block {
                TranscriptBlock::Thinking { .. } => {
                    last_thinking_block.is_some_and(|(message_id, thinking_index)| {
                        message.id == *message_id && *index == *thinking_index
                    })
                }
                _ => true,
            })
            .map(|(_, block)| block)
            .cloned()
            .collect::<Vec<_>>()
    };

    if blocks.is_empty() {
        return if matches!(message.role, MessageRole::Assistant)
            && message
                .blocks
                .iter()
                .all(|block| matches!(block, TranscriptBlock::Thinking { .. }))
        {
            Vec::new()
        } else {
            render_assistant_fallback_lines(
                message,
                transcript_width,
                model_display_name,
                show_metadata,
            )
        };
    }

    let multi_block = blocks.len() > 1;
    let first_visible_text_index = if matches!(message.role, MessageRole::Assistant) {
        blocks.iter().position(|block| {
            matches!(
                block,
                TranscriptBlock::Text { text } if !text.trim().is_empty()
            )
        })
    } else {
        None
    };
    let mut lines = Vec::new();

    for (index, block) in blocks.into_iter().enumerate() {
        if Some(index) == first_visible_text_index {
            if index > 0 && !lines.is_empty() {
                lines.push(Line::default());
            }
            if show_metadata
                && let Some(metadata_line) =
                    render_assistant_metadata_line(message, transcript_width, model_display_name)
            {
                lines.push(metadata_line);
            }
        }
        lines.extend(render_block_lines(
            &message.role,
            &block,
            multi_block,
            cwd,
            show_tool_details,
            transcript_width,
        ));
    }

    compact_blank_lines(lines)
}

fn parse_legacy_flattened_tool_blocks(content: &str) -> Vec<TranscriptBlock> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if let Some(tool_use) = parse_legacy_flattened_tool_use(trimmed) {
        return vec![tool_use];
    }

    if let Some(tool_result) = parse_legacy_flattened_tool_result(trimmed) {
        return vec![tool_result];
    }

    Vec::new()
}

fn parse_legacy_flattened_tool_use(content: &str) -> Option<TranscriptBlock> {
    let remainder = content.strip_prefix("[tool_use ")?;
    let (name, tail) = remainder.split_once(']')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    Some(TranscriptBlock::ToolUse {
        id: format!("legacy-tool-use-{name}"),
        name: name.to_string(),
        input: tail.trim_start_matches('\n').to_string(),
    })
}

fn parse_legacy_flattened_tool_result(content: &str) -> Option<TranscriptBlock> {
    let (header, tail) = content.split_once(']')?;
    let header = header.strip_prefix("[tool_result ")?;
    let (is_error, tool_use_id) = if let Some(id) = header.strip_prefix("error ") {
        (true, id.trim())
    } else {
        (false, header.trim())
    };
    if tool_use_id.is_empty() {
        return None;
    }

    Some(TranscriptBlock::ToolResult {
        tool_use_id: tool_use_id.to_string(),
        content: tail.trim_start_matches('\n').to_string().into(),
        is_error,
        metadata: None,
    })
}

fn render_block_lines(
    role: &MessageRole,
    block: &TranscriptBlock,
    _multi_block: bool,
    cwd: &Path,
    show_tool_details: bool,
    transcript_width: usize,
) -> Vec<StyledLine> {
    if let TranscriptBlock::Text { text } = block {
        return render_text_block_lines(role, text, transcript_width);
    }

    let (_label, body, accent, text_style) = match block {
        TranscriptBlock::Text { .. } => unreachable!("text blocks are handled above"),
        TranscriptBlock::Thinking { text, .. } => {
            if !show_tool_details {
                return Vec::new();
            }
            return render_thinking_block_lines(text, transcript_width);
        }
        TranscriptBlock::ToolUse { name, input, .. } => (
            Some(format!("tool_use:{name}")),
            format_tool_use_summary(name, input, cwd),
            Span::styled("●", Style::default().fg(active_palette().tool)),
            inactive_style(),
        ),
        TranscriptBlock::ToolResult {
            content,
            is_error,
            metadata,
            ..
        } => (
            Some("tool_result".to_string()),
            format_tool_card_status_line(content, *is_error, metadata.as_deref()),
            if *is_error {
                Span::styled("●", Style::default().fg(active_palette().error))
            } else {
                Span::styled("●", Style::default().fg(active_palette().success))
            },
            if *is_error {
                Style::default().fg(active_palette().error)
            } else {
                inactive_style()
            },
        ),
        _ => return Vec::new(),
    };
    if body.trim().is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut body_lines = body.lines();

    if let Some(first) = body_lines.next() {
        let prefix_width = 2; // "● " = 2 display columns
        let content_width = transcript_content_width(transcript_width)
            .saturating_sub(prefix_width)
            .max(1);
        let wrapped = wrap_styled_line(
            &Line::from(vec![Span::styled(first.to_string(), text_style)]),
            content_width,
        );
        for (i, sub_line) in wrapped.into_iter().enumerate() {
            if i == 0 {
                let mut spans = vec![accent.clone(), Span::raw(" ")];
                spans.extend(sub_line.spans);
                lines.push(Line::from(spans));
            } else {
                let mut spans = vec![Span::styled("  ", subtle_style())];
                spans.extend(sub_line.spans);
                lines.push(Line::from(spans));
            }
        }
    } else {
        lines.push(Line::from(vec![accent]));
    }

    for line in body_lines {
        if line.trim().is_empty() {
            lines.push(Line::default());
        } else {
            lines.extend(render_prefixed_wrapped_spans(
                vec![Span::styled(line.to_string(), text_style)],
                "  └ ",
                "    ",
                subtle_style(),
                transcript_width,
            ));
        }
    }

    lines
}

pub(crate) fn render_text_block_lines(
    role: &MessageRole,
    text: &str,
    transcript_width: usize,
) -> Vec<StyledLine> {
    if matches!(role, MessageRole::Assistant) {
        return render_assistant_markdown_lines(
            text,
            inactive_style(),
            transcript_content_width(transcript_width)
                .saturating_sub(2)
                .max(1),
        );
    }

    let accent = assistant_bullet(role);
    let text_style = inactive_style();
    let mut lines = Vec::new();
    let mut body_lines = text.lines();

    if let Some(first) = body_lines.next() {
        let prefix_width = 2; // bullet + space = 2 display columns
        let content_width = transcript_content_width(transcript_width)
            .saturating_sub(prefix_width)
            .max(1);
        let wrapped = wrap_styled_line(
            &Line::from(vec![Span::styled(first.to_string(), text_style)]),
            content_width,
        );
        for (i, sub_line) in wrapped.into_iter().enumerate() {
            if i == 0 {
                let mut spans = vec![accent.clone(), Span::raw(" ")];
                spans.extend(sub_line.spans);
                lines.push(Line::from(spans));
            } else {
                let mut spans = vec![Span::styled("  ", subtle_style())];
                spans.extend(sub_line.spans);
                lines.push(Line::from(spans));
            }
        }
    } else {
        lines.push(Line::from(vec![accent]));
    }

    for line in body_lines {
        if line.trim().is_empty() {
            lines.push(Line::default());
        } else {
            lines.extend(render_prefixed_wrapped_spans(
                vec![Span::styled(line.to_string(), text_style)],
                "  ",
                "  ",
                subtle_style(),
                transcript_width,
            ));
        }
    }

    lines
}
