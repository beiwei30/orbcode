use serde::{Deserialize, Serialize};

use crate::usage::TokenUsage;

/// Per-model pricing in USD per one million tokens for each billable token
/// class. This is a pure data model: it carries no model-identity logic and is
/// looked up via [`pricing_for_model`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    /// USD per 1M uncached input tokens.
    pub input_per_mtok: f64,
    /// USD per 1M output tokens.
    pub output_per_mtok: f64,
    /// USD per 1M cache-read input tokens.
    pub cache_read_per_mtok: f64,
    /// USD per 1M cache-write (cache-creation) input tokens.
    pub cache_write_per_mtok: f64,
}

impl ModelPricing {
    pub const fn new(
        input_per_mtok: f64,
        output_per_mtok: f64,
        cache_read_per_mtok: f64,
        cache_write_per_mtok: f64,
    ) -> Self {
        Self {
            input_per_mtok,
            output_per_mtok,
            cache_read_per_mtok,
            cache_write_per_mtok,
        }
    }
}

/// Anthropic Opus placeholder pricing ($15 / $75 per Mtok).
pub const PRICING_ANTHROPIC_OPUS: ModelPricing = ModelPricing::new(15.0, 75.0, 1.5, 18.75);
/// Anthropic Sonnet placeholder pricing ($3 / $15 per Mtok).
pub const PRICING_ANTHROPIC_SONNET: ModelPricing = ModelPricing::new(3.0, 15.0, 0.3, 3.75);
/// Anthropic Haiku placeholder pricing ($0.80 / $4 per Mtok).
pub const PRICING_ANTHROPIC_HAIKU: ModelPricing = ModelPricing::new(0.8, 4.0, 0.08, 1.0);
/// OpenAI-compatible placeholder pricing ($5 / $15 per Mtok).
pub const PRICING_OPENAI_COMPATIBLE: ModelPricing = ModelPricing::new(5.0, 15.0, 2.5, 6.25);

/// Look up placeholder pricing by model id. Returns `None` for models with no
/// known pricing so callers can distinguish "free" from "unknown" instead of
/// silently treating an unpriced model as $0.
///
/// Matching is substring-based on the lowercased id so full API ids
/// (`claude-opus-4-6-20250514`), short ids (`claude-opus-4-6`), and provider
/// prefixes all resolve. Order matters: more specific families are checked
/// first.
pub fn pricing_for_model(model_id: &str) -> Option<ModelPricing> {
    let id = model_id.to_ascii_lowercase();
    if id.contains("opus") {
        Some(PRICING_ANTHROPIC_OPUS)
    } else if id.contains("sonnet") {
        Some(PRICING_ANTHROPIC_SONNET)
    } else if id.contains("haiku") {
        Some(PRICING_ANTHROPIC_HAIKU)
    } else if id.contains("gpt") || id.contains("openai") || id.contains("o1") || id.contains("o3")
    {
        Some(PRICING_OPENAI_COMPATIBLE)
    } else {
        None
    }
}

/// Per-class USD cost for a single [`TokenUsage`] sample plus a flag recording
/// whether pricing was actually known. When pricing is unknown every component
/// is `0.0` and `pricing_known` is `false`, so an unpriced model is never
/// confused with a genuinely free turn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_read_cost_usd: f64,
    pub cache_write_cost_usd: f64,
    pub total_usd: f64,
    /// `false` when pricing was unknown (all costs fell back to `0.0`).
    pub pricing_known: bool,
}

impl CostBreakdown {
    /// A zero-cost breakdown flagged as having unknown pricing.
    pub const UNKNOWN: CostBreakdown = CostBreakdown {
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        cache_write_cost_usd: 0.0,
        total_usd: 0.0,
        pricing_known: false,
    };
}

/// Compute the USD cost of `usage` under the given `pricing`. Pure function with
/// no model-identity logic. Pass `None` for an unpriced model: the result is a
/// zero-cost breakdown with `pricing_known == false` rather than a silent `$0`.
pub fn accumulate_cost(usage: &TokenUsage, pricing: Option<&ModelPricing>) -> CostBreakdown {
    let Some(pricing) = pricing else {
        return CostBreakdown::UNKNOWN;
    };
    let per_mtok = |tokens: u32, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
    let input_cost_usd = per_mtok(usage.input_tokens, pricing.input_per_mtok);
    let output_cost_usd = per_mtok(usage.output_tokens, pricing.output_per_mtok);
    let cache_read_cost_usd = per_mtok(usage.cache_read_input_tokens, pricing.cache_read_per_mtok);
    let cache_write_cost_usd = per_mtok(
        usage.cache_creation_input_tokens,
        pricing.cache_write_per_mtok,
    );
    CostBreakdown {
        input_cost_usd,
        output_cost_usd,
        cache_read_cost_usd,
        cache_write_cost_usd,
        total_usd: input_cost_usd + output_cost_usd + cache_read_cost_usd + cache_write_cost_usd,
        pricing_known: true,
    }
}

/// Running cost total against an optional spend cap. A `max_budget_usd` of
/// `None` means no cap is configured (never over budget).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct BudgetState {
    pub total_usd: f64,
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
}

impl BudgetState {
    pub fn new(max_budget_usd: Option<f64>) -> Self {
        Self {
            total_usd: 0.0,
            max_budget_usd,
        }
    }

    /// Add a turn's cost to the running total.
    pub fn add(&mut self, cost: &CostBreakdown) {
        self.total_usd += cost.total_usd;
    }

    /// Whether the accumulated total has reached or exceeded the configured cap.
    /// Returns `false` when no cap is set.
    pub fn is_over_budget(&self) -> bool {
        match self.max_budget_usd {
            Some(max) => over_budget(self.total_usd, max),
            None => false,
        }
    }
}

/// Budget decision: reaching the cap exactly counts as over budget so a turn
/// that lands on the limit still stops the loop.
pub fn over_budget(total_usd: f64, max_budget_usd: f64) -> bool {
    total_usd >= max_budget_usd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, cache_read: u32, cache_write: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_write,
            ..Default::default()
        }
    }

    #[test]
    fn pricing_lookup_resolves_known_families() {
        assert_eq!(
            pricing_for_model("claude-opus-4-6-20250514"),
            Some(PRICING_ANTHROPIC_OPUS)
        );
        assert_eq!(
            pricing_for_model("claude-sonnet-4-6"),
            Some(PRICING_ANTHROPIC_SONNET)
        );
        assert_eq!(
            pricing_for_model("claude-3-5-haiku-20241022"),
            Some(PRICING_ANTHROPIC_HAIKU)
        );
        assert_eq!(
            pricing_for_model("gpt-4o-mini"),
            Some(PRICING_OPENAI_COMPATIBLE)
        );
    }

    #[test]
    fn pricing_lookup_returns_none_for_unknown() {
        assert_eq!(pricing_for_model("my-custom-model-v1"), None);
    }

    #[test]
    fn known_model_cost_sums_all_classes() {
        let pricing = pricing_for_model("claude-sonnet-4-6").unwrap();
        let breakdown = accumulate_cost(&usage(50_000, 2_000, 100_000, 10_000), Some(&pricing));
        let expected_input = (50_000.0 / 1e6) * 3.0;
        let expected_output = (2_000.0 / 1e6) * 15.0;
        let expected_cache_read = (100_000.0 / 1e6) * 0.3;
        let expected_cache_write = (10_000.0 / 1e6) * 3.75;
        assert!((breakdown.input_cost_usd - expected_input).abs() < 1e-12);
        assert!((breakdown.output_cost_usd - expected_output).abs() < 1e-12);
        assert!((breakdown.cache_read_cost_usd - expected_cache_read).abs() < 1e-12);
        assert!((breakdown.cache_write_cost_usd - expected_cache_write).abs() < 1e-12);
        assert!(
            (breakdown.total_usd
                - (expected_input + expected_output + expected_cache_read + expected_cache_write))
                .abs()
                < 1e-12
        );
        assert!(breakdown.pricing_known);
    }

    #[test]
    fn cache_tokens_are_billed_at_their_own_rates() {
        let pricing = pricing_for_model("claude-sonnet-4-6").unwrap();
        let breakdown = accumulate_cost(&usage(1_000_000, 0, 1_000_000, 1_000_000), Some(&pricing));
        assert!((breakdown.total_usd - (3.0 + 0.3 + 3.75)).abs() < 1e-12);
        assert!((breakdown.cache_read_cost_usd - 0.3).abs() < 1e-12);
        assert!((breakdown.cache_write_cost_usd - 3.75).abs() < 1e-12);
    }

    #[test]
    fn unknown_model_falls_back_to_zero_but_is_flagged() {
        let pricing = pricing_for_model("totally-unknown");
        assert!(pricing.is_none());
        let breakdown = accumulate_cost(&usage(1_000_000, 1_000_000, 0, 0), pricing.as_ref());
        assert_eq!(breakdown.total_usd, 0.0);
        assert!(
            !breakdown.pricing_known,
            "unknown pricing must be distinguishable from a genuine $0 turn"
        );
    }

    #[test]
    fn zero_usage_with_known_pricing_is_free_but_known() {
        let pricing = pricing_for_model("claude-opus-4-6").unwrap();
        let breakdown = accumulate_cost(&TokenUsage::default(), Some(&pricing));
        assert_eq!(breakdown.total_usd, 0.0);
        assert!(breakdown.pricing_known);
    }

    #[test]
    fn over_budget_boundary_is_inclusive() {
        assert!(!over_budget(9.99, 10.0), "under budget");
        assert!(over_budget(10.0, 10.0), "exactly at budget counts as over");
        assert!(over_budget(10.01, 10.0), "above budget");
    }

    #[test]
    fn budget_state_accumulates_and_detects_cap() {
        let pricing = pricing_for_model("claude-sonnet-4-6").unwrap();
        let mut state = BudgetState::new(Some(6.0));
        state.add(&accumulate_cost(&usage(1_000_000, 0, 0, 0), Some(&pricing)));
        assert!(!state.is_over_budget(), "$3 < $6 cap");
        state.add(&accumulate_cost(&usage(1_000_000, 0, 0, 0), Some(&pricing)));
        assert!(state.is_over_budget(), "$6 reaches $6 cap");
    }

    #[test]
    fn budget_state_without_cap_is_never_over() {
        let mut state = BudgetState::new(None);
        state.total_usd = 1_000_000.0;
        assert!(!state.is_over_budget());
    }
}
