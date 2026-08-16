//! Rate-limit header parsing and retry backoff math.
//!
//! Mirrors the TypeScript client's behavior in
//! `src/services/api/withRetry.ts`:
//! - `getRetryDelay(attempt, retryAfterHeader, maxDelayMs=32000)` returns the
//!   `Retry-After` value verbatim when present, otherwise an exponential
//!   backoff `min(BASE_DELAY_MS * 2^(attempt-1), maxDelayMs)` plus up to 25%
//!   jitter.
//! - `Retry-After`, `anthropic-ratelimit-unified-*`, and `x-should-retry`
//!   headers are parsed off 429/5xx responses so the retry loop can honor the
//!   server directive.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Base retry delay (`BASE_DELAY_MS` in TypeScript).
pub const BASE_RETRY_DELAY_MS: u64 = 500;
/// Default exponential-backoff ceiling (`maxDelayMs` default in TypeScript).
pub const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 32_000;

/// Rate-limit metadata extracted from a provider HTTP error response.
///
/// All fields are optional: a server may send any subset (or none) of these
/// headers. The retry loop uses `retry_after_secs` to honor `Retry-After`, and
/// `unified_reset_unix` for window-based subscription limits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RateLimitMetadata {
    /// `Retry-After` header parsed as whole seconds (the API only sends an
    /// integer-seconds form, not the HTTP-date form, for these endpoints).
    pub retry_after_secs: Option<u64>,
    /// `anthropic-ratelimit-unified-reset`: unix timestamp (seconds) when the
    /// unified rate-limit window resets.
    pub unified_reset_unix: Option<u64>,
    /// `anthropic-ratelimit-unified-remaining`: remaining requests/tokens in
    /// the current window.
    pub unified_remaining: Option<u64>,
    /// `anthropic-ratelimit-unified-status`: e.g. `allowed`, `rejected`,
    /// `allowed_warning`.
    pub unified_status: Option<String>,
    /// `x-should-retry`: explicit server retry directive when present.
    pub should_retry: Option<bool>,
}

impl RateLimitMetadata {
    pub fn is_empty(&self) -> bool {
        self == &RateLimitMetadata::default()
    }

    /// Parse rate-limit headers from a case-insensitive lookup closure.
    ///
    /// Decoupled from `reqwest` so the parsing can be unit-tested without an
    /// HTTP server. The closure receives a lowercase header name.
    pub fn from_lookup<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let retry_after_secs = lookup("retry-after").and_then(|value| parse_u64(&value));
        let unified_reset_unix =
            lookup("anthropic-ratelimit-unified-reset").and_then(|value| parse_u64(&value));
        let unified_remaining =
            lookup("anthropic-ratelimit-unified-remaining").and_then(|value| parse_u64(&value));
        let unified_status = lookup("anthropic-ratelimit-unified-status")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let should_retry = lookup("x-should-retry").and_then(|value| match value.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });

        Self {
            retry_after_secs,
            unified_reset_unix,
            unified_remaining,
            unified_status,
            should_retry,
        }
    }

    /// Delay (ms) until the unified rate-limit window resets, if that reset is
    /// in the future. Mirrors TypeScript's `getRateLimitResetDelayMs`.
    pub fn reset_delay_ms(&self) -> Option<u64> {
        let reset = self.unified_reset_unix?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        if reset <= now {
            return None;
        }
        Some((reset - now).saturating_mul(1000))
    }
}

fn parse_u64(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

/// Compute the retry delay in milliseconds for a given 1-based `attempt`.
///
/// Mirrors `getRetryDelay` in `withRetry.ts`:
/// - When `retry_after_secs` is present, it is honored verbatim (converted to
///   ms) and bypasses both the exponential schedule and `max_delay_ms`.
/// - Otherwise the base delay is `min(BASE_RETRY_DELAY_MS * 2^(attempt-1),
///   max_delay_ms)`, plus jitter of `jitter_factor * 0.25 * base`.
///
/// `jitter_factor` is taken as a parameter (clamped to `0.0..=1.0`) so callers
/// can inject deterministic values in tests; production callers pass
/// [`default_jitter_factor`].
pub fn retry_delay_ms(
    attempt: usize,
    retry_after_secs: Option<u64>,
    max_delay_ms: u64,
    jitter_factor: f64,
) -> u64 {
    retry_delay_ms_with_base(
        attempt,
        retry_after_secs,
        BASE_RETRY_DELAY_MS,
        max_delay_ms,
        jitter_factor,
    )
}

/// Like [`retry_delay_ms`] but with a caller-supplied base delay so the backoff
/// schedule can be tuned (or collapsed to 0 in tests) via
/// `CLAUDE_CODE_RETRY_BASE_DELAY_MS`.
pub fn retry_delay_ms_with_base(
    attempt: usize,
    retry_after_secs: Option<u64>,
    base_delay_ms: u64,
    max_delay_ms: u64,
    jitter_factor: f64,
) -> u64 {
    if let Some(secs) = retry_after_secs {
        return secs.saturating_mul(1000);
    }

    let exponent = u32::try_from(attempt.saturating_sub(1).min(32))
        .expect("retry exponent is clamped to at most 32");
    let scaled = base_delay_ms.saturating_mul(1u64 << exponent);
    let base = scaled.min(max_delay_ms);
    let jitter_factor = if jitter_factor.is_nan() {
        0.0
    } else {
        jitter_factor.clamp(0.0, 1.0)
    };
    let jitter = Duration::from_millis(base)
        .mul_f64(jitter_factor * 0.25)
        .as_millis();
    let jitter = u64::try_from(jitter).unwrap_or(u64::MAX);
    base.saturating_add(jitter)
}

/// A process-local pseudo-random jitter factor in `0.0..1.0`.
///
/// Avoids pulling in the `rand` crate for a non-security-sensitive jitter; the
/// sub-microsecond clock noise is sufficient to de-correlate concurrent retry
/// schedules.
pub fn default_jitter_factor() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect();
        move |key: &str| map.get(&key.to_ascii_lowercase()).cloned()
    }

    #[test]
    fn retry_after_header_is_honored_verbatim() {
        // 7 second Retry-After must win over the exponential schedule and the
        // max-delay ceiling.
        assert_eq!(
            retry_delay_ms(1, Some(7), DEFAULT_MAX_RETRY_DELAY_MS, 0.0),
            7_000
        );
        assert_eq!(retry_delay_ms(5, Some(120), 1_000, 1.0), 120_000);
    }

    #[test]
    fn exponential_backoff_sequence_without_jitter() {
        // 500, 1000, 2000, 4000, ... with zero jitter.
        assert_eq!(
            retry_delay_ms(1, None, DEFAULT_MAX_RETRY_DELAY_MS, 0.0),
            500
        );
        assert_eq!(
            retry_delay_ms(2, None, DEFAULT_MAX_RETRY_DELAY_MS, 0.0),
            1_000
        );
        assert_eq!(
            retry_delay_ms(3, None, DEFAULT_MAX_RETRY_DELAY_MS, 0.0),
            2_000
        );
        assert_eq!(
            retry_delay_ms(4, None, DEFAULT_MAX_RETRY_DELAY_MS, 0.0),
            4_000
        );
    }

    #[test]
    fn backoff_is_capped_at_max_delay() {
        // 2^10 * 500 = 512000 but capped at 32000.
        assert_eq!(
            retry_delay_ms(11, None, DEFAULT_MAX_RETRY_DELAY_MS, 0.0),
            DEFAULT_MAX_RETRY_DELAY_MS
        );
        // A very large attempt count must not overflow.
        assert_eq!(
            retry_delay_ms(1000, None, DEFAULT_MAX_RETRY_DELAY_MS, 0.0),
            DEFAULT_MAX_RETRY_DELAY_MS
        );
    }

    #[test]
    fn jitter_adds_up_to_25_percent() {
        let base = retry_delay_ms(3, None, DEFAULT_MAX_RETRY_DELAY_MS, 0.0);
        let full_jitter = retry_delay_ms(3, None, DEFAULT_MAX_RETRY_DELAY_MS, 1.0);
        assert_eq!(base, 2_000);
        // 2000 + 0.25 * 2000 = 2500.
        assert_eq!(full_jitter, 2_500);
        // Out-of-range jitter factors are clamped.
        assert_eq!(
            retry_delay_ms(3, None, DEFAULT_MAX_RETRY_DELAY_MS, 5.0),
            2_500
        );
        assert_eq!(
            retry_delay_ms(3, None, DEFAULT_MAX_RETRY_DELAY_MS, -1.0),
            2_000
        );
        assert_eq!(
            retry_delay_ms(3, None, DEFAULT_MAX_RETRY_DELAY_MS, f64::NAN),
            2_000
        );
    }

    #[test]
    fn extreme_delays_saturate_instead_of_overflowing() {
        assert_eq!(retry_delay_ms(1, Some(u64::MAX), u64::MAX, 1.0), u64::MAX);
        assert_eq!(
            retry_delay_ms_with_base(1, None, u64::MAX, u64::MAX, 1.0),
            u64::MAX
        );
    }

    #[test]
    fn parses_retry_after_header() {
        let meta = RateLimitMetadata::from_lookup(lookup_from(&[("Retry-After", "42")]));
        assert_eq!(meta.retry_after_secs, Some(42));
        assert!(meta.unified_reset_unix.is_none());
    }

    #[test]
    fn parses_anthropic_ratelimit_headers() {
        let meta = RateLimitMetadata::from_lookup(lookup_from(&[
            ("anthropic-ratelimit-unified-reset", "1735689600"),
            ("anthropic-ratelimit-unified-remaining", "0"),
            ("anthropic-ratelimit-unified-status", "rejected"),
        ]));
        assert_eq!(meta.unified_reset_unix, Some(1_735_689_600));
        assert_eq!(meta.unified_remaining, Some(0));
        assert_eq!(meta.unified_status.as_deref(), Some("rejected"));
    }

    #[test]
    fn parses_should_retry_header() {
        let yes = RateLimitMetadata::from_lookup(lookup_from(&[("x-should-retry", "true")]));
        assert_eq!(yes.should_retry, Some(true));
        let no = RateLimitMetadata::from_lookup(lookup_from(&[("x-should-retry", "false")]));
        assert_eq!(no.should_retry, Some(false));
        let other = RateLimitMetadata::from_lookup(lookup_from(&[("x-should-retry", "maybe")]));
        assert_eq!(other.should_retry, None);
    }

    #[test]
    fn missing_headers_yield_empty_metadata() {
        let meta =
            RateLimitMetadata::from_lookup(lookup_from(&[("content-type", "application/json")]));
        assert!(meta.is_empty());
    }

    #[test]
    fn reset_delay_is_none_for_past_timestamps() {
        let meta = RateLimitMetadata {
            unified_reset_unix: Some(1),
            ..Default::default()
        };
        assert!(meta.reset_delay_ms().is_none());
    }

    #[test]
    fn reset_delay_saturates_for_extreme_future_timestamp() {
        let meta = RateLimitMetadata {
            unified_reset_unix: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(meta.reset_delay_ms(), Some(u64::MAX));
    }
}
