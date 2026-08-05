use std::collections::HashMap;

use orbcode_config::canonical_model_name;
use orbcode_protocol::TokenUsage;
pub use orbcode_protocol::{BillingBasis, CostSummary, ModelUsage};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelCosts {
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_write_tokens: f64,
    pub cache_read_tokens: f64,
    pub web_search_request: f64,
}

pub const COST_TIER_3_15: ModelCosts = ModelCosts {
    input_tokens: 3.0,
    output_tokens: 15.0,
    cache_write_tokens: 3.75,
    cache_read_tokens: 0.3,
    web_search_request: 0.01,
};

pub const COST_TIER_15_75: ModelCosts = ModelCosts {
    input_tokens: 15.0,
    output_tokens: 75.0,
    cache_write_tokens: 18.75,
    cache_read_tokens: 1.5,
    web_search_request: 0.01,
};

pub const COST_TIER_5_25: ModelCosts = ModelCosts {
    input_tokens: 5.0,
    output_tokens: 25.0,
    cache_write_tokens: 6.25,
    cache_read_tokens: 0.5,
    web_search_request: 0.01,
};

pub const COST_TIER_30_150: ModelCosts = ModelCosts {
    input_tokens: 30.0,
    output_tokens: 150.0,
    cache_write_tokens: 37.5,
    cache_read_tokens: 3.0,
    web_search_request: 0.01,
};

pub const COST_HAIKU_35: ModelCosts = ModelCosts {
    input_tokens: 0.8,
    output_tokens: 4.0,
    cache_write_tokens: 1.0,
    cache_read_tokens: 0.08,
    web_search_request: 0.01,
};

pub const COST_HAIKU_45: ModelCosts = ModelCosts {
    input_tokens: 1.0,
    output_tokens: 5.0,
    cache_write_tokens: 1.25,
    cache_read_tokens: 0.1,
    web_search_request: 0.01,
};

const DEFAULT_UNKNOWN_MODEL_COST: ModelCosts = COST_TIER_5_25;

fn model_costs_for_canonical(canonical: &str) -> Option<ModelCosts> {
    match canonical {
        "claude-3-5-haiku" => Some(COST_HAIKU_35),
        "claude-haiku-4-5" => Some(COST_HAIKU_45),
        "claude-3-5-sonnet" | "claude-3-7-sonnet" | "claude-sonnet-4" | "claude-sonnet-4-5"
        | "claude-sonnet-4-6" => Some(COST_TIER_3_15),
        "claude-opus-4" | "claude-opus-4-1" => Some(COST_TIER_15_75),
        "claude-opus-4-5" | "claude-opus-4-6" => Some(COST_TIER_5_25),
        _ => None,
    }
}

pub fn get_model_costs(model: &str, usage: &TokenUsage) -> (ModelCosts, bool) {
    let canonical = canonical_model_name(&model.to_ascii_lowercase());

    if canonical == "claude-opus-4-6" {
        if usage.speed.as_deref() == Some("fast") {
            return (COST_TIER_30_150, false);
        }
        return (COST_TIER_5_25, false);
    }

    match model_costs_for_canonical(&canonical) {
        Some(costs) => (costs, false),
        None => (DEFAULT_UNKNOWN_MODEL_COST, true),
    }
}

fn tokens_to_usd(costs: &ModelCosts, usage: &TokenUsage) -> f64 {
    (usage.input_tokens as f64 / 1_000_000.0) * costs.input_tokens
        + (usage.output_tokens as f64 / 1_000_000.0) * costs.output_tokens
        + (usage.cache_read_input_tokens as f64 / 1_000_000.0) * costs.cache_read_tokens
        + (usage.cache_creation_input_tokens as f64 / 1_000_000.0) * costs.cache_write_tokens
        + (usage.server_tool_use.web_search_requests as f64) * costs.web_search_request
}

pub fn calculate_usd_cost(model: &str, usage: &TokenUsage) -> (f64, bool) {
    let (costs, has_unknown) = get_model_costs(model, usage);
    (tokens_to_usd(&costs, usage), has_unknown)
}

pub fn format_cost(cost: f64) -> String {
    if cost > 0.5 {
        format!("${:.2}", (cost * 100.0).round() / 100.0)
    } else {
        format!("${cost:.4}")
    }
}

pub fn format_model_pricing(costs: &ModelCosts) -> String {
    fn format_price(price: f64) -> String {
        if price == price.floor() {
            format!("${}", price as u64)
        } else {
            format!("${price:.2}")
        }
    }
    format!(
        "{}/{} per Mtok",
        format_price(costs.input_tokens),
        format_price(costs.output_tokens)
    )
}

#[derive(Clone, Debug, Default)]
pub struct CostTracker {
    total_cost_usd: f64,
    model_usage: HashMap<String, ModelUsage>,
    has_unknown_model_cost: bool,
    billing_basis: Option<BillingBasis>,
}

impl CostTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_usage(
        &mut self,
        model: &str,
        usage: &TokenUsage,
        context_window: u32,
        max_output_tokens: u32,
    ) -> f64 {
        let (cost, has_unknown) = calculate_usd_cost(model, usage);
        if has_unknown {
            self.has_unknown_model_cost = true;
        }
        self.total_cost_usd += cost;

        self.accumulate_usage(
            model,
            usage,
            cost,
            context_window,
            max_output_tokens,
            BillingBasis::Api,
        );

        cost
    }

    pub fn add_subscription_usage(
        &mut self,
        model: &str,
        usage: &TokenUsage,
        context_window: u32,
        max_output_tokens: u32,
    ) {
        self.accumulate_usage(
            model,
            usage,
            0.0,
            context_window,
            max_output_tokens,
            BillingBasis::Subscription,
        );
    }

    fn accumulate_usage(
        &mut self,
        model: &str,
        usage: &TokenUsage,
        cost: f64,
        context_window: u32,
        max_output_tokens: u32,
        billing_basis: BillingBasis,
    ) {
        self.billing_basis = Some(match self.billing_basis {
            Some(existing) => existing.merge(billing_basis),
            None => billing_basis,
        });

        let canonical = canonical_model_name(&model.to_ascii_lowercase());
        let entry = self
            .model_usage
            .entry(canonical)
            .or_insert_with(|| ModelUsage {
                billing_basis,
                ..Default::default()
            });
        entry.billing_basis = entry.billing_basis.merge(billing_basis);
        entry.input_tokens += usage.input_tokens as u64;
        entry.output_tokens += usage.output_tokens as u64;
        entry.cache_read_input_tokens += usage.cache_read_input_tokens as u64;
        entry.cache_creation_input_tokens += usage.cache_creation_input_tokens as u64;
        entry.web_search_requests += usage.server_tool_use.web_search_requests as u64;
        entry.cost_usd += cost;
        entry.context_window = context_window;
        entry.max_output_tokens = max_output_tokens;
    }

    pub fn total_cost_usd(&self) -> f64 {
        self.total_cost_usd
    }

    pub fn model_usage(&self) -> &HashMap<String, ModelUsage> {
        &self.model_usage
    }

    pub fn has_unknown_model_cost(&self) -> bool {
        self.has_unknown_model_cost
    }

    pub fn into_summary(self) -> CostSummary {
        CostSummary {
            total_cost_usd: self.total_cost_usd,
            model_usage: self.model_usage,
            has_unknown_model_cost: self.has_unknown_model_cost,
            billing_basis: self.billing_basis.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use orbcode_protocol::ServerToolUseUsage;

    use super::*;

    fn usage_with(
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
            server_tool_use: ServerToolUseUsage {
                web_search_requests: web_search,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn usage_with_speed(
        input: u32,
        output: u32,
        cache_read: u32,
        cache_creation: u32,
        speed: &str,
    ) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_creation,
            speed: Some(speed.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn known_model_sonnet_resolves_to_3_15_tier() {
        for model in [
            "claude-3-5-sonnet-20241022",
            "claude-3-7-sonnet-20250219",
            "claude-sonnet-4-20250514",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-6",
        ] {
            let usage = TokenUsage::default();
            let (costs, has_unknown) = get_model_costs(model, &usage);
            assert_eq!(
                costs, COST_TIER_3_15,
                "model {model} should map to 3/15 tier"
            );
            assert!(!has_unknown, "model {model} should be known");
        }
    }

    #[test]
    fn known_model_opus_4_4_1_resolves_to_15_75_tier() {
        for model in ["claude-opus-4-20250514", "claude-opus-4-1-20250805"] {
            let usage = TokenUsage::default();
            let (costs, has_unknown) = get_model_costs(model, &usage);
            assert_eq!(
                costs, COST_TIER_15_75,
                "model {model} should map to 15/75 tier"
            );
            assert!(!has_unknown, "model {model} should be known");
        }
    }

    #[test]
    fn known_model_opus_4_5_resolves_to_5_25_tier() {
        let usage = TokenUsage::default();
        let (costs, has_unknown) = get_model_costs("claude-opus-4-5-20251101", &usage);
        assert_eq!(costs, COST_TIER_5_25);
        assert!(!has_unknown);
    }

    #[test]
    fn known_model_opus_4_6_normal_resolves_to_5_25_tier() {
        let usage = TokenUsage::default();
        let (costs, has_unknown) = get_model_costs("claude-opus-4-6", &usage);
        assert_eq!(costs, COST_TIER_5_25);
        assert!(!has_unknown);
    }

    #[test]
    fn known_model_haiku_35_resolves_to_haiku_35_tier() {
        let usage = TokenUsage::default();
        let (costs, has_unknown) = get_model_costs("claude-3-5-haiku-20241022", &usage);
        assert_eq!(costs, COST_HAIKU_35);
        assert!(!has_unknown);
    }

    #[test]
    fn known_model_haiku_45_resolves_to_haiku_45_tier() {
        let usage = TokenUsage::default();
        let (costs, has_unknown) = get_model_costs("claude-haiku-4-5-20251001", &usage);
        assert_eq!(costs, COST_HAIKU_45);
        assert!(!has_unknown);
    }

    #[test]
    fn canonical_name_stripping_date_suffix() {
        let usage = TokenUsage::default();
        let (costs_full, _) = get_model_costs("claude-opus-4-6-20250514", &usage);
        let (costs_short, _) = get_model_costs("claude-opus-4-6", &usage);
        assert_eq!(costs_full, costs_short);
    }

    #[test]
    fn canonical_name_bedrock_arn() {
        let usage = TokenUsage::default();
        let (costs, has_unknown) = get_model_costs("us.anthropic.claude-sonnet-4-6-v1:0", &usage);
        assert_eq!(costs, COST_TIER_3_15);
        assert!(!has_unknown);
    }

    #[test]
    fn cache_token_pricing_different_rates() {
        let usage = usage_with(1_000_000, 0, 1_000_000, 1_000_000, 0);
        let (cost, _) = calculate_usd_cost("claude-sonnet-4-6", &usage);
        let expected = 3.0 + 0.3 + 3.75;
        assert!(
            (cost - expected).abs() < 1e-10,
            "expected {expected}, got {cost}"
        );
    }

    #[test]
    fn opus_4_6_fast_mode_uses_30_150_tier() {
        let usage = usage_with_speed(1_000_000, 1_000_000, 0, 0, "fast");
        let (costs, has_unknown) = get_model_costs("claude-opus-4-6", &usage);
        assert_eq!(costs, COST_TIER_30_150);
        assert!(!has_unknown);

        let (cost, _) = calculate_usd_cost("claude-opus-4-6", &usage);
        let expected = 30.0 + 150.0;
        assert!(
            (cost - expected).abs() < 1e-10,
            "expected {expected}, got {cost}"
        );
    }

    #[test]
    fn opus_4_6_non_fast_uses_5_25_tier() {
        let usage = usage_with_speed(1_000_000, 1_000_000, 0, 0, "normal");
        let (costs, _) = get_model_costs("claude-opus-4-6", &usage);
        assert_eq!(costs, COST_TIER_5_25);
    }

    #[test]
    fn unknown_model_fallback_to_5_25_tier() {
        let usage = TokenUsage::default();
        let (costs, has_unknown) = get_model_costs("gpt-4o-mini", &usage);
        assert_eq!(costs, COST_TIER_5_25);
        assert!(has_unknown);
    }

    #[test]
    fn unknown_model_another_string() {
        let usage = TokenUsage::default();
        let (costs, has_unknown) = get_model_costs("my-custom-model-v1", &usage);
        assert_eq!(costs, COST_TIER_5_25);
        assert!(has_unknown);
    }

    #[test]
    fn zero_usage_produces_zero_cost() {
        let usage = TokenUsage::default();
        let (cost, _) = calculate_usd_cost("claude-sonnet-4-6", &usage);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn web_search_cost_per_request() {
        let usage = usage_with(0, 0, 0, 0, 5);
        let (cost, _) = calculate_usd_cost("claude-sonnet-4-6", &usage);
        assert!(
            (cost - 0.05).abs() < 1e-10,
            "5 web searches at $0.01 = $0.05"
        );
    }

    #[test]
    fn end_to_end_realistic_usage() {
        let usage = usage_with(50_000, 2_000, 100_000, 10_000, 2);
        let (cost, has_unknown) = calculate_usd_cost("claude-sonnet-4-6", &usage);
        let expected = (50_000.0 / 1e6) * 3.0
            + (2_000.0 / 1e6) * 15.0
            + (100_000.0 / 1e6) * 0.3
            + (10_000.0 / 1e6) * 3.75
            + 2.0 * 0.01;
        assert!(
            (cost - expected).abs() < 1e-10,
            "expected {expected}, got {cost}"
        );
        assert!(!has_unknown);
    }

    #[test]
    fn cost_tracker_accumulation() {
        let mut tracker = CostTracker::new();
        let usage1 = usage_with(1_000_000, 500_000, 0, 0, 0);
        let usage2 = usage_with(2_000_000, 1_000_000, 0, 0, 0);

        let cost1 = tracker.add_usage("claude-sonnet-4-6", &usage1, 200_000, 16_384);
        let cost2 = tracker.add_usage("claude-sonnet-4-6", &usage2, 200_000, 16_384);

        let expected_total = cost1 + cost2;
        assert!(
            (tracker.total_cost_usd() - expected_total).abs() < 1e-10,
            "tracker total should accumulate"
        );

        let model_usage = &tracker.model_usage()["claude-sonnet-4-6"];
        assert_eq!(model_usage.input_tokens, 3_000_000);
        assert_eq!(model_usage.output_tokens, 1_500_000);
        assert_eq!(model_usage.context_window, 200_000);
        assert_eq!(model_usage.max_output_tokens, 16_384);
    }

    #[test]
    fn subscription_usage_counts_tokens_without_api_cost() {
        let mut tracker = CostTracker::new();
        tracker.add_subscription_usage(
            "gpt-5.6-sol",
            &usage_with(10_000, 2_000, 3_000, 500, 0),
            272_000,
            128_000,
        );

        let summary = tracker.into_summary();
        assert_eq!(summary.total_cost_usd, 0.0);
        assert!(!summary.has_unknown_model_cost);
        assert_eq!(summary.billing_basis, BillingBasis::Subscription);
        let usage = &summary.model_usage["gpt-5.6-sol"];
        assert_eq!(usage.input_tokens, 10_000);
        assert_eq!(usage.output_tokens, 2_000);
        assert_eq!(usage.billing_basis, BillingBasis::Subscription);
        assert!(usage.to_string().contains("subscription (not API-priced)"));
    }

    #[test]
    fn mixed_usage_display_matches_diagnostics_cost_wording() {
        let usage = ModelUsage {
            cost_usd: 0.42,
            billing_basis: BillingBasis::Mixed,
            ..ModelUsage::default()
        };

        assert!(
            usage
                .to_string()
                .contains("$0.4200 API + subscription usage (not API-priced)")
        );
    }

    #[test]
    fn cost_tracker_multi_model() {
        let mut tracker = CostTracker::new();
        let sonnet_usage = usage_with(1_000_000, 1_000_000, 0, 0, 0);
        let haiku_usage = usage_with(1_000_000, 1_000_000, 0, 0, 0);

        tracker.add_usage("claude-sonnet-4-6", &sonnet_usage, 200_000, 16_384);
        tracker.add_usage("claude-haiku-4-5-20251001", &haiku_usage, 200_000, 8_192);

        assert_eq!(tracker.model_usage().len(), 2);

        let sonnet = &tracker.model_usage()["claude-sonnet-4-6"];
        assert!(
            (sonnet.cost_usd - (3.0 + 15.0)).abs() < 1e-10,
            "sonnet cost should be $18"
        );

        let haiku = &tracker.model_usage()["claude-haiku-4-5"];
        assert!(
            (haiku.cost_usd - (1.0 + 5.0)).abs() < 1e-10,
            "haiku cost should be $6"
        );

        assert!(!tracker.has_unknown_model_cost());
    }

    #[test]
    fn cost_tracker_unknown_model_sets_flag() {
        let mut tracker = CostTracker::new();
        let usage = usage_with(1_000_000, 1_000_000, 0, 0, 0);
        tracker.add_usage("some-unknown-model", &usage, 128_000, 4_096);
        assert!(tracker.has_unknown_model_cost());
    }

    #[test]
    fn cost_tracker_into_summary() {
        let mut tracker = CostTracker::new();
        let usage = usage_with(1_000_000, 0, 0, 0, 0);
        tracker.add_usage("claude-sonnet-4-6", &usage, 200_000, 16_384);
        let summary = tracker.into_summary();
        assert!((summary.total_cost_usd - 3.0).abs() < 1e-10);
        assert!(!summary.has_unknown_model_cost);
        assert!(summary.model_usage.contains_key("claude-sonnet-4-6"));
    }

    #[test]
    fn format_cost_large_value() {
        assert_eq!(format_cost(1.234), "$1.23");
        assert_eq!(format_cost(10.0), "$10.00");
    }

    #[test]
    fn format_cost_small_value() {
        assert_eq!(format_cost(0.1234), "$0.1234");
        assert_eq!(format_cost(0.0001), "$0.0001");
    }

    #[test]
    fn format_model_pricing_display() {
        assert_eq!(format_model_pricing(&COST_TIER_3_15), "$3/$15 per Mtok");
        assert_eq!(format_model_pricing(&COST_HAIKU_35), "$0.80/$4 per Mtok");
    }
}
