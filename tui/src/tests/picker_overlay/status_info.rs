use crate::tests::support::*;

#[test]
fn render_context_overview_uses_grid_and_legend_breakdown() {
    let overview = ContextOverview {
        max_thinking_tokens: None,
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            current_date: "2026-05-06".to_string(),
            ..Default::default()
        },
        report: ContextDiagnosticsReport::default(),
        usage: ContextUsageOverview {
            model: "gpt-5.4".to_string(),
            estimated_tokens: 98_000,
            token_source: ContextTokenSource::ProviderCountTokens,
            categories: orbcode_app_server::ContextCategoryBreakdown {
                system_prompt: 6_000,
                system_tools: 19_200,
                mcp_tools: 0,
                memory: 4_000,
                skills: 0,
                conversation: 68_800,
                attachments: 0,
                uncategorized: 0,
            },
            system_tools_tokens: 29_200,
            message_tokens: 68_800,
            context_window: 304_000,
            reserved_output_tokens: 20_000,
            reserved_buffer_tokens: 45_600,
            reserved_context_tokens: 45_600,
            free_space_tokens: 160_400,
            effective_context_window: 284_000,
            auto_compact_threshold: 258_400,
            warning_threshold: 238_400,
            error_threshold: 238_400,
            blocking_limit: 281_000,
            percent_left: 62,
            is_above_warning_threshold: false,
            is_above_error_threshold: false,
            is_above_auto_compact_threshold: false,
            is_at_blocking_limit: false,
        },
    };

    let rendered = render_context_overview(&overview, false);
    let lines = rendered.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 10);
    assert!(lines[0].contains("gpt-5.4 · 98k/304k tokens (32%)"));
    assert!(rendered.contains("◆ System prompt:       6k (2%)"));
    assert!(rendered.contains("○ System tools:     19.2k (6%)"));
    assert!(rendered.contains("✦ Memory:              4k (1%)"));
    assert!(rendered.contains("◉ Messages:         68.8k (23%)"));
    assert!(rendered.contains("· Free space:      160.4k (53%)"));
    assert!(rendered.contains("◎ Buffer:           45.6k (15%)"));
    // MCP, skills, attachments stay hidden when no tokens are reported yet.
    assert!(!rendered.contains("MCP tools"));
    assert!(!rendered.contains("Skills:"));
    assert!(!rendered.contains("Attachments:"));
}

#[test]
fn render_context_overview_lists_memory_source_diagnostics() {
    let overview = ContextOverview {
        max_thinking_tokens: None,
        context: TurnContext {
            cwd: "/tmp/project".to_string(),
            current_date: "2026-05-06".to_string(),
            memory_sources: vec![MemorySource {
                kind: MemorySourceKind::Project,
                label: "Project memory".to_string(),
                path: Some("/tmp/project/CLAUDE.md".to_string()),
                status: MemorySourceStatus::Skipped,
                writable: true,
                trust_boundary: Some("untrusted project".to_string()),
                scope: None,
                skipped_reason: Some(
                    "project memory skipped because project is not trusted".to_string(),
                ),
                content: None,
            }],
            ..Default::default()
        },
        report: ContextDiagnosticsReport::default(),
        usage: ContextUsageOverview {
            model: "gpt-5.4".to_string(),
            estimated_tokens: 1_000,
            token_source: ContextTokenSource::RoughEstimateFallback,
            categories: orbcode_app_server::ContextCategoryBreakdown::default(),
            system_tools_tokens: 0,
            message_tokens: 0,
            context_window: 100_000,
            reserved_output_tokens: 20_000,
            reserved_buffer_tokens: 0,
            reserved_context_tokens: 0,
            free_space_tokens: 99_000,
            effective_context_window: 80_000,
            auto_compact_threshold: 80_000,
            warning_threshold: 70_000,
            error_threshold: 70_000,
            blocking_limit: 90_000,
            percent_left: 99,
            is_above_warning_threshold: false,
            is_above_error_threshold: false,
            is_above_auto_compact_threshold: false,
            is_at_blocking_limit: false,
        },
    };

    let compact = render_context_overview(&overview, false);
    assert!(!compact.contains("Memory sources:"));
    assert!(!compact.contains("Project memory: skipped"));

    let rendered = render_context_overview(&overview, true);

    assert!(rendered.contains("Memory sources:"));
    assert!(rendered.contains("Project memory: skipped (/tmp/project/CLAUDE.md)"));
    assert!(rendered.contains("project memory skipped because project is not trusted"));
}

#[test]
fn render_usage_overview_summarizes_session_tokens() {
    let overview = UsageOverview {
        session_id: "12345678-90ab-cdef-1234-567890abcdef".to_string(),
        model: "gpt-5.4".to_string(),
        provider: ProviderId::OpenAi,
        message_count: 5,
        assistant_message_count: 2,
        usage_message_count: 2,
        total_usage: TokenUsage {
            input_tokens: 10_000,
            cache_creation_input_tokens: 2_000,
            cache_read_input_tokens: 3_000,
            output_tokens: 4_000,
            server_tool_use: orbcode_protocol::ServerToolUseUsage {
                web_search_requests: 2,
                web_fetch_requests: 1,
            },
            total_tokens: 19_000,
            ..TokenUsage::default()
        },
        cost: {
            let mut usage = std::collections::HashMap::new();
            usage.insert(
                "gpt-5.4".to_string(),
                ModelUsage {
                    input_tokens: 10_000,
                    output_tokens: 4_000,
                    cache_read_input_tokens: 3_000,
                    cache_creation_input_tokens: 2_000,
                    web_search_requests: 2,
                    cost_usd: 0.1234,
                    context_window: 128_000,
                    max_output_tokens: 4_096,
                    billing_basis: Default::default(),
                },
            );
            CostSummary {
                total_cost_usd: 0.1234,
                model_usage: usage,
                has_unknown_model_cost: true,
                billing_basis: Default::default(),
            }
        },
    };

    let rendered = render_usage_overview(&overview);

    assert!(rendered.contains("Usage:"));
    assert!(rendered.contains("session: 12345678"));
    assert!(rendered.contains("model: gpt-5.4"));
    assert!(rendered.contains("provider: openai"));
    assert!(rendered.contains("usage samples: 2"));
    assert!(rendered.contains("input: 10k"));
    assert!(rendered.contains("cache creation: 2k"));
    assert!(rendered.contains("cache read: 3k"));
    assert!(rendered.contains("output: 4k"));
    assert!(rendered.contains("total: 19k"));
    assert!(rendered.contains("web search requests: 2"));
    assert!(rendered.contains("web fetch requests: 1"));
    assert!(rendered.contains("Cost: $0.1234"));
    assert!(rendered.contains("may be inaccurate"));
    assert!(rendered.contains("gpt-5.4: 10000 input"));
}

#[test]
fn render_usage_overview_labels_subscription_usage_consistently() {
    let mut model_usage = std::collections::HashMap::new();
    model_usage.insert(
        "gpt-5.6-sol".to_string(),
        ModelUsage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            billing_basis: orbcode_app_server::BillingBasis::Subscription,
            ..ModelUsage::default()
        },
    );
    let overview = UsageOverview {
        session_id: "abcdef12-3456-7890-abcd-ef1234567890".to_string(),
        model: "gpt-5.6-sol".to_string(),
        provider: ProviderId::OpenAi,
        message_count: 2,
        assistant_message_count: 1,
        usage_message_count: 1,
        total_usage: TokenUsage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            total_tokens: 11_000,
            ..TokenUsage::default()
        },
        cost: CostSummary {
            model_usage,
            billing_basis: orbcode_app_server::BillingBasis::Subscription,
            ..CostSummary::default()
        },
    };

    let rendered = render_usage_overview(&overview);
    let model_line = rendered
        .lines()
        .find(|line| line.trim_start().starts_with("gpt-5.6-sol:"))
        .expect("subscription model line");
    assert!(rendered.contains("Cost: subscription (not API-priced)"));
    assert!(model_line.contains("10000 input"));
    assert!(model_line.contains("subscription (not API-priced)"));
    assert!(!rendered.contains("subscription; not API-priced"));
}

#[test]
fn render_usage_overview_warns_on_unknown_mixed_pricing() {
    let overview = UsageOverview {
        session_id: "abcdef12-3456-7890-abcd-ef1234567890".to_string(),
        model: "mixed-models".to_string(),
        provider: ProviderId::OpenAi,
        message_count: 2,
        assistant_message_count: 1,
        usage_message_count: 1,
        total_usage: TokenUsage::default(),
        cost: CostSummary {
            total_cost_usd: 0.25,
            has_unknown_model_cost: true,
            billing_basis: orbcode_app_server::BillingBasis::Mixed,
            ..CostSummary::default()
        },
    };

    let rendered = render_usage_overview(&overview);
    assert!(rendered.contains("Cost: $0.2500 API + subscription usage (not API-priced)"));
    assert!(rendered.contains("may be inaccurate due to unknown model pricing"));
}

#[test]
fn render_cost_overview_shows_total_and_per_model_breakdown() {
    let mut model_usage = std::collections::HashMap::new();
    model_usage.insert(
        "claude-sonnet-4-6".to_string(),
        ModelUsage {
            input_tokens: 110_000,
            output_tokens: 5_000,
            cache_read_input_tokens: 180_000,
            cache_creation_input_tokens: 15_000,
            web_search_requests: 1,
            cost_usd: 0.42,
            context_window: 200_000,
            max_output_tokens: 16_384,
            billing_basis: Default::default(),
        },
    );
    let overview = CostOverview {
        session_id: "12345678-90ab-cdef-1234-567890abcdef".to_string(),
        model: "Sonnet 4.6".to_string(),
        provider: ProviderId::Anthropic,
        cost: CostSummary {
            total_cost_usd: 0.42,
            model_usage,
            has_unknown_model_cost: false,
            billing_basis: Default::default(),
        },
    };

    let rendered = render_cost_overview(&overview);

    assert!(rendered.contains("Cost:"));
    assert!(rendered.contains("session: 12345678"));
    assert!(rendered.contains("model: Sonnet 4.6"));
    assert!(rendered.contains("provider: anthropic"));
    assert!(rendered.contains("total: $0.4200"));
    assert!(rendered.contains("By model:"));
    assert!(rendered.contains("claude-sonnet-4-6: $0.4200 (110000 input"));
    assert!(!rendered.contains("may be inaccurate"));
}

#[test]
fn render_cost_overview_warns_on_unknown_model_pricing() {
    let mut model_usage = std::collections::HashMap::new();
    model_usage.insert(
        "stub-model".to_string(),
        ModelUsage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            cost_usd: 0.075,
            ..ModelUsage::default()
        },
    );
    let overview = CostOverview {
        session_id: "abcdef12-3456-7890-abcd-ef1234567890".to_string(),
        model: "stub-model".to_string(),
        provider: ProviderId::Anthropic,
        cost: CostSummary {
            total_cost_usd: 0.075,
            model_usage,
            has_unknown_model_cost: true,
            billing_basis: Default::default(),
        },
    };

    let rendered = render_cost_overview(&overview);

    assert!(rendered.contains("total: $0.0750"));
    assert!(rendered.contains("may be inaccurate due to unknown model pricing"));
}

#[test]
fn render_cost_overview_warns_on_unknown_mixed_pricing() {
    let overview = CostOverview {
        session_id: "abcdef12-3456-7890-abcd-ef1234567890".to_string(),
        model: "mixed-models".to_string(),
        provider: ProviderId::OpenAi,
        cost: CostSummary {
            total_cost_usd: 0.25,
            has_unknown_model_cost: true,
            billing_basis: orbcode_app_server::BillingBasis::Mixed,
            ..CostSummary::default()
        },
    };

    let rendered = render_cost_overview(&overview);
    assert!(rendered.contains("total: $0.2500 API + subscription usage (not API-priced)"));
    assert!(rendered.contains("may be inaccurate due to unknown model pricing"));
}

#[test]
fn render_cost_overview_labels_subscription_usage() {
    let mut model_usage = std::collections::HashMap::new();
    model_usage.insert(
        "gpt-5.6-sol".to_string(),
        ModelUsage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            billing_basis: orbcode_app_server::BillingBasis::Subscription,
            ..ModelUsage::default()
        },
    );
    let overview = CostOverview {
        session_id: "abcdef12-3456-7890-abcd-ef1234567890".to_string(),
        model: "gpt-5.6-sol".to_string(),
        provider: ProviderId::OpenAi,
        cost: CostSummary {
            model_usage,
            billing_basis: orbcode_app_server::BillingBasis::Subscription,
            ..CostSummary::default()
        },
    };

    let rendered = render_cost_overview(&overview);
    assert!(rendered.contains("total: subscription (not API-priced)"));
    assert!(rendered.contains("gpt-5.6-sol: subscription (not API-priced)"));
    assert!(!rendered.contains("unknown model pricing"));
}

#[test]
fn render_cost_overview_handles_empty_session() {
    let overview = CostOverview {
        session_id: "00000000-0000-0000-0000-000000000000".to_string(),
        model: "Sonnet 4.6".to_string(),
        provider: ProviderId::Anthropic,
        cost: CostSummary::default(),
    };

    let rendered = render_cost_overview(&overview);

    assert!(rendered.contains("total: $0.0000"));
    assert!(rendered.contains("No provider token usage has been recorded"));
    assert!(!rendered.contains("By model:"));
}

#[test]
fn render_last_provider_request_snapshot_includes_bounded_provider_body() {
    let body_json = format!(
        "{{\n  \"model\": \"stub-model\",\n  \"messages\": [\n{}  ]\n}}",
        "    {\"role\":\"user\",\"content\":\"line\"},\n".repeat(2_000)
    );
    let snapshot = ProviderRequestDebugSnapshot {
        provider: ProviderId::Anthropic,
        source: "turn".to_string(),
        session_id: "session-last-request".to_string(),
        model: "stub-model".to_string(),
        base_url: "stub://anthropic".to_string(),
        captured_at: "2026-05-14T03:00:00Z".to_string(),
        recent_activity_json: "[]".to_string(),
        previous_turn_json: "[]".to_string(),
        body_json,
    };

    let (summary, detail) = render_last_provider_request_snapshot(&snapshot);

    assert_eq!(summary, "Recent LLM activity loaded.");
    assert!(detail.contains("● Provider request body"));
    assert!(detail.contains("provider: anthropic"));
    assert!(detail.contains("source: turn"));
    assert!(detail.contains("model: stub-model"));
    assert!(detail.contains("base_url: stub://anthropic"));
    assert!(detail.contains("\"messages\""));
    assert!(detail.contains("Provider request body truncated for interactive responsiveness"));
    assert!(detail.contains("● Recent activity"));
    assert!(detail.contains("No recent activity recorded."));
    assert!(
        detail.chars().count() < LAST_REQUEST_BODY_PREVIEW_CHARS + 1_000,
        "{detail}"
    );
    let tail = detail
        .split("middle characters.]\n\n")
        .nth(1)
        .expect("provider body preview tail");
    assert!(tail.starts_with("    {\"role\":\"user\""), "{tail}");
    assert!(!tail.lines().any(|line| line == "e"), "{tail}");
}

#[test]
fn render_recent_activity_trace_uses_flow_titles_and_pretty_json() {
    let activity = serde_json::json!([
        {
            "type": "assistant_response_from_llm",
            "label": "assistant response",
            "messages": [{
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": "I'll execute that bash command for you."
                    },
                    {
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "Bash",
                        "input": {
                            "command": "echo hello",
                            "description": "Print hello"
                        }
                    }
                ]
            }]
        },
        {
            "type": "tool_result_to_llm",
            "label": "tool result",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": "hello",
                    "is_error": false
                }]
            }]
        },
        {
            "type": "hook_notice_to_orbcode",
            "label": "Stop hook",
            "hook_event_name": "Stop",
            "message": "Done enough",
            "is_error": false
        },
        {
            "type": "hook_context_to_llm",
            "label": "PreToolUse hook context",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "PreToolUse hook context:\ninspect output carefully"
                }]
            }]
        }
    ]);

    let rendered = render_recent_activity_trace(&activity.to_string());

    assert!(rendered.contains("● LLM -> Orb Code"));
    assert!(rendered.contains("\"type\": \"assistant_response_from_llm\""));
    assert!(rendered.contains("\"text\": \"I'll execute that bash command for you.\""));
    assert!(rendered.contains("\"id\": \"call_1\""));
    assert!(rendered.contains("\"command\": \"echo hello\""));
    assert!(rendered.contains("● Bash -> Orb Code -> LLM"));
    assert!(rendered.contains("\"tool_use_id\": \"call_1\""));
    assert!(rendered.contains("\"content\": \"hello\""));
    assert!(rendered.contains("● Stop hook -> Orb Code"));
    assert!(rendered.contains("\"message\": \"Done enough\""));
    assert!(rendered.contains("● PreToolUse hook -> LLM"));
    assert!(rendered.contains("PreToolUse hook context:\\ninspect output carefully"));
}

#[test]
fn recent_activity_detail_renders_json_under_flow_title() {
    let lines = render_recent_activity_detail_lines(
        "● LLM -> Orb Code\n{\n  \"type\": \"assistant_response_from_llm\"\n}",
        80,
    );
    let rendered = plain_text_lines(&lines);

    assert_eq!(rendered[0], "   ● LLM -> Orb Code");
    assert_eq!(rendered[1], "    {");
    assert_eq!(
        rendered[2],
        "      \"type\": \"assistant_response_from_llm\""
    );
    assert_eq!(rendered[3], "    }");
    assert_eq!(lines[0].spans[0].style, subtle_style());
    assert_eq!(lines[2].spans[0].style, subtle_style());
    assert_eq!(lines[2].spans[1].style, inactive_style());
}

#[test]
fn recent_activity_detail_wraps_json_on_words() {
    let rendered = plain_text_lines(&render_recent_activity_detail_lines(
        "● LLM -> Orb Code\n  \"thinking\": \"The bash command executed successfully and returned denied as expected.\"",
        58,
    ));

    assert!(rendered.len() > 1);
    assert!(rendered[1].starts_with("      \"thinking\": \"The bash command"));
    assert!(rendered[2].starts_with("    "));
    assert!(
        !rendered
            .iter()
            .any(|line| line.trim_start().starts_with("xpected"))
    );
}

#[test]
fn render_stats_overview_shows_activity_heatmap() {
    let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let activity_days = (0..14)
        .map(|offset| StatsActivityDay {
            date: start + chrono::Duration::days(offset),
            message_count: match offset {
                0 => 0,
                1 => 1,
                2 => 3,
                3 => 6,
                _ => 10,
            },
        })
        .collect::<Vec<_>>();
    let overview = StatsOverview {
        window_days: 14,
        message_count: activity_days.iter().map(|day| day.message_count).sum(),
        activity_days,
    };

    let rendered = render_stats_overview(&overview);
    let lines = rendered.lines().collect::<Vec<_>>();

    assert_eq!(
        render_stats_summary(&overview),
        "Last 14 days · 110 messages."
    );
    assert_eq!(lines[0].trim(), "Jan");
    assert!(rendered.contains("Jan"));
    assert!(rendered.contains("Less ▪ ░ ▒ ▓ ■ More"));
    assert!(rendered.contains("▪"));
    assert!(lines.iter().any(|line| line.trim_start().starts_with("M")));
    assert!(rendered.contains("░"));
    assert!(rendered.contains("▒"));
    assert!(rendered.contains("▓"));
    assert!(rendered.contains("■"));
}

#[test]
fn render_status_overview_includes_session_and_runtime_summary() {
    let overview = StatusOverview {
        max_thinking_tokens: None,
        session_id: "12345678-90ab-cdef-1234-567890abcdef".to_string(),
        active_permission_preset: Some(ModelPermissionPreset::AskForApproval),
        cwd: PathBuf::from("/tmp/project"),
        home_dir: PathBuf::from("/tmp/home"),
        model_display_name: "test-model".to_string(),
        model_name: "test-model-id".to_string(),
        model_capabilities: vec!["thinking".to_string(), "interleaved_thinking".to_string()],
        small_fast_model_display_name: "fast-model".to_string(),
        effort_level: Some(EffortLevel::High),
        default_provider: ProviderId::Anthropic,
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: 2,
        sandbox_mode: "workspace-write".to_string(),
        sandbox_allow_network: false,
        permissions: PermissionOverview {
            permissions: orbcode_app_server_client::PermissionContext {
                cwd: PathBuf::from("/tmp/project"),
                allow_network: true,
                provider_allow_network: false,
                allow_tools: true,
                allowed_rules: Vec::new(),
                denied_rules: Vec::new(),
                ask_rules: Vec::new(),
                additional_directories: vec![PathBuf::from("/tmp/extra")],
            },
            effective_rules: Default::default(),
            settings_allowed_rules: Vec::new(),
            settings_denied_rules: Vec::new(),
            startup_allowed_rules: Vec::new(),
            startup_denied_rules: Vec::new(),
            edited_allowed_rules: Vec::new(),
            edited_denied_rules: Vec::new(),
            runtime_allowed_rules: Vec::new(),
            runtime_denied_rules: Vec::new(),
            configured_additional_directories: vec![PathBuf::from("/tmp/configured")],
            session_additional_directories: vec![PathBuf::from("/tmp/extra")],
        },
        auth: StatusAuthOverview {
            store_path: PathBuf::from("/tmp/home/auth.json"),
            entries: Vec::new(),
        },
        persisted_session_count: 3,
        background_job_count: 1,
        available_tool_count: 12,
        configured_mcp_server_count: 2,
        enabled_mcp_capability_count: 1,
        policy: orbcode_app_server::PolicyOverview {
            managed_origin: None,
            managed_paths: Vec::new(),
            available_models: None,
            allowed_mcp_servers: None,
            denied_mcp_servers: 0,
            allow_managed_hooks_only: false,
            allow_managed_permission_rules_only: false,
            allow_managed_mcp_servers_only: false,
            disable_bypass_permissions_mode: false,
            strict_plugin_only_customization: None,
            force_login_method: None,
            effective_model_source: None,
            conflicts: Vec::new(),
            settings_sources: Vec::new(),
        },
    };

    let rendered = render_status_overview(&overview);
    assert!(rendered.contains("permissions: Ask for approval"));

    assert!(rendered.contains("Status:"));
    assert!(rendered.contains("session: 12345678"));
    assert!(rendered.contains("model: test-model"));
    assert!(rendered.contains("model id: test-model-id"));
    assert!(rendered.contains("model capabilities: thinking,interleaved_thinking"));
    assert!(rendered.contains("small/fast model: fast-model"));
    assert!(rendered.contains("effort: high"));
    assert!(rendered.contains("provider: anthropic"));
    assert!(rendered.contains("fallback provider: openai"));
    assert!(rendered.contains("sandbox: workspace-write"));
    assert!(rendered.contains("sandbox network: disabled"));
    assert!(rendered.contains("tools permission: enabled"));
    assert!(rendered.contains("configured dirs: 1"));
    assert!(rendered.contains("session-only dirs: 1"));
    assert!(rendered.contains("MCP: 2 server(s), 1 enabled transport(s)"));
    assert!(rendered.contains("persisted sessions: 3"));
    assert!(rendered.contains("background jobs: 1"));
    assert!(rendered.contains("auth: no stored credentials"));
    assert!(rendered.contains("Managed policy:"));
    assert!(rendered.contains("source: none"));
    assert!(rendered.contains("Policy conflicts: none"));
}

#[test]
fn render_status_overview_shows_managed_policy_and_conflicts() {
    use orbcode_app_server::{PolicyConflictOverview, PolicyOverview, PolicySourceOverview};

    let overview = StatusOverview {
        max_thinking_tokens: None,
        session_id: "policy-session".to_string(),
        active_permission_preset: Some(ModelPermissionPreset::FullAccess),
        cwd: PathBuf::from("/tmp/project"),
        home_dir: PathBuf::from("/tmp/home"),
        model_display_name: "opus".to_string(),
        model_name: "claude-opus-4-7".to_string(),
        model_capabilities: Vec::new(),
        small_fast_model_display_name: "haiku".to_string(),
        effort_level: None,
        default_provider: ProviderId::Anthropic,
        fallback_provider: None,
        max_retries: 0,
        sandbox_mode: "danger-full-access".to_string(),
        sandbox_allow_network: true,
        permissions: PermissionOverview {
            permissions: orbcode_app_server_client::PermissionContext {
                cwd: PathBuf::from("/tmp/project"),
                allow_network: true,
                provider_allow_network: true,
                allow_tools: false,
                allowed_rules: Vec::new(),
                denied_rules: Vec::new(),
                ask_rules: Vec::new(),
                additional_directories: Vec::new(),
            },
            effective_rules: Default::default(),
            settings_allowed_rules: Vec::new(),
            settings_denied_rules: Vec::new(),
            startup_allowed_rules: Vec::new(),
            startup_denied_rules: Vec::new(),
            edited_allowed_rules: Vec::new(),
            edited_denied_rules: Vec::new(),
            runtime_allowed_rules: Vec::new(),
            runtime_denied_rules: Vec::new(),
            configured_additional_directories: Vec::new(),
            session_additional_directories: Vec::new(),
        },
        auth: StatusAuthOverview {
            store_path: PathBuf::from("/tmp/home/auth.json"),
            entries: Vec::new(),
        },
        persisted_session_count: 0,
        background_job_count: 0,
        available_tool_count: 0,
        configured_mcp_server_count: 0,
        enabled_mcp_capability_count: 0,
        policy: PolicyOverview {
            managed_origin: Some("file + drop-in".to_string()),
            managed_paths: vec![
                PathBuf::from("/etc/claude-code/managed-settings.json"),
                PathBuf::from("/etc/claude-code/managed-settings.d/10-security.json"),
            ],
            available_models: Some(vec!["opus".to_string(), "sonnet".to_string()]),
            allowed_mcp_servers: Some(2),
            denied_mcp_servers: 1,
            allow_managed_hooks_only: true,
            allow_managed_permission_rules_only: false,
            allow_managed_mcp_servers_only: true,
            disable_bypass_permissions_mode: true,
            strict_plugin_only_customization: Some("hooks, skills".to_string()),
            force_login_method: Some("console".to_string()),
            effective_model_source: Some("managed".to_string()),
            conflicts: vec![PolicyConflictOverview {
                source: "user".to_string(),
                source_path: PathBuf::from("/tmp/home/settings.json"),
                message: "user settings hooks are ignored because the active policy restricts hooks to managed settings (events: PreToolUse)".to_string(),
            }],
            settings_sources: vec![
                PolicySourceOverview {
                    source: "user".to_string(),
                    primary_path: PathBuf::from("/tmp/home/settings.json"),
                    present: true,
                    read_only: false,
                    error_count: 0,
                },
                PolicySourceOverview {
                    source: "managed".to_string(),
                    primary_path: PathBuf::from("/etc/claude-code/managed-settings.json"),
                    present: true,
                    read_only: true,
                    error_count: 0,
                },
            ],
        },
    };

    let rendered = render_status_overview(&overview);
    assert!(rendered.contains("permissions: Full Access"));

    assert!(rendered.contains("Managed policy:"));
    assert!(rendered.contains("source: file + drop-in"));
    assert!(rendered.contains("paths: 2"));
    assert!(rendered.contains("/etc/claude-code/managed-settings.json"));
    assert!(rendered.contains("/etc/claude-code/managed-settings.d/10-security.json"));
    assert!(rendered.contains("availableModels: [opus, sonnet]"));
    assert!(rendered.contains("allowManagedHooksOnly: true"));
    assert!(rendered.contains("allowManagedMcpServersOnly: true"));
    assert!(!rendered.contains("allowManagedPermissionRulesOnly"));
    assert!(rendered.contains("disableBypassPermissionsMode: disable"));
    assert!(rendered.contains("strictPluginOnlyCustomization: hooks, skills"));
    assert!(rendered.contains("forceLoginMethod: console"));
    assert!(rendered.contains("allowedMcpServers: 2"));
    assert!(rendered.contains("deniedMcpServers: 1"));
    assert!(rendered.contains("effective model source: managed"));
    assert!(rendered.contains("Settings sources:"));
    assert!(rendered.contains("- user (present, writable): /tmp/home/settings.json"));
    assert!(
        rendered.contains("- managed (present, read-only): /etc/claude-code/managed-settings.json")
    );
    assert!(rendered.contains("Policy conflicts: 1"));
    assert!(rendered.contains("[user] user settings hooks are ignored"));
}

#[test]
fn render_auth_overview_marks_blocked_oauth_status() {
    let rendered = render_auth_overview(&AuthOverview {
        store_path: PathBuf::from("/tmp/home/auth.json"),
        entries: vec![AuthStatusEntry {
            provider: ProviderId::Anthropic,
            method: AuthMethod::OAuthDevice,
            source_summary: "credentials:claude.ai oauth (expired)".to_string(),
            persisted: true,
            usable: false,
            active: false,
        }],
    });

    assert!(rendered.contains("credentials:claude.ai oauth (expired)"));
    assert!(rendered.contains("(persisted, blocked)"));
}

#[test]
fn render_auth_overview_marks_active_source() {
    let rendered = render_auth_overview(&AuthOverview {
        store_path: PathBuf::from("/tmp/home/auth.json"),
        entries: vec![
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::ApiKey,
                source_summary: "stored:sk-a***et".to_string(),
                persisted: true,
                usable: true,
                active: true,
            },
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::OAuthDevice,
                source_summary: "credentials:claude.ai oauth (ready)".to_string(),
                persisted: true,
                usable: true,
                active: false,
            },
        ],
    });

    assert!(rendered.contains("stored:sk-a***et (persisted) active"));
    assert!(rendered.contains("credentials:claude.ai oauth (ready) (persisted)"));
    assert!(!rendered.contains("credentials:claude.ai oauth (ready) (persisted) active"));
}

#[test]
fn render_memory_overview_includes_user_and_project_memory() {
    let overview = MemoryOverview {
        user_memory: orbcode_app_server::MemoryFileOverview {
            label: "User memory".to_string(),
            path: PathBuf::from("/tmp/home/CLAUDE.md"),
            exists: true,
            content: Some("User preference".to_string()),
            status: orbcode_protocol::MemorySourceStatus::Loaded,
            writable: true,
            trust_boundary: Some("private user".to_string()),
            scope: None,
            skipped_reason: None,
        },
        project_memories: vec![
            orbcode_app_server::MemoryFileOverview {
                label: "Project memory".to_string(),
                path: PathBuf::from("/tmp/project/CLAUDE.md"),
                exists: true,
                content: Some("Project instruction".to_string()),
                status: orbcode_protocol::MemorySourceStatus::Loaded,
                writable: true,
                trust_boundary: Some("trusted project".to_string()),
                scope: None,
                skipped_reason: None,
            },
            orbcode_app_server::MemoryFileOverview {
                label: "Project memory".to_string(),
                path: PathBuf::from("/tmp/project/nested/CLAUDE.md"),
                exists: true,
                content: None,
                status: orbcode_protocol::MemorySourceStatus::Empty,
                writable: true,
                trust_boundary: Some("trusted project".to_string()),
                scope: None,
                skipped_reason: None,
            },
        ],
        auto_memory_enabled: true,
        auto_memory_dir: PathBuf::from("/tmp/home/projects/project/memory"),
    };

    let rendered = render_memory_overview(&overview);

    assert!(rendered.contains("Memory:"));
    assert!(rendered.contains("path: /tmp/home/CLAUDE.md"));
    assert!(rendered.contains("status: loaded"));
    assert!(rendered.contains("User preference"));
    assert!(rendered.contains("Project memory:"));
    assert!(rendered.contains("path: /tmp/project/CLAUDE.md"));
    assert!(rendered.contains("Project instruction"));
    assert!(rendered.contains("path: /tmp/project/nested/CLAUDE.md"));
    assert!(rendered.contains("status: empty"));
}
