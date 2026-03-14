//! API key rotation for model providers.
//!
//! Manages multiple API keys per provider with round-robin selection,
//! exponential backoff on failure, and automatic recovery when cooldowns expire.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use secrecy::SecretString;

/// Maximum cooldown duration (1 hour).
const MAX_COOLDOWN: Duration = Duration::from_secs(3600);

/// Base cooldown duration for transient failures (1 minute).
const BASE_COOLDOWN: Duration = Duration::from_secs(60);

/// Reason a key was marked as failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureReason {
    /// Rate limit hit (429).
    RateLimit,
    /// Authentication failed (401/403).
    AuthError,
    /// Server overloaded (503).
    Overloaded,
    /// Request timed out.
    Timeout,
    /// Billing/quota issue (402).
    Billing,
    /// Unknown/other error.
    Unknown,
}

/// Usage stats for a single API key.
#[derive(Debug)]
struct KeyStats {
    last_used: Option<Instant>,
    cooldown_until: Option<Instant>,
    consecutive_failures: u32,
    last_failure_reason: Option<FailureReason>,
}

impl KeyStats {
    fn new() -> Self {
        Self {
            last_used: None,
            cooldown_until: None,
            consecutive_failures: 0,
            last_failure_reason: None,
        }
    }

    fn is_available(&self, now: Instant) -> bool {
        self.cooldown_until.is_none_or(|until| now >= until)
    }

    fn cooldown_remaining(&self, now: Instant) -> Option<Duration> {
        let until = self.cooldown_until?;
        until.checked_duration_since(now)
    }
}

/// Manages multiple API keys for a single provider with rotation.
pub struct KeyRotator {
    /// Keys in insertion order.
    keys: Vec<SecretString>,
    /// Stats indexed by key position.
    stats: Vec<KeyStats>,
}

impl KeyRotator {
    /// Create a rotator with a set of API keys.
    ///
    /// # Panics
    ///
    /// Panics if `keys` is empty.
    pub fn new(keys: Vec<SecretString>) -> Self {
        assert!(!keys.is_empty(), "KeyRotator requires at least one key");
        let stats = keys.iter().map(|_| KeyStats::new()).collect();
        Self { keys, stats }
    }

    /// Select the best available key using round-robin.
    ///
    /// Returns `None` if all keys are in cooldown.
    pub fn select(&mut self) -> Option<&SecretString> {
        let now = Instant::now();

        // Clear expired cooldowns (circuit-breaker half-open → closed).
        for stat in &mut self.stats {
            if stat.cooldown_until.is_some_and(|until| now >= until) {
                stat.cooldown_until = None;
                stat.consecutive_failures = 0;
                stat.last_failure_reason = None;
            }
        }

        // Find the available key with the oldest `last_used` (round-robin).
        let best = self
            .stats
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_available(now))
            .min_by_key(|(_, s)| s.last_used);

        if let Some((idx, _)) = best {
            self.stats[idx].last_used = Some(now);
            Some(&self.keys[idx])
        } else {
            None
        }
    }

    /// Mark the most recently used key as having succeeded.
    /// Resets failure counters.
    pub fn mark_success(&mut self) {
        if let Some(stat) = self.most_recent_stat_mut() {
            stat.consecutive_failures = 0;
            stat.cooldown_until = None;
            stat.last_failure_reason = None;
        }
    }

    /// Mark the most recently used key as having failed.
    /// Applies exponential backoff cooldown.
    pub fn mark_failure(&mut self, reason: FailureReason) {
        let now = Instant::now();
        if let Some(stat) = self.most_recent_stat_mut() {
            stat.consecutive_failures += 1;
            stat.last_failure_reason = Some(reason);

            let cooldown = compute_cooldown(stat.consecutive_failures, reason);
            stat.cooldown_until = Some(now + cooldown);
        }
    }

    /// Number of keys currently available (not in cooldown).
    pub fn available_count(&self) -> usize {
        let now = Instant::now();
        self.stats.iter().filter(|s| s.is_available(now)).count()
    }

    /// Total number of keys.
    pub fn total_count(&self) -> usize {
        self.keys.len()
    }

    /// Get the soonest cooldown expiry among cooled-down keys.
    pub fn next_available_in(&self) -> Option<Duration> {
        let now = Instant::now();
        self.stats
            .iter()
            .filter_map(|s| s.cooldown_remaining(now))
            .min()
    }

    fn most_recent_stat_mut(&mut self) -> Option<&mut KeyStats> {
        // Find the key with the most recent `last_used`.
        self.stats
            .iter_mut()
            .filter(|s| s.last_used.is_some())
            .max_by_key(|s| s.last_used)
    }
}

/// Compute cooldown duration using exponential backoff.
///
/// Auth/billing errors use longer base cooldowns than transient errors.
fn compute_cooldown(consecutive_failures: u32, reason: FailureReason) -> Duration {
    let base = match reason {
        FailureReason::AuthError | FailureReason::Billing => Duration::from_secs(300), // 5 min
        FailureReason::RateLimit
        | FailureReason::Overloaded
        | FailureReason::Timeout
        | FailureReason::Unknown => BASE_COOLDOWN,
    };

    // Exponential: base * 5^(failures-1), capped at MAX_COOLDOWN.
    let exponent = consecutive_failures.saturating_sub(1).min(5);
    let multiplier = 5u64.pow(exponent);
    #[expect(clippy::cast_possible_truncation, reason = "multiplier is at most 5^5 = 3125, which fits in u32")]
    let cooldown = base.saturating_mul(multiplier as u32);

    cooldown.min(MAX_COOLDOWN)
}

/// Manages key rotators for multiple providers.
pub struct ProviderKeyManager {
    rotators: HashMap<String, KeyRotator>,
}

impl ProviderKeyManager {
    pub fn new() -> Self {
        Self {
            rotators: HashMap::new(),
        }
    }

    /// Register keys for a provider.
    pub fn register(&mut self, provider: impl Into<String>, keys: Vec<SecretString>) {
        if !keys.is_empty() {
            self.rotators.insert(provider.into(), KeyRotator::new(keys));
        }
    }

    /// Get or create a rotator for a provider.
    pub fn rotator_mut(&mut self, provider: &str) -> Option<&mut KeyRotator> {
        self.rotators.get_mut(provider)
    }

    /// Select a key for the given provider.
    pub fn select(&mut self, provider: &str) -> Option<&SecretString> {
        let r = self.rotators.get_mut(provider)?;
        r.select()
    }

    /// Mark the most recently used key for a provider as successful.
    pub fn mark_success(&mut self, provider: &str) {
        if let Some(r) = self.rotators.get_mut(provider) {
            r.mark_success();
        }
    }

    /// Mark the most recently used key for a provider as failed.
    pub fn mark_failure(&mut self, provider: &str, reason: FailureReason) {
        if let Some(r) = self.rotators.get_mut(provider) {
            r.mark_failure(reason);
        }
    }

    /// List registered providers.
    pub fn providers(&self) -> Vec<&str> {
        self.rotators.keys().map(std::string::String::as_str).collect()
    }
}

impl Default for ProviderKeyManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn make_keys(n: usize) -> Vec<SecretString> {
        (0..n)
            .map(|i| SecretString::from(format!("key-{i}")))
            .collect()
    }

    #[test]
    fn select_round_robins() {
        let mut rotator = KeyRotator::new(make_keys(3));

        let k1 = rotator.select().unwrap().expose_secret().to_string();
        let k2 = rotator.select().unwrap().expose_secret().to_string();
        let k3 = rotator.select().unwrap().expose_secret().to_string();

        // All three keys should be different (round-robin).
        assert_ne!(k1, k2);
        assert_ne!(k2, k3);
        assert_ne!(k1, k3);

        // Fourth selection wraps around.
        let k4 = rotator.select().unwrap().expose_secret().to_string();
        assert_eq!(k1, k4);
    }

    #[test]
    fn failed_key_enters_cooldown() {
        let mut rotator = KeyRotator::new(make_keys(2));

        // Use first key and mark it failed.
        let k1 = rotator.select().unwrap().expose_secret().to_string();
        rotator.mark_failure(FailureReason::RateLimit);

        // Next selection should skip the failed key.
        let k2 = rotator.select().unwrap().expose_secret().to_string();
        assert_ne!(k1, k2);

        // With only one key available, it returns the same key.
        let k3 = rotator.select().unwrap().expose_secret().to_string();
        assert_eq!(k2, k3);
    }

    #[test]
    fn all_keys_in_cooldown_returns_none() {
        let mut rotator = KeyRotator::new(make_keys(1));

        rotator.select().unwrap();
        rotator.mark_failure(FailureReason::RateLimit);

        assert!(rotator.select().is_none());
        assert_eq!(rotator.available_count(), 0);
    }

    #[test]
    fn success_resets_failure_counters() {
        let mut rotator = KeyRotator::new(make_keys(1));

        rotator.select().unwrap();
        rotator.mark_failure(FailureReason::Timeout);
        assert_eq!(rotator.available_count(), 0);

        // Simulate cooldown expiry by directly clearing stats.
        rotator.stats[0].cooldown_until = None;
        rotator.stats[0].consecutive_failures = 3;

        rotator.select().unwrap();
        rotator.mark_success();

        assert_eq!(rotator.stats[0].consecutive_failures, 0);
        assert!(rotator.stats[0].cooldown_until.is_none());
    }

    #[test]
    fn exponential_backoff_increases_cooldown() {
        let c1 = compute_cooldown(1, FailureReason::RateLimit);
        let c2 = compute_cooldown(2, FailureReason::RateLimit);
        let c3 = compute_cooldown(3, FailureReason::RateLimit);

        // Each step should be longer.
        assert!(c2 > c1);
        assert!(c3 > c2);
    }

    #[test]
    fn cooldown_capped_at_max() {
        let c = compute_cooldown(100, FailureReason::RateLimit);
        assert!(c <= MAX_COOLDOWN);
    }

    #[test]
    fn auth_errors_have_longer_base_cooldown() {
        let transient = compute_cooldown(1, FailureReason::RateLimit);
        let auth = compute_cooldown(1, FailureReason::AuthError);
        assert!(auth > transient);
    }

    #[test]
    fn available_count_reflects_cooldowns() {
        let mut rotator = KeyRotator::new(make_keys(3));
        assert_eq!(rotator.available_count(), 3);

        rotator.select().unwrap();
        rotator.mark_failure(FailureReason::Timeout);
        assert_eq!(rotator.available_count(), 2);
    }

    #[test]
    fn next_available_in_when_no_cooldowns() {
        let rotator = KeyRotator::new(make_keys(2));
        assert!(rotator.next_available_in().is_none());
    }

    #[test]
    fn next_available_in_returns_soonest() {
        let mut rotator = KeyRotator::new(make_keys(2));

        rotator.select().unwrap();
        rotator.mark_failure(FailureReason::RateLimit);

        let remaining = rotator.next_available_in();
        assert!(remaining.is_some());
        assert!(remaining.unwrap() <= MAX_COOLDOWN);
    }

    #[test]
    fn provider_key_manager_basic_flow() {
        let mut mgr = ProviderKeyManager::new();
        mgr.register("openai", make_keys(2));
        mgr.register("anthropic", make_keys(1));

        assert!(mgr.select("openai").is_some());
        assert!(mgr.select("anthropic").is_some());
        assert!(mgr.select("unknown").is_none());
    }

    #[test]
    fn provider_key_manager_failure_tracking() {
        let mut mgr = ProviderKeyManager::new();
        mgr.register("openai", make_keys(1));

        mgr.select("openai").unwrap();
        mgr.mark_failure("openai", FailureReason::RateLimit);

        // Key should be in cooldown.
        assert!(mgr.select("openai").is_none());
    }

    #[test]
    #[should_panic(expected = "requires at least one key")]
    fn empty_keys_panics() {
        KeyRotator::new(vec![]);
    }
}
