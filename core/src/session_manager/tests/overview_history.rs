use super::support::*;
use super::*;
use crate::ContextDiagnosticCategory;
use orbcode_config::{SettingsLayer, SettingsSource};
use serde_json::{Map, json};

#[tokio::test]
async fn pre_user_instructions_preview_includes_context_and_tools() {
    let mut manager = test_manager().await;
    manager.config.allow_tools = true;
    tokio::fs::write(manager.config.cwd.join("CLAUDE.md"), "Use project rules.")
        .await
        .expect("write claude md");

    let preview = manager.pre_user_instructions_preview("session-id").await;

    assert!(preview.contains("# System prompt"));
    assert!(preview.contains("You are Orb Code, a terminal coding assistant"));
    assert!(preview.contains("# Context message"));
    assert!(preview.contains("# claudeMd"));
    assert!(preview.contains("Use project rules."));
    assert!(preview.contains("# Tools"));
    assert!(preview.contains("## Bash"));
}

#[tokio::test]
async fn context_usage_overview_reports_glm_4_7_window_math() {
    let mut manager = test_manager().await;
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_MODEL".to_string(), "glm-4.7".to_string());
    let context = TurnContext {
        cwd: "/repo".to_string(),
        current_date: "2026-05-06".to_string(),
        ..Default::default()
    };

    let overview = manager
        .context_usage_overview("new-session", context)
        .await
        .expect("context usage overview");

    assert_eq!(overview.model, "glm-4.7");
    assert_eq!(overview.context_window, 200_000);
    assert_eq!(overview.reserved_output_tokens, 20_000);
    assert_eq!(overview.effective_context_window, 180_000);
    assert_eq!(overview.blocking_limit, 177_000);
    assert_eq!(
        overview.token_source,
        ContextTokenSource::RoughEstimateFallback
    );
}

#[tokio::test]
async fn context_usage_overview_populates_category_breakdown() {
    let mut manager = test_manager().await;
    manager.config.allow_tools = true;
    let memory_body = "Always use the project's testing harness.";
    let context = TurnContext {
        cwd: "/repo".to_string(),
        repo_root: Some("/repo".to_string()),
        current_date: "2026-05-25".to_string(),
        git_branch: Some("token".to_string()),
        claude_md: Some(memory_body.to_string()),
        ..Default::default()
    };

    let overview = manager
        .context_usage_overview("new-session", context)
        .await
        .expect("context usage overview");

    let categories = overview.categories;
    assert!(categories.system_prompt > 0, "system prompt counted");
    assert!(categories.system_tools > 0, "built-in tools counted");
    assert!(categories.memory > 0, "claude.md counted as memory");
    assert_eq!(categories.conversation, 0);
    assert_eq!(categories.mcp_tools, 0);
    assert_eq!(categories.skills, 0);
    assert_eq!(categories.attachments, 0);
    assert_eq!(categories.uncategorized, 0);
    assert_eq!(
        overview.system_tools_tokens,
        categories.system_overhead().min(overview.estimated_tokens)
    );
}

#[tokio::test]
async fn context_usage_and_diagnostics_reports_context_sources() {
    let mut manager = test_manager().await;
    manager.config.allow_tools = true;
    let mut raw_settings = Map::new();
    raw_settings.insert(
        "sandbox".to_string(),
        json!({"excludedCommands": ["npm run test:*"]}),
    );
    let local_settings_path = manager.config.cwd.join(".claude/settings.local.json");
    manager.config.settings_layers.layers.push(SettingsLayer {
        source: SettingsSource::Local,
        primary_path: local_settings_path.clone(),
        contributing_paths: vec![local_settings_path],
        raw: Some(raw_settings),
        errors: Vec::new(),
    });
    let extra_dir = manager.config.cwd.join("extra");
    manager
        .config
        .additional_directories
        .push(extra_dir.clone());
    tokio::fs::create_dir(&extra_dir)
        .await
        .expect("create extra dir");
    tokio::fs::write(manager.config.cwd.join("CLAUDE.md"), "Use project rules.")
        .await
        .expect("write memory");
    let mut context = manager.context_preview().await;
    context.repo_root = Some(manager.config.cwd.display().to_string());
    context.git_branch = Some("context-report".to_string());
    let (usage, report) = manager
        .context_usage_and_diagnostics("new-session", context)
        .await
        .expect("context report");

    assert!(usage.categories.system_tools > 0);
    assert!(usage.free_space_tokens > 0);
    assert_eq!(usage.reserved_context_tokens, usage.reserved_buffer_tokens);
    let categories = report
        .sections
        .iter()
        .map(|section| section.category)
        .collect::<Vec<_>>();
    assert!(categories.contains(&ContextDiagnosticCategory::SystemPrompt));
    assert!(categories.contains(&ContextDiagnosticCategory::Settings));
    assert!(categories.contains(&ContextDiagnosticCategory::Tools));
    assert!(categories.contains(&ContextDiagnosticCategory::Mcp));
    assert!(categories.contains(&ContextDiagnosticCategory::Git));
    assert!(categories.contains(&ContextDiagnosticCategory::AddDir));
    assert!(categories.contains(&ContextDiagnosticCategory::Exclusions));
    assert!(categories.contains(&ContextDiagnosticCategory::Memory));

    let memory = report
        .sections
        .iter()
        .find(|section| section.category == ContextDiagnosticCategory::Memory)
        .expect("memory diagnostics");
    assert!(memory.summary.contains("loaded"));
}

#[tokio::test]
async fn count_tokens_request_uses_small_fast_model() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_SMALL_FAST_MODEL".to_string(),
        "claude-fast-count[1m]".to_string(),
    );
    let context = TurnContext {
        cwd: "/repo".to_string(),
        current_date: "2026-05-22".to_string(),
        ..Default::default()
    };
    let request = manager
        .provider_request_for_messages("new-session", "", context, Vec::new(), true, true)
        .await;

    let count_request = manager.count_tokens_request_for_provider(
        ProviderId::Anthropic,
        &request,
        &manager.effective_config(),
    );

    assert_eq!(request.model, "stub-model");
    assert_eq!(count_request.model, "claude-fast-count");
}

#[tokio::test]
async fn count_tokens_cache_suppresses_duplicate_network_calls() {
    let (base_url, request_count, shutdown_tx, server_handle) =
        start_counting_count_tokens_server();
    let mut manager = test_manager().await;
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_BASE_URL".to_string(), base_url);
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_API_KEY".to_string(), "test-api-key".to_string());

    let context = TurnContext {
        cwd: "/repo".to_string(),
        current_date: "2026-05-22".to_string(),
        ..Default::default()
    };
    let request = manager
        .provider_request_for_messages("cache-session", "", context, Vec::new(), true, true)
        .await;
    let count_request = manager.count_tokens_request_for_provider(
        ProviderId::Anthropic,
        &request,
        &manager.effective_config(),
    );

    let first = manager
        .count_tokens_cached(ProviderId::Anthropic, &count_request)
        .await
        .expect("first count-tokens call");
    let second = manager
        .count_tokens_cached(ProviderId::Anthropic, &count_request)
        .await
        .expect("second count-tokens call");

    assert_eq!(first, Some(42));
    assert_eq!(second, Some(42));
    assert_eq!(
        request_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "second call should be served from cache"
    );
    assert_eq!(manager.count_tokens_cache.misses(), 1);
    assert_eq!(manager.count_tokens_cache.hits(), 1);

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();
}

#[tokio::test]
async fn loads_prompt_history_from_claude_home() {
    let manager = test_manager().await;
    let entry = json!({
        "display": "older",
        "project": manager.config.cwd.display().to_string(),
        "timestamp": 1,
    });
    tokio::fs::write(
        &manager.config.history_path,
        format!(
            "{}\n",
            serde_json::to_string(&entry).expect("serialize entry")
        ),
    )
    .await
    .expect("write history");

    let history = manager.prompt_history(5).await.expect("load history");
    assert_eq!(history, vec!["older"]);
}

#[tokio::test]
async fn prompt_history_for_session_orders_current_session_first() {
    let manager = test_manager().await;
    let project = manager.config.cwd.display().to_string();
    let lines = [
        json!({ "display": "session-a-old", "project": project, "sessionId": "session-a", "timestamp": 1 }),
        json!({ "display": "session-b-old", "project": project, "sessionId": "session-b", "timestamp": 2 }),
        json!({ "display": "session-a-new", "project": project, "sessionId": "session-a", "timestamp": 3 }),
    ]
    .map(|value| serde_json::to_string(&value).expect("serialize"))
    .join("\n");
    tokio::fs::write(&manager.config.history_path, format!("{lines}\n"))
        .await
        .expect("write history");

    let history = manager
        .prompt_history_for_session("session-a", 5)
        .await
        .expect("load history");
    assert_eq!(
        history,
        vec![
            "session-a-new".to_string(),
            "session-a-old".to_string(),
            "session-b-old".to_string(),
        ]
    );
}

#[tokio::test]
async fn remove_last_prompt_history_entry_filters_subsequent_loads() {
    let manager = test_manager().await;
    manager
        .prompt_history_store()
        .append("session-r", "first")
        .await
        .expect("append first");
    manager
        .prompt_history_store()
        .append("session-r", "second")
        .await
        .expect("append second");

    manager.remove_last_prompt_history_entry();

    let history = manager
        .prompt_history_for_session("session-r", 5)
        .await
        .expect("load history");
    assert_eq!(history, vec!["first".to_string()]);
}

#[tokio::test]
async fn usage_overview_sums_session_assistant_usage() {
    let manager = test_manager().await;
    let session_id = "usage-overview-session";
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "hello"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "first").with_usage(TokenUsage {
                input_tokens: 10,
                cache_creation_input_tokens: 2,
                output_tokens: 3,
                total_tokens: 15,
                ..TokenUsage::default()
            }),
        )
        .await
        .expect("append first assistant");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "second").with_usage(TokenUsage {
                input_tokens: 5,
                cache_read_input_tokens: 4,
                output_tokens: 1,
                server_tool_use: orbcode_protocol::ServerToolUseUsage {
                    web_search_requests: 2,
                    web_fetch_requests: 1,
                },
                total_tokens: 10,
                ..TokenUsage::default()
            }),
        )
        .await
        .expect("append second assistant");

    let overview = manager
        .usage_overview(session_id)
        .await
        .expect("usage overview");

    assert_eq!(overview.session_id, session_id);
    assert_eq!(overview.message_count, 3);
    assert_eq!(overview.assistant_message_count, 2);
    assert_eq!(overview.usage_message_count, 2);
    assert_eq!(overview.total_usage.input_tokens, 15);
    assert_eq!(overview.total_usage.cache_creation_input_tokens, 2);
    assert_eq!(overview.total_usage.cache_read_input_tokens, 4);
    assert_eq!(overview.total_usage.output_tokens, 4);
    assert_eq!(overview.total_usage.component_total_tokens(), 25);
    assert_eq!(overview.total_usage.server_tool_use.web_search_requests, 2);
    assert_eq!(overview.total_usage.server_tool_use.web_fetch_requests, 1);
}

fn assistant_usage(
    input: u32,
    output: u32,
    cache_read: u32,
    cache_creation: u32,
    web_search: u32,
) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
        server_tool_use: orbcode_protocol::ServerToolUseUsage {
            web_search_requests: web_search,
            ..Default::default()
        },
        ..TokenUsage::default()
    }
}

/// Builds a second `SessionManager` over the same on-disk session/project
/// directories as `manager`, simulating a fresh process resuming the session.
async fn resumed_manager(manager: &SessionManager) -> SessionManager {
    let config = manager.config.clone();
    let mcp = orbcode_mcp::McpRegistry::load(config.home_dir.clone(), config.home_dir.clone())
        .await
        .expect("create mcp registry");
    SessionManager::new(config, orbcode_tools::ToolRegistry::foundation(), mcp)
}

#[tokio::test]
async fn cost_overview_live_matches_transcript_recompute() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "cost-live-session";

    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "hello"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "first")
                .with_usage(assistant_usage(50_000, 2_000, 100_000, 10_000, 1)),
        )
        .await
        .expect("append first assistant");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "again"),
        )
        .await
        .expect("append second user");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "second")
                .with_usage(assistant_usage(60_000, 3_000, 80_000, 5_000, 0)),
        )
        .await
        .expect("append second assistant");

    let live = manager
        .cost_overview(session_id)
        .await
        .expect("cost overview");
    let transcript = manager
        .usage_overview(session_id)
        .await
        .expect("usage overview");

    // Live accumulation must reproduce a fresh transcript recompute, including
    // cache read/write and web-search billing.
    assert!(
        (live.cost.total_cost_usd - transcript.cost.total_cost_usd).abs() < 1e-12,
        "live {} != transcript {}",
        live.cost.total_cost_usd,
        transcript.cost.total_cost_usd
    );
    assert!(!live.cost.has_unknown_model_cost);

    // And it must equal the sum of the per-turn costs computed independently.
    let (cost1, _) = crate::calculate_usd_cost(
        "claude-sonnet-4-6",
        &assistant_usage(50_000, 2_000, 100_000, 10_000, 1),
    );
    let (cost2, _) = crate::calculate_usd_cost(
        "claude-sonnet-4-6",
        &assistant_usage(60_000, 3_000, 80_000, 5_000, 0),
    );
    assert!(
        (live.cost.total_cost_usd - (cost1 + cost2)).abs() < 1e-12,
        "live total {} != per-turn sum {}",
        live.cost.total_cost_usd,
        cost1 + cost2
    );
}

#[tokio::test]
async fn cost_overview_restores_across_resume_without_double_counting() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "cost-resume-session";

    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "hello"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "first")
                .with_usage(assistant_usage(50_000, 2_000, 100_000, 10_000, 1)),
        )
        .await
        .expect("append first assistant");

    let before_resume = manager
        .cost_overview(session_id)
        .await
        .expect("cost overview")
        .cost
        .total_cost_usd;
    assert!(before_resume > 0.0);

    // Resume in a fresh manager: historical cost is restored from the
    // persisted transcript.
    let resumed = resumed_manager(&manager).await;
    let restored = resumed
        .cost_overview(session_id)
        .await
        .expect("resumed cost overview")
        .cost
        .total_cost_usd;
    assert!(
        (restored - before_resume).abs() < 1e-12,
        "resume should restore historical cost: {restored} != {before_resume}"
    );

    // A further turn accumulates on top of the restored total without
    // re-counting the seeded history.
    resumed
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "second")
                .with_usage(assistant_usage(60_000, 3_000, 80_000, 5_000, 0)),
        )
        .await
        .expect("append after resume");

    let after = resumed
        .cost_overview(session_id)
        .await
        .expect("cost overview");
    let transcript = resumed
        .usage_overview(session_id)
        .await
        .expect("usage overview");
    assert!(
        (after.cost.total_cost_usd - transcript.cost.total_cost_usd).abs() < 1e-12,
        "post-resume live {} != transcript {} (double count?)",
        after.cost.total_cost_usd,
        transcript.cost.total_cost_usd
    );

    let (cost1, _) = crate::calculate_usd_cost(
        "claude-sonnet-4-6",
        &assistant_usage(50_000, 2_000, 100_000, 10_000, 1),
    );
    let (cost2, _) = crate::calculate_usd_cost(
        "claude-sonnet-4-6",
        &assistant_usage(60_000, 3_000, 80_000, 5_000, 0),
    );
    assert!(
        (after.cost.total_cost_usd - (cost1 + cost2)).abs() < 1e-12,
        "post-resume total {} should equal both turns once: {}",
        after.cost.total_cost_usd,
        cost1 + cost2
    );
}

#[tokio::test]
async fn cost_overview_flags_unknown_model_pricing() {
    // Default test manager uses the unpriced "stub-model".
    let manager = test_manager().await;
    let session_id = "cost-unknown-session";

    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "answer")
                .with_usage(assistant_usage(10_000, 1_000, 0, 0, 0)),
        )
        .await
        .expect("append assistant");

    let cost = manager
        .cost_overview(session_id)
        .await
        .expect("cost overview")
        .cost;
    assert!(
        cost.has_unknown_model_cost,
        "unknown model pricing should be flagged, not silently zero"
    );
    assert!(
        cost.total_cost_usd > 0.0,
        "unknown model should still estimate a non-zero cost via fallback tier"
    );
}

#[tokio::test]
async fn stats_overview_counts_recent_project_messages_by_day() {
    let manager = test_manager().await;
    let today = Local::now().date_naive();
    let yesterday = today - ChronoDuration::days(1);
    let older = today - ChronoDuration::days(200);

    let mut first = TranscriptMessage::new(MessageRole::User, "today one");
    first.created_at = today
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    manager
        .append_message("stats-session-a", first)
        .await
        .expect("append today first");

    let mut second = TranscriptMessage::new(MessageRole::Assistant, "today two");
    second.created_at = today
        .and_hms_opt(12, 1, 0)
        .unwrap()
        .and_local_timezone(Local)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    manager
        .append_message("stats-session-a", second)
        .await
        .expect("append today second");

    let mut third = TranscriptMessage::new(MessageRole::User, "yesterday");
    third.created_at = yesterday
        .and_hms_opt(9, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    manager
        .append_message("stats-session-b", third)
        .await
        .expect("append yesterday");

    let mut old = TranscriptMessage::new(MessageRole::User, "old");
    old.created_at = older
        .and_hms_opt(9, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    manager
        .append_message("stats-session-c", old)
        .await
        .expect("append old");

    let overview = manager.stats_overview().await.expect("stats overview");

    assert_eq!(overview.window_days, 180);
    assert_eq!(overview.message_count, 3);
    assert_eq!(
        overview
            .activity_days
            .iter()
            .find(|day| day.date == today)
            .map(|day| day.message_count),
        Some(2)
    );
    assert_eq!(
        overview
            .activity_days
            .iter()
            .find(|day| day.date == yesterday)
            .map(|day| day.message_count),
        Some(1)
    );
    assert!(overview.activity_days.iter().all(|day| day.date != older));
}
