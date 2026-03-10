use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use frankclaw_core::auth::RateLimitConfig;

/// Per-IP rate limiter for authentication attempts.
///
/// Tracks failed attempts per IP with sliding window and lockout.
/// Uses a `Mutex<HashMap>` since auth attempts are infrequent
/// and contention is negligible.
pub struct AuthRateLimiter {
    config: RateLimitConfig,
    attempts: Mutex<HashMap<IpAddr, IpAttempts>>,
}

struct IpAttempts {
    count: u32,
    first_attempt: Instant,
    locked_until: Option<Instant>,
}

impl AuthRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// Check if an IP is currently locked out.
    pub fn is_locked(&self, ip: &IpAddr) -> Option<Duration> {
        let attempts = self.attempts.lock().expect("rate limiter poisoned");
        if let Some(entry) = attempts.get(ip) {
            if let Some(locked_until) = entry.locked_until {
                if Instant::now() < locked_until {
                    return Some(locked_until - Instant::now());
                }
            }
        }
        None
    }

    /// Record a failed authentication attempt.
    pub fn record_failure(&self, ip: &IpAddr) {
        let mut attempts = self.attempts.lock().expect("rate limiter poisoned");
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_secs);

        let entry = attempts.entry(*ip).or_insert(IpAttempts {
            count: 0,
            first_attempt: now,
            locked_until: None,
        });

        // Reset window if expired.
        if now.duration_since(entry.first_attempt) > window {
            entry.count = 0;
            entry.first_attempt = now;
            entry.locked_until = None;
        }

        entry.count += 1;

        if entry.count >= self.config.max_attempts {
            entry.locked_until =
                Some(now + Duration::from_secs(self.config.lockout_secs));
            tracing::warn!(%ip, lockout_secs = self.config.lockout_secs, "IP locked out after too many auth failures");
        }
    }

    /// Clear rate limit state for an IP (on successful auth).
    pub fn record_success(&self, ip: &IpAddr) {
        let mut attempts = self.attempts.lock().expect("rate limiter poisoned");
        attempts.remove(ip);
    }

    /// Periodic cleanup of expired entries to prevent unbounded growth.
    pub fn cleanup(&self) {
        let mut attempts = self.attempts.lock().expect("rate limiter poisoned");
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_secs + self.config.lockout_secs);
        attempts.retain(|_, v| now.duration_since(v.first_attempt) < window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn test_config() -> RateLimitConfig {
        RateLimitConfig {
            max_attempts: 3,
            window_secs: 60,
            lockout_secs: 300,
        }
    }

    #[test]
    fn allows_under_limit() {
        let limiter = AuthRateLimiter::new(test_config());
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));

        limiter.record_failure(&ip);
        limiter.record_failure(&ip);
        assert!(limiter.is_locked(&ip).is_none());
    }

    #[test]
    fn locks_at_limit() {
        let limiter = AuthRateLimiter::new(test_config());
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));

        for _ in 0..3 {
            limiter.record_failure(&ip);
        }
        assert!(limiter.is_locked(&ip).is_some());
    }

    #[test]
    fn success_clears() {
        let limiter = AuthRateLimiter::new(test_config());
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));

        for _ in 0..3 {
            limiter.record_failure(&ip);
        }
        limiter.record_success(&ip);
        assert!(limiter.is_locked(&ip).is_none());
    }
}
