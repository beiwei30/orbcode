use chrono::{Datelike, NaiveDate};
use orbcode_app_server_client::{
    BillingBasis, CostOverview, DoctorCheck, DoctorReport, DoctorStatus, StatsActivityDay,
    StatsOverview, UsageOverview, format_cost,
};

use super::format_context_tokens;
use crate::commands::utils::short_session_id;

pub(crate) fn render_stats_overview(overview: &StatsOverview) -> String {
    let mut lines = Vec::new();
    lines.extend(render_activity_heatmap(&overview.activity_days));
    lines.push("     Less ▪ ░ ▒ ▓ ■ More".to_string());
    lines.join("\n")
}

pub(crate) fn render_stats_summary(overview: &StatsOverview) -> String {
    format!(
        "Last {} days · {} messages.",
        overview.window_days, overview.message_count
    )
}

fn render_activity_heatmap(days: &[StatsActivityDay]) -> Vec<String> {
    let Some(first_day) = days.first() else {
        return Vec::new();
    };
    let leading = weekday_index(first_day.date);
    let columns = (leading + days.len()).div_ceil(7).max(1);
    let percentiles = activity_percentiles(days);
    let mut cells = vec![vec![' '; columns]; 7];

    for (index, day) in days.iter().enumerate() {
        let slot = leading + index;
        let column = slot / 7;
        let row = slot % 7;
        cells[row][column] = activity_glyph(day.message_count, percentiles);
    }

    let mut lines = vec![activity_month_header(days, leading, columns)];
    let labels = ["", "M", "", "W", "", "F", ""];
    for row in 0..7 {
        let prefix = if labels[row].is_empty() {
            "     ".to_string()
        } else {
            format!("{:>3}  ", labels[row])
        };
        let body = cells[row]
            .iter()
            .map(|glyph| format!("{glyph} "))
            .collect::<String>()
            .trim_end()
            .to_string();
        lines.push(format!("{prefix}{body}"));
    }
    lines
}

fn activity_month_header(days: &[StatsActivityDay], leading: usize, columns: usize) -> String {
    let mut chars = vec![' '; columns * 2];
    let mut previous_month = None;
    let mut next_available = 0;
    for (index, day) in days.iter().enumerate() {
        let column = (leading + index) / 7;
        let month = day.date.month();
        if previous_month == Some(month) {
            continue;
        }
        previous_month = Some(month);
        let name = month_abbrev(month);
        let start = column * 2;
        if start >= next_available && start + name.len() <= chars.len() {
            for (offset, ch) in name.chars().enumerate() {
                chars[start + offset] = ch;
            }
            next_available = start + name.len() + 2;
        }
    }
    format!("     {}", chars.into_iter().collect::<String>().trim_end())
}

fn activity_percentiles(days: &[StatsActivityDay]) -> Option<(usize, usize, usize)> {
    let mut counts = days
        .iter()
        .map(|day| day.message_count)
        .filter(|count| *count > 0)
        .collect::<Vec<_>>();
    if counts.is_empty() {
        return None;
    }
    counts.sort_unstable();
    Some((
        counts[counts.len() * 25 / 100],
        counts[counts.len() * 50 / 100],
        counts[counts.len() * 75 / 100],
    ))
}

fn activity_glyph(count: usize, percentiles: Option<(usize, usize, usize)>) -> char {
    let Some((p25, p50, p75)) = percentiles else {
        return '▪';
    };
    if count == 0 {
        '▪'
    } else if count >= p75 {
        '■'
    } else if count >= p50 {
        '▓'
    } else if count >= p25 {
        '▒'
    } else {
        '░'
    }
}

fn weekday_index(date: NaiveDate) -> usize {
    date.weekday().num_days_from_sunday() as usize
}

fn month_abbrev(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "",
    }
}

pub(crate) fn render_usage_overview(overview: &UsageOverview) -> String {
    let usage = &overview.total_usage;
    let mut lines = vec![
        "Usage:".to_string(),
        format!("session: {}", short_session_id(&overview.session_id)),
        format!("model: {}", overview.model),
        format!("provider: {}", overview.provider),
        format!("messages: {}", overview.message_count),
        format!("assistant messages: {}", overview.assistant_message_count),
        format!("usage samples: {}", overview.usage_message_count),
    ];

    if overview.usage_message_count == 0 {
        lines.push(String::new());
        lines.push("No provider token usage has been recorded for this session yet.".to_string());
        return lines.join("\n");
    }

    lines.extend([
        String::new(),
        "Tokens:".to_string(),
        format!("  input: {}", format_context_tokens(usage.input_tokens)),
        format!(
            "  cache creation: {}",
            format_context_tokens(usage.cache_creation_input_tokens)
        ),
        format!(
            "  cache read: {}",
            format_context_tokens(usage.cache_read_input_tokens)
        ),
        format!("  output: {}", format_context_tokens(usage.output_tokens)),
        format!(
            "  total: {}",
            format_context_tokens(usage.component_total_tokens())
        ),
        String::new(),
        "Server tools:".to_string(),
        format!(
            "  web search requests: {}",
            usage.server_tool_use.web_search_requests
        ),
        format!(
            "  web fetch requests: {}",
            usage.server_tool_use.web_fetch_requests
        ),
    ]);

    let cost = &overview.cost;
    let cost_display = format_total_cost(
        cost.total_cost_usd,
        cost.billing_basis,
        cost.has_unknown_model_cost,
    );
    lines.extend([String::new(), format!("Cost: {cost_display}")]);

    if !cost.model_usage.is_empty() {
        let mut models: Vec<_> = cost.model_usage.iter().collect();
        models.sort_by_key(|(name, _)| *name);
        for (model, mu) in models {
            lines.push(format!("  {model}: {mu}"));
        }
    }

    lines.join("\n")
}

pub(crate) fn render_cost_overview(overview: &CostOverview) -> String {
    let cost = &overview.cost;
    let mut lines = vec![
        "Cost:".to_string(),
        format!("session: {}", short_session_id(&overview.session_id)),
        format!("model: {}", overview.model),
        format!("provider: {}", overview.provider),
    ];

    let total_display = format_total_cost(
        cost.total_cost_usd,
        cost.billing_basis,
        cost.has_unknown_model_cost,
    );
    lines.extend([String::new(), format!("total: {total_display}")]);

    if cost.model_usage.is_empty() {
        lines.push(String::new());
        lines.push("No provider token usage has been recorded for this session yet.".to_string());
        return lines.join("\n");
    }

    lines.push(String::new());
    lines.push("By model:".to_string());
    let mut models: Vec<_> = cost.model_usage.iter().collect();
    models.sort_by_key(|(name, _)| *name);
    for (model, mu) in models {
        let billed = match mu.billing_basis {
            BillingBasis::Api => format_cost(mu.cost_usd),
            BillingBasis::Subscription => "subscription (not API-priced)".to_string(),
            BillingBasis::Mixed => format!(
                "{} API + subscription usage (not API-priced)",
                format_cost(mu.cost_usd)
            ),
        };
        lines.push(format!(
            "  {model}: {billed} ({} input, {} output, {} cache read, {} cache write)",
            mu.input_tokens,
            mu.output_tokens,
            mu.cache_read_input_tokens,
            mu.cache_creation_input_tokens,
        ));
    }

    lines.join("\n")
}

fn format_total_cost(
    total_cost_usd: f64,
    billing_basis: BillingBasis,
    has_unknown_model_cost: bool,
) -> String {
    let mut display = match billing_basis {
        BillingBasis::Api => format_cost(total_cost_usd),
        BillingBasis::Subscription => "subscription (not API-priced)".to_string(),
        BillingBasis::Mixed => format!(
            "{} API + subscription usage (not API-priced)",
            format_cost(total_cost_usd)
        ),
    };
    if has_unknown_model_cost && billing_basis != BillingBasis::Subscription {
        display.push_str(" (may be inaccurate due to unknown model pricing)");
    }
    display
}

pub(crate) fn render_doctor_report(report: &DoctorReport) -> String {
    let (pass, warn, fail) = report.counts();
    let mut lines = vec![format!(
        "Doctor summary: pass={pass} warn={warn} fail={fail}"
    )];
    lines.push(String::new());
    for check in &report.checks {
        lines.push(render_doctor_check(check));
    }
    lines.join("\n")
}

fn render_doctor_check(check: &DoctorCheck) -> String {
    format!(
        "{:<4} {:<18} {}",
        doctor_status_label(check.status),
        check.name,
        check.detail.replace('\n', " ")
    )
}

fn doctor_status_label(status: DoctorStatus) -> &'static str {
    match status {
        DoctorStatus::Pass => "PASS",
        DoctorStatus::Fail => "FAIL",
        _ => "WARN",
    }
}
