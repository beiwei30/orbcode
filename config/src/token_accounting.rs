pub const MODEL_CONTEXT_WINDOW_DEFAULT: u32 = 200_000;
pub const CONTEXT_1M_TOKENS: u32 = 1_000_000;
pub const COMPACT_MAX_OUTPUT_TOKENS: u32 = 20_000;
pub const MAX_OUTPUT_TOKENS_DEFAULT: u32 = 32_000;
pub const MAX_OUTPUT_TOKENS_UPPER_LIMIT: u32 = 64_000;
pub const AUTOCOMPACT_BUFFER_TOKENS: u32 = 13_000;
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u32 = 20_000;
pub const ERROR_THRESHOLD_BUFFER_TOKENS: u32 = 20_000;
pub const MANUAL_COMPACT_BUFFER_TOKENS: u32 = 3_000;

use crate::{ContextWindowOptions, MaxOutputTokenOptions, TokenWarningOptions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelMaxOutputTokens {
    pub default: u32,
    pub upper_limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenWarningState {
    pub percent_left: u32,
    pub warning_threshold: u32,
    pub error_threshold: u32,
    pub auto_compact_threshold: u32,
    pub blocking_limit: u32,
    pub is_above_warning_threshold: bool,
    pub is_above_error_threshold: bool,
    pub is_above_auto_compact_threshold: bool,
    pub is_at_blocking_limit: bool,
}

pub fn resolve_context_window(model: &str, options: &ContextWindowOptions) -> u32 {
    if let Some(override_tokens) = options
        .max_context_tokens_override
        .filter(|value| *value > 0)
    {
        return override_tokens;
    }
    if has_1m_context(model, options) || model_supports_1m_context(model, options) {
        return CONTEXT_1M_TOKENS;
    }
    MODEL_CONTEXT_WINDOW_DEFAULT
}

pub fn has_1m_context(model: &str, options: &ContextWindowOptions) -> bool {
    !options.disable_1m_context && model.to_ascii_lowercase().contains("[1m]")
}

pub fn model_supports_1m_context(model: &str, options: &ContextWindowOptions) -> bool {
    if options.disable_1m_context {
        return false;
    }
    let canonical = canonical_model_name(model);
    canonical.contains("claude-sonnet-4")
        || canonical.contains("opus-4-6")
        || canonical.contains("opus-4-7")
}

pub fn resolve_model_max_output_tokens(model: &str) -> ModelMaxOutputTokens {
    let canonical = canonical_model_name(model);
    if canonical.contains("opus-4-6") {
        ModelMaxOutputTokens {
            default: 64_000,
            upper_limit: 128_000,
        }
    } else if canonical.contains("sonnet-4-6") {
        ModelMaxOutputTokens {
            default: 32_000,
            upper_limit: 128_000,
        }
    } else if canonical.contains("opus-4-5")
        || canonical.contains("sonnet-4")
        || canonical.contains("haiku-4")
        || canonical.contains("3-7-sonnet")
    {
        ModelMaxOutputTokens {
            default: 32_000,
            upper_limit: 64_000,
        }
    } else if canonical.contains("opus-4-1") || canonical.contains("opus-4") {
        ModelMaxOutputTokens {
            default: 32_000,
            upper_limit: 32_000,
        }
    } else if canonical.contains("claude-3-sonnet")
        || canonical.contains("3-5-sonnet")
        || canonical.contains("3-5-haiku")
    {
        ModelMaxOutputTokens {
            default: 8_192,
            upper_limit: 8_192,
        }
    } else if canonical.contains("claude-3-opus") || canonical.contains("claude-3-haiku") {
        ModelMaxOutputTokens {
            default: 4_096,
            upper_limit: 4_096,
        }
    } else {
        ModelMaxOutputTokens {
            default: MAX_OUTPUT_TOKENS_DEFAULT,
            upper_limit: MAX_OUTPUT_TOKENS_UPPER_LIMIT,
        }
    }
}

pub fn resolve_max_output_tokens(model: &str, options: &MaxOutputTokenOptions) -> u32 {
    let limits = resolve_model_max_output_tokens(model);
    options
        .max_output_tokens_override
        .filter(|value| *value > 0)
        .map_or(limits.default, |value| value.min(limits.upper_limit))
}

pub fn effective_context_window_size(
    model: &str,
    context_options: &ContextWindowOptions,
    max_output_options: &MaxOutputTokenOptions,
) -> u32 {
    let reserved_tokens =
        resolve_max_output_tokens(model, max_output_options).min(COMPACT_MAX_OUTPUT_TOKENS);
    let context_window = context_options
        .auto_compact_window_override
        .filter(|value| *value > 0)
        .map_or_else(
            || resolve_context_window(model, context_options),
            |override_tokens| resolve_context_window(model, context_options).min(override_tokens),
        );
    context_window.saturating_sub(reserved_tokens)
}

pub fn auto_compact_threshold(
    model: &str,
    context_options: &ContextWindowOptions,
    max_output_options: &MaxOutputTokenOptions,
    warning_options: &TokenWarningOptions,
) -> u32 {
    let effective_window =
        effective_context_window_size(model, context_options, max_output_options);
    let default_threshold = effective_window.saturating_sub(AUTOCOMPACT_BUFFER_TOKENS);
    warning_options
        .auto_compact_percent_override
        .filter(|percent| (1..=100).contains(percent))
        .map(|percent| (effective_window as u64 * percent as u64 / 100) as u32)
        .map_or(default_threshold, |threshold| {
            threshold.min(default_threshold)
        })
}

pub fn calculate_token_warning_state(
    token_usage: u32,
    model: &str,
    context_options: &ContextWindowOptions,
    max_output_options: &MaxOutputTokenOptions,
    warning_options: &TokenWarningOptions,
) -> TokenWarningState {
    let effective_window =
        effective_context_window_size(model, context_options, max_output_options);
    let auto_compact_threshold =
        auto_compact_threshold(model, context_options, max_output_options, warning_options);
    let threshold = if warning_options.auto_compact_enabled {
        auto_compact_threshold
    } else {
        effective_window
    };
    let percent_left = if threshold == 0 {
        0
    } else {
        let remaining = threshold.saturating_sub(token_usage);
        (((remaining as f64 / threshold as f64) * 100.0).round()) as u32
    };
    let warning_threshold = threshold.saturating_sub(WARNING_THRESHOLD_BUFFER_TOKENS);
    let error_threshold = threshold.saturating_sub(ERROR_THRESHOLD_BUFFER_TOKENS);
    let default_blocking_limit = effective_window.saturating_sub(MANUAL_COMPACT_BUFFER_TOKENS);
    let blocking_limit = warning_options
        .blocking_limit_override
        .filter(|value| *value > 0)
        .unwrap_or(default_blocking_limit);

    TokenWarningState {
        percent_left,
        warning_threshold,
        error_threshold,
        auto_compact_threshold,
        blocking_limit,
        is_above_warning_threshold: token_usage >= warning_threshold,
        is_above_error_threshold: token_usage >= error_threshold,
        is_above_auto_compact_threshold: warning_options.auto_compact_enabled
            && token_usage >= auto_compact_threshold,
        is_at_blocking_limit: token_usage >= blocking_limit,
    }
}

pub fn prompt_too_long_preflight_message(
    estimated_tokens: u32,
    model: &str,
    context_options: &ContextWindowOptions,
    max_output_options: &MaxOutputTokenOptions,
    warning_options: &TokenWarningOptions,
) -> Option<String> {
    let warning_state = calculate_token_warning_state(
        estimated_tokens,
        model,
        context_options,
        max_output_options,
        warning_options,
    );
    if !warning_state.is_at_blocking_limit {
        return None;
    }
    Some(format!(
        "Prompt is too long: estimated {estimated_tokens} context tokens exceeds the blocking limit of {} for model `{model}`. Run /compact or start a new session before retrying.",
        warning_state.blocking_limit
    ))
}

fn canonical_model_name(model: &str) -> String {
    model
        .trim()
        .trim_end_matches("[1m]")
        .trim_end_matches("[1M]")
        .trim_end_matches("[2m]")
        .trim_end_matches("[2M]")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_context_window_from_default_suffix_and_overrides() {
        assert_eq!(
            resolve_context_window("claude-3-5-sonnet", &ContextWindowOptions::default()),
            MODEL_CONTEXT_WINDOW_DEFAULT
        );
        assert_eq!(
            resolve_context_window("claude-sonnet-4-5[1m]", &ContextWindowOptions::default()),
            CONTEXT_1M_TOKENS
        );
        assert!(model_supports_1m_context(
            "claude-opus-4-7",
            &ContextWindowOptions::default()
        ));
        assert_eq!(
            resolve_context_window("claude-opus-4-7", &ContextWindowOptions::default()),
            CONTEXT_1M_TOKENS
        );
        assert_eq!(
            resolve_context_window(
                "claude-sonnet-4-5[1m]",
                &ContextWindowOptions {
                    disable_1m_context: true,
                    ..Default::default()
                }
            ),
            MODEL_CONTEXT_WINDOW_DEFAULT
        );
        assert_eq!(
            resolve_context_window(
                "claude-sonnet-4-5[1m]",
                &ContextWindowOptions {
                    max_context_tokens_override: Some(123_456),
                    ..Default::default()
                }
            ),
            123_456
        );
    }

    #[test]
    fn resolves_max_output_tokens_by_model_and_override() {
        assert_eq!(
            resolve_model_max_output_tokens("claude-3-opus").default,
            4_096
        );
        assert_eq!(
            resolve_model_max_output_tokens("claude-sonnet-4-6").upper_limit,
            128_000
        );
        assert_eq!(
            resolve_max_output_tokens(
                "claude-sonnet-4-6",
                &MaxOutputTokenOptions {
                    max_output_tokens_override: Some(200_000)
                }
            ),
            128_000
        );
    }

    #[test]
    fn calculates_effective_window_and_thresholds() {
        let context_options = ContextWindowOptions {
            auto_compact_window_override: Some(100_000),
            ..Default::default()
        };
        let max_output_options = MaxOutputTokenOptions::default();
        let warning_options = TokenWarningOptions {
            auto_compact_percent_override: Some(50),
            blocking_limit_override: Some(90_000),
            ..Default::default()
        };

        assert_eq!(
            effective_context_window_size(
                "claude-sonnet-4-5",
                &context_options,
                &max_output_options
            ),
            80_000
        );
        assert_eq!(
            auto_compact_threshold(
                "claude-sonnet-4-5",
                &context_options,
                &max_output_options,
                &warning_options
            ),
            40_000
        );
        let state = calculate_token_warning_state(
            40_000,
            "claude-sonnet-4-5",
            &context_options,
            &max_output_options,
            &warning_options,
        );
        assert_eq!(state.percent_left, 0);
        assert!(state.is_above_warning_threshold);
        assert!(state.is_above_auto_compact_threshold);
        assert!(!state.is_at_blocking_limit);
    }

    #[test]
    fn glm_4_7_default_window_reserves_output_and_manual_compact_buffer() {
        assert_eq!(
            resolve_context_window("glm-4.7", &ContextWindowOptions::default()),
            200_000
        );
        assert_eq!(
            resolve_max_output_tokens("glm-4.7", &MaxOutputTokenOptions::default()),
            32_000
        );
        assert_eq!(
            effective_context_window_size(
                "glm-4.7",
                &ContextWindowOptions::default(),
                &MaxOutputTokenOptions::default()
            ),
            180_000
        );

        let below_limit = calculate_token_warning_state(
            176_999,
            "glm-4.7",
            &ContextWindowOptions::default(),
            &MaxOutputTokenOptions::default(),
            &TokenWarningOptions::default(),
        );
        assert_eq!(below_limit.blocking_limit, 177_000);
        assert!(!below_limit.is_at_blocking_limit);

        let at_limit = calculate_token_warning_state(
            177_000,
            "glm-4.7",
            &ContextWindowOptions::default(),
            &MaxOutputTokenOptions::default(),
            &TokenWarningOptions::default(),
        );
        assert_eq!(at_limit.blocking_limit, 177_000);
        assert!(at_limit.is_at_blocking_limit);
    }

    #[test]
    fn percent_left_matches_typescript_rounding() {
        let state = calculate_token_warning_state(
            42,
            "glm-4.7",
            &ContextWindowOptions {
                auto_compact_window_override: Some(100),
                ..Default::default()
            },
            &MaxOutputTokenOptions {
                max_output_tokens_override: Some(1),
            },
            &TokenWarningOptions {
                auto_compact_enabled: false,
                ..Default::default()
            },
        );

        assert_eq!(state.percent_left, 58);
    }

    #[test]
    fn blocking_limit_uses_manual_compact_buffer_when_auto_compact_disabled() {
        let state = calculate_token_warning_state(
            188_808,
            "claude-3-5-sonnet",
            &ContextWindowOptions::default(),
            &MaxOutputTokenOptions::default(),
            &TokenWarningOptions {
                auto_compact_enabled: false,
                ..Default::default()
            },
        );

        assert_eq!(state.blocking_limit, 188_808);
        assert!(state.is_at_blocking_limit);
        assert!(!state.is_above_auto_compact_threshold);
    }

    #[test]
    fn prompt_too_long_preflight_message_reports_blocking_limit_and_model() {
        let message = prompt_too_long_preflight_message(
            177_000,
            "glm-4.7",
            &ContextWindowOptions::default(),
            &MaxOutputTokenOptions::default(),
            &TokenWarningOptions::default(),
        )
        .expect("prompt should be at the default blocking limit");

        assert!(message.contains("estimated 177000 context tokens"));
        assert!(message.contains("blocking limit of 177000"));
        assert!(message.contains("model `glm-4.7`"));
        assert!(
            prompt_too_long_preflight_message(
                176_999,
                "glm-4.7",
                &ContextWindowOptions::default(),
                &MaxOutputTokenOptions::default(),
                &TokenWarningOptions::default(),
            )
            .is_none()
        );
    }
}
