use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::{Duration as ChronoDuration, Local, NaiveDate};
use orbcode_config::AppConfig;
use orbcode_config::{
    COMPACT_MAX_OUTPUT_TOKENS, ContextWindowOptions, MANUAL_COMPACT_BUFFER_TOKENS,
    MaxOutputTokenOptions, TokenWarningOptions, auto_compact_threshold,
    calculate_token_warning_state, effective_context_window_size, resolve_context_window,
    resolve_max_output_tokens,
};
use orbcode_model_provider::ProviderRequest;
pub use orbcode_protocol::{
    ContextCategoryBreakdown, ContextDiagnosticCategory, ContextDiagnosticSection,
    ContextDiagnosticStatus, ContextDiagnosticsReport, ContextTokenSource, ContextUsageOverview,
    CostOverview, StatsActivityDay, StatsOverview, UsageOverview,
};
use orbcode_protocol::{MemorySourceStatus, MessageRole, ProviderId, SessionRecord, TokenUsage};

/// Per-source token breakdown used by `/context`.
///
/// Mirrors the categories surfaced by TypeScript's `analyzeContextUsage`.
/// `attachments` remains a reserved interface field until Rust has a
/// model-visible attachment transcript block.
pub(crate) fn build_context_usage_overview(
    model: String,
    estimated_tokens: u32,
    token_source: ContextTokenSource,
    mut categories: ContextCategoryBreakdown,
    context_options: &ContextWindowOptions,
    max_output_options: &MaxOutputTokenOptions,
    warning_options: &TokenWarningOptions,
) -> ContextUsageOverview {
    let categorized_tokens = categories.total();
    if estimated_tokens > categorized_tokens {
        categories.uncategorized = categories
            .uncategorized
            .saturating_add(estimated_tokens.saturating_sub(categorized_tokens));
    }
    let system_tools_tokens = categories.system_overhead().min(estimated_tokens);
    let message_tokens = estimated_tokens.saturating_sub(system_tools_tokens);
    let context_window = resolve_context_window(&model, context_options);
    let reserved_output_tokens =
        resolve_max_output_tokens(&model, max_output_options).min(COMPACT_MAX_OUTPUT_TOKENS);
    let effective_context_window =
        effective_context_window_size(&model, context_options, max_output_options);
    let auto_compact_threshold =
        auto_compact_threshold(&model, context_options, max_output_options, warning_options);
    let reserved_buffer_tokens = if warning_options.auto_compact_enabled {
        context_window.saturating_sub(auto_compact_threshold)
    } else {
        MANUAL_COMPACT_BUFFER_TOKENS
    };
    let reserved_context_tokens = reserved_buffer_tokens.min(context_window);
    let free_space_tokens = context_window
        .saturating_sub(estimated_tokens)
        .saturating_sub(reserved_context_tokens);
    let warning_state = calculate_token_warning_state(
        estimated_tokens,
        &model,
        context_options,
        max_output_options,
        warning_options,
    );

    ContextUsageOverview {
        model,
        estimated_tokens,
        token_source,
        categories,
        system_tools_tokens,
        message_tokens,
        context_window,
        reserved_output_tokens,
        reserved_buffer_tokens,
        reserved_context_tokens,
        free_space_tokens,
        effective_context_window,
        auto_compact_threshold,
        warning_threshold: warning_state.warning_threshold,
        error_threshold: warning_state.error_threshold,
        blocking_limit: warning_state.blocking_limit,
        percent_left: warning_state.percent_left,
        is_above_warning_threshold: warning_state.is_above_warning_threshold,
        is_above_error_threshold: warning_state.is_above_error_threshold,
        is_above_auto_compact_threshold: warning_state.is_above_auto_compact_threshold,
        is_at_blocking_limit: warning_state.is_at_blocking_limit,
    }
}

pub(crate) fn build_context_diagnostics_report(
    request: &ProviderRequest,
    config: &AppConfig,
    usage: &ContextUsageOverview,
    mcp_server_count: usize,
    mcp_capability_count: usize,
) -> ContextDiagnosticsReport {
    let categories = &usage.categories;
    let builtin_tools = request
        .tools
        .iter()
        .filter(|tool| !is_mcp_tool_name(&tool.name))
        .collect::<Vec<_>>();
    let mcp_tools = request
        .tools
        .iter()
        .filter(|tool| is_mcp_tool_name(&tool.name))
        .collect::<Vec<_>>();
    let context = &request.context;
    let mut sections = Vec::new();

    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::SystemPrompt,
        status: if request.system_prompt.trim().is_empty() {
            ContextDiagnosticStatus::Empty
        } else {
            ContextDiagnosticStatus::Loaded
        },
        summary: format!(
            "{} chars, {} line(s)",
            request.system_prompt.chars().count(),
            request.system_prompt.lines().count()
        ),
        details: vec![format!("model: {}", request.model)],
        token_estimate: categories.system_prompt,
    });

    let present_settings = config
        .settings_layers
        .layers
        .iter()
        .filter(|layer| layer.is_present())
        .count();
    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::Settings,
        status: if present_settings == 0 {
            ContextDiagnosticStatus::Empty
        } else {
            ContextDiagnosticStatus::Loaded
        },
        summary: format!(
            "{} source(s), {} policy conflict(s)",
            present_settings,
            config.policy_conflicts.len()
        ),
        details: vec![
            format!("provider: {}", config.default_provider),
            format!("sandbox: {:?}", config.sandbox_mode),
            format!("allow_tools: {}", config.allow_tools),
            format!(
                "append_system_prompt: {}",
                config.append_system_prompt.is_some()
            ),
        ],
        token_estimate: 0,
    });

    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::Tools,
        status: if builtin_tools.is_empty() {
            ContextDiagnosticStatus::Skipped
        } else {
            ContextDiagnosticStatus::Loaded
        },
        summary: format!("{} built-in tool schema(s)", builtin_tools.len()),
        details: builtin_tools
            .iter()
            .take(8)
            .map(|tool| tool.name.clone())
            .collect(),
        token_estimate: categories.system_tools,
    });

    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::Mcp,
        status: if mcp_server_count == 0 && mcp_tools.is_empty() {
            ContextDiagnosticStatus::Empty
        } else {
            ContextDiagnosticStatus::Loaded
        },
        summary: format!(
            "{} server(s), {} enabled transport(s), {} modeled tool(s)",
            mcp_server_count,
            mcp_capability_count,
            mcp_tools.len()
        ),
        details: mcp_tools
            .iter()
            .take(8)
            .map(|tool| tool.name.clone())
            .collect(),
        token_estimate: categories.mcp_tools,
    });

    let mut git_details = Vec::new();
    if let Some(root) = context.repo_root.as_deref() {
        git_details.push(format!("repo root: {root}"));
    }
    if let Some(branch) = context.git_branch.as_deref() {
        git_details.push(format!("branch: {branch}"));
    }
    if let Some(default_branch) = context.git_default_branch.as_deref() {
        git_details.push(format!("default branch: {default_branch}"));
    }
    if let Some(state) = context.git_worktree_state {
        git_details.push(format!("worktree: {}", state.as_label()));
    }
    if let Some(remote) = context.git_remote.as_deref() {
        git_details.push(format!("remote: {remote}"));
    }
    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::Git,
        status: if git_details.is_empty() {
            ContextDiagnosticStatus::Empty
        } else {
            ContextDiagnosticStatus::Loaded
        },
        summary: if context.repo_root.is_some() {
            "repository context available".to_string()
        } else {
            "not inside a git repository".to_string()
        },
        details: git_details,
        token_estimate: 0,
    });

    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::AddDir,
        status: if context.additional_directories.is_empty() {
            ContextDiagnosticStatus::Empty
        } else {
            ContextDiagnosticStatus::Configured
        },
        summary: format!(
            "{} configured, {} diagnosed",
            context.additional_directories.len(),
            context.additional_directory_details.len()
        ),
        details: context
            .additional_directory_details
            .iter()
            .map(|detail| {
                let mut line = detail.path.clone();
                if detail.has_claude_md {
                    line.push_str(" (CLAUDE.md)");
                }
                if let Some(branch) = detail.git_branch.as_deref() {
                    write!(line, " [branch: {branch}]").expect("writing to String cannot fail");
                }
                line
            })
            .collect(),
        token_estimate: 0,
    });

    let excluded_commands = config.settings_layers.sandbox_excluded_commands();
    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::Exclusions,
        status: if excluded_commands.is_empty() {
            ContextDiagnosticStatus::Empty
        } else {
            ContextDiagnosticStatus::Configured
        },
        summary: format!("{} sandbox command exclusion(s)", excluded_commands.len()),
        details: excluded_commands,
        token_estimate: 0,
    });

    let loaded_memory = context
        .memory_sources
        .iter()
        .filter(|source| source.status == MemorySourceStatus::Loaded)
        .count();
    let skipped_memory = context
        .memory_sources
        .iter()
        .filter(|source| source.status == MemorySourceStatus::Skipped)
        .count();
    sections.push(ContextDiagnosticSection {
        category: ContextDiagnosticCategory::Memory,
        status: if loaded_memory == 0 {
            ContextDiagnosticStatus::Empty
        } else {
            ContextDiagnosticStatus::Loaded
        },
        summary: format!("{loaded_memory} loaded, {skipped_memory} skipped"),
        details: context
            .memory_sources
            .iter()
            .map(|source| {
                let path = source.path.as_deref().unwrap_or("(not configured)");
                format!("{}: {} ({path})", source.label, source.status.as_label())
            })
            .collect(),
        token_estimate: categories.memory.saturating_add(categories.skills),
    });

    ContextDiagnosticsReport { sections }
}

fn is_mcp_tool_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("mcp__") else {
        return false;
    };
    let Some((server, tool)) = rest.split_once("__") else {
        return false;
    };
    !server.is_empty() && !tool.is_empty()
}

pub(crate) fn build_usage_overview(
    session: SessionRecord,
    model: String,
    api_model: &str,
    provider: ProviderId,
    context_window: u32,
    max_output_tokens: u32,
    billing_basis: crate::BillingBasis,
) -> UsageOverview {
    let mut total_usage = TokenUsage::default();
    let mut assistant_message_count = 0;
    let mut usage_message_count = 0;
    let mut cost_tracker = crate::CostTracker::new();

    for message in &session.messages {
        if matches!(message.role, MessageRole::Assistant) {
            assistant_message_count += 1;
        }
        if let Some(usage) = message.usage.clone() {
            usage_message_count += 1;
            let (usage_model, usage_billing_basis) = message.cost_attribution.as_ref().map_or(
                (api_model, billing_basis),
                |attribution| {
                    (
                        attribution.model.as_str(),
                        if attribution.subscription {
                            crate::BillingBasis::Subscription
                        } else {
                            crate::BillingBasis::Api
                        },
                    )
                },
            );
            match usage_billing_basis {
                crate::BillingBasis::Subscription => cost_tracker.add_subscription_usage(
                    usage_model,
                    &usage,
                    context_window,
                    max_output_tokens,
                ),
                crate::BillingBasis::Api | crate::BillingBasis::Mixed => {
                    cost_tracker.add_usage(usage_model, &usage, context_window, max_output_tokens);
                }
            }
            accumulate_token_usage(&mut total_usage, usage);
        }
    }
    total_usage.refresh_total_from_components();

    UsageOverview {
        session_id: session.session_id,
        model,
        provider,
        message_count: session.messages.len(),
        assistant_message_count,
        usage_message_count,
        total_usage,
        cost: cost_tracker.into_summary(),
    }
}

pub(crate) fn build_stats_overview(
    sessions: impl IntoIterator<Item = SessionRecord>,
    window_days: usize,
    end: NaiveDate,
) -> StatsOverview {
    let activity_days = build_activity_days(sessions, window_days, end);
    let message_count = activity_days.iter().map(|day| day.message_count).sum();

    StatsOverview {
        window_days: window_days.max(1),
        message_count,
        activity_days,
    }
}

pub(crate) fn build_activity_days(
    sessions: impl IntoIterator<Item = SessionRecord>,
    days: usize,
    end: NaiveDate,
) -> Vec<StatsActivityDay> {
    let days = days.max(1);
    let start = end - ChronoDuration::days(days.saturating_sub(1) as i64);
    let mut counts = (0..days)
        .map(|offset| {
            let date = start + ChronoDuration::days(offset as i64);
            (
                date,
                StatsActivityDay {
                    date,
                    message_count: 0,
                },
            )
        })
        .collect::<HashMap<_, _>>();

    for session in sessions {
        for message in session.messages {
            if let Some(day) =
                counts.get_mut(&message.created_at.with_timezone(&Local).date_naive())
            {
                day.message_count += 1;
            }
        }
    }

    (0..days)
        .filter_map(|offset| counts.remove(&(start + ChronoDuration::days(offset as i64))))
        .collect()
}

pub(crate) fn accumulate_token_usage(total: &mut TokenUsage, delta: TokenUsage) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(delta.cache_creation_input_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(delta.cache_read_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.server_tool_use.web_search_requests = total
        .server_tool_use
        .web_search_requests
        .saturating_add(delta.server_tool_use.web_search_requests);
    total.server_tool_use.web_fetch_requests = total
        .server_tool_use
        .web_fetch_requests
        .saturating_add(delta.server_tool_use.web_fetch_requests);
    total.service_tier = delta.service_tier;
    total.cache_creation.ephemeral_1h_input_tokens = total
        .cache_creation
        .ephemeral_1h_input_tokens
        .saturating_add(delta.cache_creation.ephemeral_1h_input_tokens);
    total.cache_creation.ephemeral_5m_input_tokens = total
        .cache_creation
        .ephemeral_5m_input_tokens
        .saturating_add(delta.cache_creation.ephemeral_5m_input_tokens);
    total.iterations = delta.iterations;
    total.speed = delta.speed;
    total.total_tokens = total.total_tokens.saturating_add(delta.total_tokens);
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Utc;
    use orbcode_protocol::{ServerToolUseUsage, TranscriptMessage};

    fn assistant_message_with_usage(usage: TokenUsage) -> TranscriptMessage {
        let mut msg = TranscriptMessage::new(MessageRole::Assistant, "response");
        msg.usage = Some(usage);
        msg
    }

    fn user_message() -> TranscriptMessage {
        TranscriptMessage::new(MessageRole::User, "hello")
    }

    fn session_with_messages(messages: Vec<TranscriptMessage>) -> SessionRecord {
        SessionRecord {
            session_id: "test-session".to_string(),
            title: None,
            custom_title: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            cwd: None,
            git_branch: None,
            model: None,
            provider: None,
            additional_directories: Vec::new(),
            session_allowed_tools: Vec::new(),
            session_disallowed_tools: Vec::new(),
            session_effort: None,
            goal: None,
            goal_transcript_records: Vec::new(),
            messages,
        }
    }

    #[test]
    fn build_usage_overview_computes_cost_from_transcript() {
        let usage1 = TokenUsage {
            input_tokens: 50_000,
            output_tokens: 2_000,
            cache_read_input_tokens: 100_000,
            cache_creation_input_tokens: 10_000,
            server_tool_use: ServerToolUseUsage {
                web_search_requests: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let usage2 = TokenUsage {
            input_tokens: 60_000,
            output_tokens: 3_000,
            cache_read_input_tokens: 80_000,
            cache_creation_input_tokens: 5_000,
            ..Default::default()
        };

        let session = session_with_messages(vec![
            user_message(),
            assistant_message_with_usage(usage1.clone()),
            user_message(),
            assistant_message_with_usage(usage2.clone()),
        ]);

        let overview = build_usage_overview(
            session,
            "Sonnet 4.6(anthropic)".to_string(),
            "claude-sonnet-4-6",
            ProviderId::Anthropic,
            200_000,
            16_384,
            crate::BillingBasis::Api,
        );

        assert_eq!(overview.message_count, 4);
        assert_eq!(overview.assistant_message_count, 2);
        assert_eq!(overview.usage_message_count, 2);

        assert_eq!(overview.total_usage.input_tokens, 110_000);
        assert_eq!(overview.total_usage.output_tokens, 5_000);
        assert_eq!(overview.total_usage.cache_read_input_tokens, 180_000);
        assert_eq!(overview.total_usage.cache_creation_input_tokens, 15_000);

        let cost = &overview.cost;
        assert!(!cost.has_unknown_model_cost);
        assert!(cost.total_cost_usd > 0.0, "cost should be positive");

        let expected_cost = (110_000.0 / 1e6) * 3.0
            + (5_000.0 / 1e6) * 15.0
            + (180_000.0 / 1e6) * 0.3
            + (15_000.0 / 1e6) * 3.75
            + 1.0 * 0.01;
        assert!(
            (cost.total_cost_usd - expected_cost).abs() < 1e-10,
            "expected {expected_cost}, got {}",
            cost.total_cost_usd
        );

        let model_usage = cost.model_usage.get("claude-sonnet-4-6").unwrap();
        assert_eq!(model_usage.input_tokens, 110_000);
        assert_eq!(model_usage.output_tokens, 5_000);
        assert_eq!(model_usage.context_window, 200_000);
        assert_eq!(model_usage.max_output_tokens, 16_384);
    }

    #[test]
    fn build_usage_overview_unknown_model_sets_flag() {
        let usage = TokenUsage {
            input_tokens: 1_000,
            output_tokens: 500,
            ..Default::default()
        };
        let session = session_with_messages(vec![assistant_message_with_usage(usage)]);

        let overview = build_usage_overview(
            session,
            "GPT-5".to_string(),
            "gpt-5-turbo",
            ProviderId::OpenAi,
            128_000,
            4_096,
            crate::BillingBasis::Api,
        );

        assert!(overview.cost.has_unknown_model_cost);
        assert!(overview.cost.total_cost_usd > 0.0);
    }

    #[test]
    fn build_usage_overview_marks_subscription_as_not_api_priced() {
        let usage = TokenUsage {
            input_tokens: 1_000,
            output_tokens: 500,
            ..Default::default()
        };
        let session = session_with_messages(vec![assistant_message_with_usage(usage)]);

        let overview = build_usage_overview(
            session,
            "gpt-5.6-sol".to_string(),
            "gpt-5.6-sol",
            ProviderId::OpenAi,
            272_000,
            128_000,
            crate::BillingBasis::Subscription,
        );

        assert_eq!(overview.cost.total_cost_usd, 0.0);
        assert!(!overview.cost.has_unknown_model_cost);
        assert_eq!(
            overview.cost.billing_basis,
            crate::BillingBasis::Subscription
        );
        assert_eq!(
            overview.cost.model_usage["gpt-5.6-sol"].billing_basis,
            crate::BillingBasis::Subscription
        );
    }

    #[test]
    fn build_usage_overview_empty_session_has_zero_cost() {
        let session = session_with_messages(vec![]);
        let overview = build_usage_overview(
            session,
            "Sonnet 4.6".to_string(),
            "claude-sonnet-4-6",
            ProviderId::Anthropic,
            200_000,
            16_384,
            crate::BillingBasis::Api,
        );

        assert_eq!(overview.cost.total_cost_usd, 0.0);
        assert!(overview.cost.model_usage.is_empty());
        assert!(!overview.cost.has_unknown_model_cost);
    }
}
