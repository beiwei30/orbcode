use std::fmt::Write as _;

use orbcode_app_server_client::ContextOverview;
use orbcode_protocol::TurnContext;

use super::format_context_tokens;

#[derive(Clone, Copy)]
struct ContextGridSegment {
    symbol: char,
    tokens: u32,
}

pub(crate) fn render_context_overview(overview: &ContextOverview, full: bool) -> String {
    let usage = &overview.usage;
    let window = usage.context_window.max(1);
    let categories = &usage.categories;
    let total = usage.estimated_tokens;

    let category_views = vec![
        ContextCategoryView {
            label: "System prompt:",
            symbol: '◆',
            tokens: categories.system_prompt,
            always_show: true,
        },
        ContextCategoryView {
            label: "System tools:",
            symbol: '○',
            tokens: categories.system_tools,
            always_show: true,
        },
        ContextCategoryView {
            label: "MCP tools:",
            symbol: '◌',
            tokens: categories.mcp_tools,
            always_show: false,
        },
        ContextCategoryView {
            label: "Memory:",
            symbol: '✦',
            tokens: categories.memory,
            always_show: false,
        },
        ContextCategoryView {
            label: "Skills:",
            symbol: '★',
            tokens: categories.skills,
            always_show: false,
        },
        ContextCategoryView {
            label: "Messages:",
            symbol: '◉',
            tokens: categories.conversation,
            always_show: true,
        },
        ContextCategoryView {
            label: "Attachments:",
            symbol: '▣',
            tokens: categories.attachments,
            always_show: false,
        },
        ContextCategoryView {
            label: "Other:",
            symbol: '?',
            tokens: categories.uncategorized,
            always_show: false,
        },
    ];

    let mut segments = Vec::with_capacity(category_views.len() + 2);
    for view in &category_views {
        segments.push(ContextGridSegment {
            symbol: view.symbol,
            tokens: view.tokens,
        });
    }
    let buffer = usage.reserved_context_tokens.min(window);
    let free = usage.free_space_tokens.min(window);
    segments.push(ContextGridSegment {
        symbol: '·',
        tokens: free,
    });
    segments.push(ContextGridSegment {
        symbol: '◎',
        tokens: buffer,
    });

    let grid = context_grid_rows(&segments, window);
    let percentage = percentage_of(total, window);

    let mut legend = Vec::with_capacity(category_views.len() + 6);
    legend.push(format!(
        "{} · {}/{} tokens ({}%)",
        usage.model,
        format_context_tokens(total),
        format_context_tokens(window),
        percentage
    ));
    legend.push(String::new());
    for view in &category_views {
        if view.tokens == 0 && !view.always_show {
            continue;
        }
        legend.push(context_legend_line(
            view.symbol,
            view.label,
            view.tokens,
            window,
        ));
    }
    legend.push(context_legend_line('·', "Free space:", free, window));
    legend.push(context_legend_line('◎', "Buffer:", buffer, window));

    let mut lines = grid
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let Some(legend_line) = legend.get(index).filter(|line| !line.is_empty()) else {
                return row;
            };
            format!("{row}   {legend_line}")
        })
        .collect::<Vec<_>>();
    if full {
        append_context_memory_sources(&mut lines, &overview.context);
        append_context_diagnostics(&mut lines, overview);
    }
    lines.join("\n")
}

fn append_context_diagnostics(lines: &mut Vec<String>, overview: &ContextOverview) {
    if overview.report.sections.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Context diagnostics:".to_string());
    for section in &overview.report.sections {
        lines.push(format!(
            "  {}: {} - {} ({} tokens)",
            section.category.label(),
            section.status.label(),
            section.summary,
            format_context_tokens(section.token_estimate)
        ));
        for detail in section.details.iter().take(4) {
            lines.push(format!("    {detail}"));
        }
    }
}

fn append_context_memory_sources(lines: &mut Vec<String>, context: &TurnContext) {
    if context.memory_sources.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Memory sources:".to_string());
    for source in &context.memory_sources {
        let path = source.path.as_deref().unwrap_or("(not configured)");
        let mut line = format!(
            "  {}: {} ({})",
            source.label,
            source.status.as_label(),
            path
        );
        if !source.writable {
            line.push_str(", read-only");
        }
        if let Some(scope) = source.scope.as_deref() {
            write!(line, ", scope: {scope}").expect("writing to String cannot fail");
        }
        if let Some(reason) = source.skipped_reason.as_deref() {
            write!(line, ", reason: {reason}").expect("writing to String cannot fail");
        }
        lines.push(line);
    }
}

struct ContextCategoryView {
    label: &'static str,
    symbol: char,
    tokens: u32,
    always_show: bool,
}

fn context_legend_line(symbol: char, label: &str, tokens: u32, window: u32) -> String {
    format!(
        "{symbol} {label:<15} {:>7} ({}%)",
        format_context_tokens(tokens),
        percentage_of(tokens, window)
    )
}

fn context_grid_rows(segments: &[ContextGridSegment], window: u32) -> Vec<String> {
    let symbols = context_grid_symbols(segments, window, 100);
    symbols
        .chunks(10)
        .map(|chunk| {
            chunk
                .iter()
                .map(char::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn context_grid_symbols(
    segments: &[ContextGridSegment],
    window: u32,
    total_cells: usize,
) -> Vec<char> {
    let window = window.max(1) as f64;
    let mut allocations = segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let exact = (segment.tokens as f64 / window) * total_cells as f64;
            (index, exact.floor() as usize, exact.fract())
        })
        .collect::<Vec<_>>();
    let mut allocated = allocations
        .iter()
        .map(|(_, cells, _)| *cells)
        .sum::<usize>();

    if allocated < total_cells {
        let mut by_remainder = allocations
            .iter()
            .enumerate()
            .map(|(allocation_index, (segment_index, _, remainder))| {
                (allocation_index, *segment_index, *remainder)
            })
            .collect::<Vec<_>>();
        by_remainder.sort_by(|left, right| {
            right
                .2
                .partial_cmp(&left.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        });
        for (allocation_index, _, _) in by_remainder
            .into_iter()
            .cycle()
            .take(total_cells.saturating_sub(allocated))
        {
            allocations[allocation_index].1 += 1;
        }
    } else if allocated > total_cells {
        while allocated > total_cells {
            let Some((_, cells, _)) = allocations.iter_mut().max_by_key(|(_, cells, _)| *cells)
            else {
                break;
            };
            if *cells == 0 {
                break;
            }
            *cells -= 1;
            allocated -= 1;
        }
    }

    let mut symbols = Vec::with_capacity(total_cells);
    for (index, cells, _) in allocations {
        symbols.extend(std::iter::repeat_n(segments[index].symbol, cells));
    }
    symbols.truncate(total_cells);
    while symbols.len() < total_cells {
        symbols.push('·');
    }
    symbols
}

fn percentage_of(tokens: u32, window: u32) -> u32 {
    if window == 0 {
        0
    } else {
        (((tokens as f64 / window as f64) * 100.0).round()) as u32
    }
}
