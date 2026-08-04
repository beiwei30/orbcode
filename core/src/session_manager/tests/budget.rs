use super::super::session_turn_loop::BudgetDecision;
use super::support::*;
use super::*;
use orbcode_protocol::{BudgetOutcome, TokenUsage};

const EPSILON: f64 = 1e-9;

fn usage(input: u32, output: u32) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        ..TokenUsage::default()
    }
}

/// Accumulates a priced assistant turn (Sonnet) and returns the live total.
async fn seed_priced_cost(manager: &SessionManager, session_id: &str) -> f64 {
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "answer")
                .with_usage(usage(200_000, 50_000)),
        )
        .await
        .expect("append assistant");
    let (total, pricing_known) = manager.live_cost_total(session_id).await;
    assert!(
        total > 0.0,
        "priced model should accumulate a non-zero cost"
    );
    assert!(pricing_known, "sonnet pricing must be known");
    total
}

/// Accumulates an unpriced assistant turn (default stub-model) and returns the
/// live total. The total is non-zero (fallback tier) but pricing is flagged
/// unknown, so it is never silently treated as `$0`.
async fn seed_unpriced_cost(manager: &SessionManager, session_id: &str) -> f64 {
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "answer")
                .with_usage(usage(200_000, 50_000)),
        )
        .await
        .expect("append assistant");
    let (total, pricing_known) = manager.live_cost_total(session_id).await;
    assert!(total > 0.0, "fallback tier should still estimate a cost");
    assert!(!pricing_known, "stub-model pricing must be flagged unknown");
    total
}

#[tokio::test]
async fn precheck_is_none_when_no_cap_configured() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "budget-unconfigured";
    seed_priced_cost(&manager, session_id).await;

    // No `maxBudgetUsd` set: enforcement is entirely inert.
    assert!(manager.config.max_budget_usd().is_none());
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    assert!(
        decision.is_none(),
        "no cap configured must never block or warn"
    );
}

#[tokio::test]
async fn precheck_is_none_under_cap_with_known_pricing() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "budget-under-cap";
    let total = seed_priced_cost(&manager, session_id).await;

    // Cap comfortably above the accumulated cost: the request proceeds.
    manager.config.settings.max_budget_usd = Some(total + 1.0);
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    assert!(decision.is_none(), "under cap with known pricing proceeds");
}

#[tokio::test]
async fn precheck_blocks_at_cap_boundary() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "budget-at-cap";
    let total = seed_priced_cost(&manager, session_id).await;

    // Reaching the cap exactly counts as over budget (inclusive boundary).
    manager.config.settings.max_budget_usd = Some(total);
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    match decision {
        Some(BudgetDecision::Block {
            outcome,
            total_usd,
            max_budget_usd,
            pricing_known,
        }) => {
            assert_eq!(outcome, BudgetOutcome::Exceeded);
            assert!(pricing_known);
            assert!((total_usd - total).abs() < EPSILON);
            assert!((max_budget_usd - total).abs() < EPSILON);
        }
        other => panic!("expected Block at the cap boundary, got {other:?}"),
    }
}

#[tokio::test]
async fn precheck_blocks_over_cap() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "budget-over-cap";
    let total = seed_priced_cost(&manager, session_id).await;

    manager.config.settings.max_budget_usd = Some(total / 2.0);
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    match decision {
        Some(BudgetDecision::Block {
            outcome,
            total_usd,
            pricing_known,
            ..
        }) => {
            assert_eq!(outcome, BudgetOutcome::Exceeded);
            assert!(pricing_known);
            assert!((total_usd - total).abs() < EPSILON);
        }
        other => panic!("expected Block over the cap, got {other:?}"),
    }
}

#[tokio::test]
async fn precheck_warns_on_unknown_pricing_under_non_strict_policy() {
    let manager = test_manager().await;
    let session_id = "budget-unknown-warn";
    let total = seed_unpriced_cost(&manager, session_id).await;

    // Under the cap, but pricing is unknown. Default (non-strict) policy warns
    // and proceeds rather than blocking or silently ignoring the cost.
    let mut manager = manager;
    manager.config.settings.max_budget_usd = Some(total + 100.0);
    assert!(!manager.config.max_budget_strict_unknown_pricing());
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    match decision {
        Some(BudgetDecision::Warn {
            total_usd,
            max_budget_usd,
        }) => {
            assert!((total_usd - total).abs() < EPSILON);
            assert!((max_budget_usd - (total + 100.0)).abs() < EPSILON);
        }
        other => panic!("expected Warn under non-strict policy, got {other:?}"),
    }
}

#[tokio::test]
async fn precheck_blocks_on_unknown_pricing_under_strict_policy() {
    let mut manager = test_manager().await;
    let session_id = "budget-unknown-strict";
    let total = seed_unpriced_cost(&manager, session_id).await;

    manager.config.settings.max_budget_usd = Some(total + 100.0);
    manager.config.settings.max_budget_strict_unknown_pricing = Some(true);
    assert!(manager.config.max_budget_strict_unknown_pricing());
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    match decision {
        Some(BudgetDecision::Block {
            outcome,
            pricing_known,
            ..
        }) => {
            assert_eq!(outcome, BudgetOutcome::UnknownPricing);
            assert!(!pricing_known);
        }
        other => panic!("expected Block under strict policy, got {other:?}"),
    }
}

#[tokio::test]
async fn precheck_block_over_cap_wins_over_unknown_pricing() {
    let mut manager = test_manager().await;
    let session_id = "budget-unknown-over-cap";
    let total = seed_unpriced_cost(&manager, session_id).await;

    // Already over budget AND pricing unknown: the hard-cap check fires first so
    // the outcome is `Exceeded`, not `UnknownPricing`. Real spend is at least the
    // under-counted total, so an over-cap session must block regardless of the
    // unknown-pricing policy.
    manager.config.settings.max_budget_usd = Some(total / 2.0);
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    match decision {
        Some(BudgetDecision::Block {
            outcome,
            pricing_known,
            ..
        }) => {
            assert_eq!(outcome, BudgetOutcome::Exceeded);
            assert!(
                !pricing_known,
                "pricing was unknown but the cap still blocks"
            );
        }
        other => panic!("expected Exceeded block to win, got {other:?}"),
    }
}

#[tokio::test]
async fn subscription_usage_does_not_trigger_api_budget_cap() {
    let mut manager = test_manager().await;
    manager.config.default_provider = ProviderId::OpenAi;
    manager.config.settings.max_budget_usd = Some(0.000_001);
    std::fs::write(
        manager.config.home_dir.join("auth.json"),
        format!(
            r#"{{"entries":[{{"provider":"openai","method":"chatgpt","source":{{"kind":"chatgpt_oauth","credentials":{{"id_token":"id","access_token":"access","refresh_token":"refresh","expires_at":{},"account_id":"account-123","email":null,"plan_type":"plus"}}}},"updated_at":"2026-08-03T00:00:00Z"}}]}}"#,
            chrono::Utc::now().timestamp_millis() + 60 * 60 * 1000
        ),
    )
    .expect("write ChatGPT auth");
    manager
        .auth
        .refresh_stored_state()
        .await
        .expect("refresh auth state");

    let session_id = "subscription-budget";
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "answer")
                .with_usage(usage(1_000_000, 100_000)),
        )
        .await
        .expect("append assistant");

    let overview = manager
        .cost_overview(session_id)
        .await
        .expect("cost overview");
    assert_eq!(overview.cost.total_cost_usd, 0.0);
    assert_eq!(
        overview.cost.billing_basis,
        crate::BillingBasis::Subscription
    );
    assert_eq!(manager.model_display_name(), "gpt-5.6-sol");
    assert!(
        manager
            .budget_precheck(session_id, &manager.config)
            .await
            .is_none(),
        "subscription usage must not be compared with the API dollar budget"
    );
}

#[tokio::test]
async fn incomplete_chatgpt_credentials_do_not_disable_api_budget_checks() {
    let mut manager = test_manager().await;
    manager.config.default_provider = ProviderId::OpenAi;
    manager.config.settings.max_budget_usd = Some(0.000_001);
    std::fs::write(
        manager.config.home_dir.join("auth.json"),
        format!(
            r#"{{"entries":[{{"provider":"openai","method":"chatgpt","source":{{"kind":"chatgpt_oauth","credentials":{{"id_token":"id","access_token":"access","refresh_token":"refresh","expires_at":{},"account_id":null,"email":null,"plan_type":"plus"}}}},"updated_at":"2026-08-03T00:00:00Z"}}]}}"#,
            chrono::Utc::now().timestamp_millis() + 60 * 60 * 1000
        ),
    )
    .expect("write incomplete ChatGPT auth");
    manager
        .auth
        .refresh_stored_state()
        .await
        .expect("refresh auth state");

    let session_id = "incomplete-subscription-budget";
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "answer")
                .with_usage(usage(1_000_000, 100_000)),
        )
        .await
        .expect("append assistant");

    let overview = manager
        .cost_overview(session_id)
        .await
        .expect("cost overview");
    assert_eq!(overview.cost.billing_basis, crate::BillingBasis::Api);
    assert!(!manager.uses_chatgpt_subscription());
    assert!(
        manager
            .budget_precheck(session_id, &manager.config)
            .await
            .is_some(),
        "incomplete credentials must not bypass the API budget policy"
    );
}

#[tokio::test]
async fn fallback_discard_preserves_partial_usage_in_live_cost() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "budget-fallback-discard";

    // Simulate what SessionProviderStreamSink::discard_attempt does:
    // accumulate a synthetic message carrying the discarded attempt's usage.
    let discarded_usage = usage(100_000, 20_000);
    let cost_message = TranscriptMessage::new(MessageRole::Assistant, "")
        .with_usage(discarded_usage)
        .with_synthetic(true);
    manager
        .accumulate_live_cost(session_id, &cost_message)
        .await;

    let (total_after_discard, _) = manager.live_cost_total(session_id).await;
    assert!(
        total_after_discard > 0.0,
        "discarded attempt usage must still accumulate into the live cost"
    );

    // Now accumulate the successful fallback response.
    let fallback_usage = usage(150_000, 30_000);
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "fallback answer")
                .with_usage(fallback_usage),
        )
        .await
        .expect("append fallback message");

    let (total_after_fallback, _) = manager.live_cost_total(session_id).await;
    assert!(
        total_after_fallback > total_after_discard,
        "total after fallback ({total_after_fallback}) must exceed the discarded partial ({total_after_discard})"
    );
}

#[tokio::test]
async fn cancelled_turn_empty_content_preserves_usage_in_live_cost() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let session_id = "budget-cancelled-empty";

    // Simulate what session_turn_loop does for a cancelled turn with empty
    // content: accumulate a synthetic cost-only message.
    let cancelled_usage = usage(80_000, 0);
    let cost_message = TranscriptMessage::new(MessageRole::Assistant, "")
        .with_usage(cancelled_usage)
        .with_synthetic(true);
    manager
        .accumulate_live_cost(session_id, &cost_message)
        .await;

    let (total, pricing_known) = manager.live_cost_total(session_id).await;
    assert!(
        total > 0.0,
        "cancelled turn with empty content must still record cost"
    );
    assert!(pricing_known, "sonnet pricing should be known");

    // Set a cap below the accumulated cost: budget_precheck must block.
    manager.config.settings.max_budget_usd = Some(total / 2.0);
    let decision = manager.budget_precheck(session_id, &manager.config).await;
    match decision {
        Some(BudgetDecision::Block { outcome, .. }) => {
            assert_eq!(outcome, BudgetOutcome::Exceeded);
        }
        other => panic!("expected Block after cancelled turn usage, got {other:?}"),
    }
}
