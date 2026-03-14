//! Retry with exponential backoff for LLM provider calls.
//!
//! Retries transient failures (network errors, rate limits) with exponential
//! backoff and jitter. Non-transient errors (auth, model not found) fail
//! immediately.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

use std::time::Duration;

use rand::Rng;

/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting the initial call).
    pub max_retries: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self { max_retries: 3 }
    }
}

/// Compute the backoff delay for a given attempt number.
///
/// Uses exponential backoff (1s → 2s → 4s → 8s …) with ±25% jitter.
/// Minimum delay is 100ms.
///
/// - Attempt 0: ~1000ms (750–1250ms with jitter)
/// - Attempt 1: ~2000ms (1500–2500ms)
/// - Attempt 2: ~4000ms (3000–5000ms)
pub fn retry_backoff_delay(attempt: u32) -> Duration {
    let base_ms = 1000u64.saturating_mul(2u64.saturating_pow(attempt));
    let jitter_range = base_ms / 4; // ±25%
    let jitter = if jitter_range > 0 {
        let offset = rand::thread_rng().gen_range(0..=jitter_range * 2);
        offset as i64 - jitter_range as i64
    } else {
        0
    };
    let delay_ms = (base_ms as i64 + jitter).max(100) as u64;
    Duration::from_millis(delay_ms)
}

/// Whether an error message suggests a transient (retryable) failure.
///
/// Returns `true` for network errors, rate limits, and server errors.
/// Returns `false` for auth failures, context length exceeded, model not
/// found, and other permanent errors.
pub fn is_retryable_error(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    // Retryable patterns
    let retryable = [
        "timeout",
        "connection",
        "rate limit",
        "429",
        "500",
        "502",
        "503",
        "504",
        "temporary",
        "unavailable",
        "overloaded",
        "try again",
    ];
    // Non-retryable patterns (check first — these take precedence)
    let non_retryable = [
        "auth",
        "unauthorized",
        "forbidden",
        "context length",
        "model not found",
        "invalid api key",
        "not found",
        "invalid_request",
    ];

    if non_retryable.iter().any(|p| lower.contains(p)) {
        return false;
    }
    retryable.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn backoff_delay_is_within_range() {
        // Attempt 0: base 1000ms, jitter ±250ms → 750..1250
        for _ in 0..100 {
            let d = retry_backoff_delay(0).as_millis();
            assert!(d >= 750 && d <= 1250, "attempt 0 delay {d}ms out of range");
        }
    }

    #[test]
    fn backoff_delay_increases_exponentially() {
        // Average of attempt 1 should be roughly 2x attempt 0.
        let avg_0: u128 = (0..100)
            .map(|_| retry_backoff_delay(0).as_millis())
            .sum::<u128>()
            / 100;
        let avg_1: u128 = (0..100)
            .map(|_| retry_backoff_delay(1).as_millis())
            .sum::<u128>()
            / 100;
        assert!(avg_1 > avg_0, "attempt 1 avg ({avg_1}) should be > attempt 0 avg ({avg_0})");
    }

    #[test]
    fn backoff_delay_minimum_100ms() {
        // Even at attempt 0 the floor is 100ms.
        for _ in 0..100 {
            let d = retry_backoff_delay(0);
            assert!(d >= Duration::from_millis(100));
        }
    }

    #[rstest]
    #[case("connection timeout", true)]
    #[case("rate limit exceeded", true)]
    #[case("HTTP 429 Too Many Requests", true)]
    #[case("502 Bad Gateway", true)]
    #[case("server temporarily unavailable", true)]
    #[case("service overloaded, try again", true)]
    #[case("authentication failed", false)]
    #[case("401 Unauthorized", false)]
    #[case("context length exceeded", false)]
    #[case("model not found: gpt-5", false)]
    #[case("invalid api key", false)]
    fn error_retryability(#[case] msg: &str, #[case] should_retry: bool) {
        assert_eq!(is_retryable_error(msg), should_retry, "pattern: {msg}");
    }

    #[test]
    fn default_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
    }
}
