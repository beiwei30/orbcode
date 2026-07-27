use std::path::Path;

use ratatui::{
    prelude::Modifier,
    text::{Line, Span},
};

use crate::render::styled_wrap::transcript_content_width;
use crate::render::text_utils::{StyledLine, collapse_inline_whitespace, truncate_display_width};
use crate::tool_cell::ToolCell;
use crate::tool_cell::render::{black_circle_glyph, render_tool_cell_lines};
use crate::tui_theme::{inactive_style, subtle_style};

#[derive(Clone, Debug)]
pub(crate) struct AgentActivityGroup {
    pub(crate) agents: Vec<ToolCell>,
}

impl AgentActivityGroup {
    pub(crate) fn has_tool_use_id(&self, tool_use_id: &str) -> bool {
        self.agents
            .iter()
            .any(|agent| agent.tool_use_id == tool_use_id)
    }

    pub(crate) fn has_unresolved_tool_uses(&self) -> bool {
        self.agents.iter().any(|agent| agent.is_active)
    }
}

pub(crate) fn build_agent_activity_group(tool_cells: &[ToolCell]) -> Option<AgentActivityGroup> {
    if tool_cells.len() <= 1
        || !tool_cells
            .iter()
            .all(|cell| cell.tool_name.eq_ignore_ascii_case("agent"))
    {
        return None;
    }

    Some(AgentActivityGroup {
        agents: tool_cells.to_vec(),
    })
}

pub(crate) fn render_agent_activity_group_cell_lines(
    agents: &[ToolCell],
    expanded: bool,
    is_active: bool,
    blink_visible: bool,
    cwd: &Path,
    transcript_width: usize,
) -> Vec<StyledLine> {
    let mut lines = vec![agent_group_header_line(
        agents,
        expanded,
        is_active,
        blink_visible,
        transcript_width,
    )];

    if expanded {
        for (index, agent) in agents.iter().enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            let indicator_override = (agent.is_active && is_active).then(|| {
                (
                    if blink_visible {
                        black_circle_glyph().to_string()
                    } else {
                        " ".to_string()
                    },
                    inactive_style(),
                )
            });
            lines.extend(indent_styled_lines(
                render_tool_cell_lines(agent, true, indicator_override, transcript_width, cwd),
                "  ",
            ));
        }
        return lines;
    }

    let total = agents.len();
    for (index, agent) in agents.iter().enumerate() {
        let is_last = index + 1 == total;
        let branch = if is_last { "  └ " } else { "  ├ " };
        lines.push(Line::from(vec![
            Span::styled(branch.to_string(), subtle_style()),
            Span::styled(
                truncate_display_width(
                    &agent_summary_line(agent),
                    transcript_content_width(transcript_width).saturating_sub(4),
                ),
                inactive_style(),
            ),
        ]));

        if let Some(preview) = agent_preview_line(agent) {
            let prefix = if is_last { "    └ " } else { "  │ └ " };
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), subtle_style()),
                Span::styled(
                    truncate_display_width(
                        &preview,
                        transcript_content_width(transcript_width).saturating_sub(6),
                    ),
                    inactive_style().add_modifier(Modifier::DIM),
                ),
            ]));
        }
    }

    lines
}

fn agent_group_header_line(
    agents: &[ToolCell],
    expanded: bool,
    is_active: bool,
    blink_visible: bool,
    transcript_width: usize,
) -> StyledLine {
    let verb = if is_active { "Running" } else { "Ran" };
    let noun = agent_group_noun(agents);
    let hint = if expanded {
        "(ctrl+o to collapse)"
    } else {
        "(ctrl+o to expand)"
    };
    let summary = format!("{verb} {} {noun}... {hint}", agents.len());
    let indicator = if is_active {
        if blink_visible {
            black_circle_glyph().to_string()
        } else {
            " ".to_string()
        }
    } else {
        " ".to_string()
    };

    Line::from(vec![
        Span::styled(indicator, inactive_style()),
        Span::raw(" "),
        Span::styled(
            truncate_display_width(
                &summary,
                transcript_content_width(transcript_width).saturating_sub(2),
            ),
            inactive_style(),
        ),
    ])
}

fn agent_group_noun(agents: &[ToolCell]) -> String {
    let common_type = agents
        .iter()
        .filter_map(agent_type_label)
        .try_fold(None::<String>, |common, label| match common {
            Some(existing) if existing != label => None,
            Some(existing) => Some(Some(existing)),
            None => Some(Some(label)),
        })
        .flatten();

    match common_type.as_deref() {
        Some("Agent") | None => "agents".to_string(),
        Some(agent_type) => format!("{agent_type} agents"),
    }
}

fn agent_type_label(agent: &ToolCell) -> Option<String> {
    let open = agent.title.find('(')?;
    if !agent.title.ends_with(')') {
        return None;
    }
    Some(agent.title[..open].to_string())
}

fn agent_summary_line(agent: &ToolCell) -> String {
    let label = agent_description(agent);
    let Some(status) = agent_child_status(agent) else {
        return label;
    };
    format!("{label} · {status}")
}

fn agent_description(agent: &ToolCell) -> String {
    if let Some(open) = agent.title.find('(')
        && agent.title.ends_with(')')
    {
        return collapse_inline_whitespace(&agent.title[open + 1..agent.title.len() - 1]);
    }
    collapse_inline_whitespace(&agent.title)
}

fn agent_child_status(agent: &ToolCell) -> Option<String> {
    let status = normalize_agent_status(&agent.status_line);
    if status.is_empty() || matches!(status.as_str(), "Launching agent" | "Running Agent") {
        None
    } else {
        Some(status)
    }
}

fn agent_preview_line(agent: &ToolCell) -> Option<String> {
    let status = normalize_agent_status(&agent.status_line);
    agent
        .collapsed_preview_lines
        .iter()
        .rev()
        .map(|line| collapse_inline_whitespace(line))
        .find(|line| !line.is_empty() && normalize_agent_status(line) != status)
}

fn normalize_agent_status(status: &str) -> String {
    collapse_inline_whitespace(status)
        .trim()
        .trim_end_matches('.')
        .trim_end_matches('…')
        .to_string()
}

fn indent_styled_lines(lines: Vec<StyledLine>, prefix: &str) -> Vec<StyledLine> {
    lines
        .into_iter()
        .map(|line| {
            if line.spans.is_empty() {
                line
            } else {
                let mut spans = vec![Span::styled(prefix.to_string(), subtle_style())];
                spans.extend(line.spans);
                Line::from(spans)
            }
        })
        .collect()
}
