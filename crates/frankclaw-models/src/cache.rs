//! In-memory LLM response cache with TTL and LRU eviction.
//!
//! Caches `CompletionResponse` keyed by a SHA-256 hash of the request
//! (model, messages, max_tokens, temperature, system prompt). Requests with
//! tool definitions are never cached since tool calls can trigger side effects.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tracing::trace;

use frankclaw_core::model::{CompletionRequest, CompletionResponse};

/// How often (in requests) to emit a cache statistics log line.
const STATS_LOG_EVERY_N: u64 = 100;

/// Configuration for the response cache.
#[derive(Debug, Clone)]
pub struct ResponseCacheConfig {
    /// Time-to-live for cache entries.
    pub ttl: Duration,
    /// Maximum number of cached entries before LRU eviction.
    pub max_entries: usize,
}

impl Default for ResponseCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(3600), // 1 hour
            max_entries: 1000,
        }
    }
}

struct CacheEntry {
    response: CompletionResponse,
    created_at: Instant,
    last_accessed: Instant,
    hit_count: u64,
}

/// In-memory response cache.
///
/// Thread-safe via `std::sync::Mutex` (never held across `.await`).
/// Use `lookup()` before sending a request and `store()` after a successful response.
pub struct ResponseCache {
    cache: Mutex<HashMap<String, CacheEntry>>,
    config: ResponseCacheConfig,
    request_count: AtomicU64,
    total_hit_count: AtomicU64,
}

impl ResponseCache {
    pub fn new(config: ResponseCacheConfig) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            config,
            request_count: AtomicU64::new(0),
            total_hit_count: AtomicU64::new(0),
        }
    }

    /// Look up a cached response for the given request.
    ///
    /// Returns `None` if the request has tool definitions (tool calls should
    /// never be replayed from cache), if there is no cache entry, or if the
    /// entry has expired.
    pub fn lookup(&self, request: &CompletionRequest) -> Option<CompletionResponse> {
        // Never cache requests with tools — tool calls have side effects.
        if !request.tools.is_empty() {
            return None;
        }

        let key = cache_key(request);
        let now = Instant::now();
        let req_no = self.request_count.fetch_add(1, Ordering::Relaxed) + 1;

        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(entry) = guard.get_mut(&key) {
            if now.duration_since(entry.created_at) < self.config.ttl {
                entry.last_accessed = now;
                entry.hit_count += 1;
                let response = entry.response.clone();
                let total_hits = self.total_hit_count.fetch_add(1, Ordering::Relaxed) + 1;
                trace!(hits = entry.hit_count, "response cache hit");
                Self::maybe_log_stats(&guard, req_no, total_hits);
                return Some(response);
            }
            // Expired
            guard.remove(&key);
        }

        let total_hits = self.total_hit_count.load(Ordering::Relaxed);
        Self::maybe_log_stats(&guard, req_no, total_hits);
        None
    }

    /// Store a response in the cache.
    ///
    /// Skips storage if the request has tool definitions.
    pub fn store(&self, request: &CompletionRequest, response: &CompletionResponse) {
        if !request.tools.is_empty() {
            return;
        }

        let key = cache_key(request);
        let now = Instant::now();

        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());

        // Evict expired entries
        let ttl = self.config.ttl;
        guard.retain(|_, entry| now.duration_since(entry.created_at) < ttl);

        // LRU eviction if over capacity
        while guard.len() >= self.config.max_entries {
            let oldest_key = guard
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest_key {
                guard.remove(&k);
            } else {
                break;
            }
        }

        guard.insert(
            key,
            CacheEntry {
                response: response.clone(),
                created_at: now,
                last_accessed: now,
                hit_count: 0,
            },
        );
    }

    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// Total cache hits since creation (never decremented on eviction).
    pub fn total_hits(&self) -> u64 {
        self.total_hit_count.load(Ordering::Relaxed)
    }

    /// Clear all cached entries.
    pub fn clear(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn maybe_log_stats(guard: &HashMap<String, CacheEntry>, req_no: u64, total_hits: u64) {
        if req_no > 0 && req_no.is_multiple_of(STATS_LOG_EVERY_N) {
            let hit_rate = total_hits as f64 / req_no as f64 * 100.0;
            tracing::info!(
                total_requests = req_no,
                total_hits,
                hit_rate_pct = format!("{hit_rate:.1}"),
                entry_count = guard.len(),
                "LLM response cache statistics"
            );
        }
    }
}

/// Build a deterministic cache key from a completion request.
///
/// Hashes the model, messages, max_tokens, temperature, and system prompt
/// via SHA-256. Two requests with identical content produce the same key.
fn cache_key(request: &CompletionRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.model_id.as_bytes());
    hasher.update(b"|");

    // Hash messages deterministically
    for msg in &request.messages {
        hasher.update(format!("{:?}", msg.role).as_bytes());
        hasher.update(b":");
        hasher.update(msg.content.as_bytes());
        hasher.update(b"|");
    }

    if let Some(max_tokens) = request.max_tokens {
        hasher.update(max_tokens.to_le_bytes());
    }
    hasher.update(b"|");
    if let Some(temp) = request.temperature {
        hasher.update(temp.to_le_bytes());
    }
    hasher.update(b"|");
    if let Some(ref system) = request.system {
        hasher.update(system.as_bytes());
    }

    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::model::{CompletionMessage, FinishReason, ToolDef, Usage};
    use frankclaw_core::types::Role;
    use rstest::{fixture, rstest};

    #[fixture]
    fn cache() -> ResponseCache {
        ResponseCache::new(ResponseCacheConfig::default())
    }

    #[fixture]
    fn simple_request() -> CompletionRequest {
        CompletionRequest {
            model_id: "test-model".into(),
            messages: vec![CompletionMessage::text(Role::User, "hello")],
            ..Default::default()
        }
    }

    #[fixture]
    fn different_request() -> CompletionRequest {
        CompletionRequest {
            model_id: "test-model".into(),
            messages: vec![CompletionMessage::text(Role::User, "goodbye")],
            ..Default::default()
        }
    }

    #[fixture]
    fn tool_request() -> CompletionRequest {
        CompletionRequest {
            model_id: "test-model".into(),
            messages: vec![CompletionMessage::text(Role::User, "use tool")],
            tools: vec![ToolDef {
                name: "test_tool".into(),
                description: "a tool".into(),
                parameters: serde_json::json!({}),
                risk_level: Default::default(),
            }],
            ..Default::default()
        }
    }

    #[fixture]
    fn dummy_response() -> CompletionResponse {
        CompletionResponse {
            content: "cached response".into(),
            tool_calls: vec![],
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            finish_reason: FinishReason::Stop,
        }
    }

    #[rstest]
    fn cache_key_is_deterministic(simple_request: CompletionRequest) {
        let k1 = cache_key(&simple_request);
        let k2 = cache_key(&simple_request);
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64); // SHA-256 hex
    }

    #[rstest]
    fn cache_key_varies_by_model(mut simple_request: CompletionRequest) {
        let mut req_b = simple_request.clone();
        req_b.model_id = "other-model".into();
        assert_ne!(cache_key(&simple_request), cache_key(&req_b));
        simple_request.model_id = req_b.model_id.clone();
        assert_eq!(cache_key(&simple_request), cache_key(&req_b));
    }

    #[rstest]
    fn cache_key_varies_by_messages(
        simple_request: CompletionRequest,
        different_request: CompletionRequest,
    ) {
        assert_ne!(
            cache_key(&simple_request),
            cache_key(&different_request)
        );
    }

    #[rstest]
    fn cache_key_varies_by_temperature(simple_request: CompletionRequest) {
        let mut req_a = simple_request.clone();
        req_a.temperature = Some(0.0);
        let mut req_b = simple_request;
        req_b.temperature = Some(1.0);
        assert_ne!(cache_key(&req_a), cache_key(&req_b));
    }

    #[rstest]
    fn cache_key_varies_by_max_tokens(simple_request: CompletionRequest) {
        let mut req_a = simple_request.clone();
        req_a.max_tokens = Some(100);
        let mut req_b = simple_request;
        req_b.max_tokens = Some(500);
        assert_ne!(cache_key(&req_a), cache_key(&req_b));
    }

    #[rstest]
    fn cache_key_varies_by_system_prompt(simple_request: CompletionRequest) {
        let mut req_a = simple_request.clone();
        req_a.system = Some("system A".into());
        let mut req_b = simple_request;
        req_b.system = Some("system B".into());
        assert_ne!(cache_key(&req_a), cache_key(&req_b));
    }

    #[rstest]
    fn cache_hit_returns_stored_response(
        cache: ResponseCache,
        simple_request: CompletionRequest,
        dummy_response: CompletionResponse,
    ) {
        assert!(cache.lookup(&simple_request).is_none());
        cache.store(&simple_request, &dummy_response);
        let cached = cache.lookup(&simple_request).expect("should hit cache");
        assert_eq!(cached.content, "cached response");
        assert_eq!(cache.total_hits(), 1);
    }

    #[rstest]
    fn different_messages_get_different_entries(
        cache: ResponseCache,
        simple_request: CompletionRequest,
        different_request: CompletionRequest,
        dummy_response: CompletionResponse,
    ) {
        cache.store(&simple_request, &dummy_response);
        cache.store(&different_request, &dummy_response);
        assert_eq!(cache.len(), 2);
    }

    #[rstest]
    fn tool_requests_are_never_cached(
        cache: ResponseCache,
        tool_request: CompletionRequest,
        dummy_response: CompletionResponse,
    ) {
        cache.store(&tool_request, &dummy_response);
        assert!(cache.is_empty(), "tool requests should not be stored");
        assert!(cache.lookup(&tool_request).is_none());
    }

    #[rstest]
    fn expired_entries_are_evicted(
        simple_request: CompletionRequest,
        dummy_response: CompletionResponse,
    ) {
        let cache = ResponseCache::new(ResponseCacheConfig {
            ttl: Duration::from_millis(1),
            max_entries: 100,
        });

        cache.store(&simple_request, &dummy_response);
        assert_eq!(cache.len(), 1);

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(10));

        // Should be a cache miss now
        assert!(cache.lookup(&simple_request).is_none());
    }

    #[rstest]
    fn lru_eviction_removes_oldest(
        simple_request: CompletionRequest,
        different_request: CompletionRequest,
        dummy_response: CompletionResponse,
    ) {
        let cache = ResponseCache::new(ResponseCacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 2,
        });

        cache.store(&simple_request, &dummy_response);
        cache.store(&different_request, &dummy_response);
        assert_eq!(cache.len(), 2);

        // Third entry should evict the oldest
        let third = CompletionRequest {
            model_id: "test-model".into(),
            messages: vec![CompletionMessage::text(Role::User, "third")],
            ..Default::default()
        };
        cache.store(&third, &dummy_response);
        assert_eq!(cache.len(), 2);
    }

    #[rstest]
    fn clear_empties_cache(
        cache: ResponseCache,
        simple_request: CompletionRequest,
        dummy_response: CompletionResponse,
    ) {
        cache.store(&simple_request, &dummy_response);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[rstest]
    fn total_hits_survives_eviction(
        simple_request: CompletionRequest,
        different_request: CompletionRequest,
        dummy_response: CompletionResponse,
    ) {
        let cache = ResponseCache::new(ResponseCacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 1,
        });

        // Populate and score a hit
        cache.store(&simple_request, &dummy_response);
        cache.lookup(&simple_request);
        assert_eq!(cache.total_hits(), 1);

        // Evict by adding a different entry
        cache.store(&different_request, &dummy_response);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.total_hits(), 1, "hit count survives eviction");
    }

    #[test]
    fn default_config_is_reasonable() {
        let cfg = ResponseCacheConfig::default();
        assert_eq!(cfg.ttl, Duration::from_secs(3600));
        assert_eq!(cfg.max_entries, 1000);
    }
}
