//! Integration tests for FrankClaw's IronClaw-adopted features.
//!
//! These tests hit live APIs and exercise the full stack: failover chain with
//! circuit breaker, response caching, cost tracking, smart routing, and
//! credential leak detection.
//!
//! All tests are `#[ignore]` by default — they require real API keys.
//!
//! ```bash
//! source ~/.config/zsh/secrets
//! cargo test -p frankclaw-models --test integration -- --ignored
//! ```

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use frankclaw_core::model::{CompletionMessage, CompletionRequest, ModelProvider, StreamDelta};
use frankclaw_core::types::Role;
use frankclaw_models::{
    AnthropicProvider, FailoverChain, OpenAiProvider,
    ResponseCache, ResponseCacheConfig,
    CostGuard, CostGuardConfig,
    CircuitBreaker, CircuitBreakerConfig, CircuitState,
    model_cost, default_cost,
    classify_message, score_complexity, TaskComplexity,
};
use secrecy::SecretString;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn openai_key() -> Option<SecretString> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(SecretString::from)
}

fn openai_base_url() -> String {
    let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    if key.starts_with("sk-or-") {
        "https://openrouter.ai/api/v1".into()
    } else {
        std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".into())
    }
}

fn openai_model() -> String {
    let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    if key.starts_with("sk-or-") {
        "openai/gpt-4o-mini".into()
    } else {
        "gpt-4o-mini".into()
    }
}

fn anthropic_key() -> Option<SecretString> {
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(SecretString::from)
}

fn simple_request(model: &str, prompt: &str) -> CompletionRequest {
    CompletionRequest {
        model_id: model.into(),
        messages: vec![CompletionMessage::text(Role::User, prompt)],
        max_tokens: Some(50),
        temperature: Some(0.0),
        system: None,
        tools: Vec::new(),
        thinking_budget: None,
        parallel_tool_calls: None,
        seed: None,
        response_format: None,
        reasoning_effort: None,
    }
}

// ---------------------------------------------------------------------------
// Circuit Breaker Integration
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn circuit_breaker_closes_after_successes() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();

    let provider = Arc::new(OpenAiProvider::new("openai-cb", &base, key, vec![model.clone()])
        .expect("failed to build provider"));
    let chain = FailoverChain::new(vec![provider], 30);

    // Make a successful call — circuit should stay closed
    let request = simple_request(&model, "Reply with the word 'yes'.");
    let response = chain.complete(request, None).await;
    assert!(response.is_ok(), "completion should succeed through circuit breaker");

    let health = chain.health().await;
    assert!(health[0].healthy, "provider should be healthy after success");
}

#[tokio::test]
#[ignore]
async fn circuit_breaker_opens_on_failures() {
    let breaker = CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 2,
        recovery_timeout: Duration::from_secs(60),
        ..Default::default()
    });

    assert_eq!(breaker.circuit_state(), CircuitState::Closed);
    assert!(breaker.check_allowed());

    // Simulate failures
    breaker.record_failure();
    breaker.record_failure();

    assert_eq!(breaker.circuit_state(), CircuitState::Open);
    assert!(!breaker.check_allowed());
}

#[tokio::test]
#[ignore]
async fn failover_chain_skips_bad_provider_uses_good() {
    let oai_key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();

    // First provider: bad key (will fail)
    let bad_provider = Arc::new(OpenAiProvider::new(
        "bad-openai",
        &base,
        SecretString::from("sk-invalid-key".to_string()),
        vec![model.clone()],
    ).expect("failed to build provider"));

    // Second provider: good key (should succeed)
    let good_provider = Arc::new(OpenAiProvider::new(
        "good-openai",
        &base,
        oai_key,
        vec![model.clone()],
    ).expect("failed to build provider"));

    let chain = FailoverChain::new(vec![bad_provider, good_provider], 30);

    let request = simple_request(&model, "Reply with 'failover works'.");
    let response = chain.complete(request, None).await;
    assert!(response.is_ok(), "failover should succeed via second provider");

    let content = response.unwrap().content.to_lowercase();
    assert!(
        content.contains("failover") || content.contains("works") || !content.is_empty(),
        "should get a real response"
    );
}

// ---------------------------------------------------------------------------
// Response Cache Integration
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn cache_returns_identical_response_on_second_call() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();

    let provider = OpenAiProvider::new("openai-cache", &base, key, vec![model.clone()])
        .expect("failed to build provider");
    let cache = ResponseCache::new(ResponseCacheConfig {
        ttl: Duration::from_secs(60),
        max_entries: 10,
    });

    let request = simple_request(&model, "What is 2+2? Reply with just the number.");

    // First call — cache miss, hits API
    assert!(cache.lookup(&request).is_none(), "cache should miss on first call");

    let response = provider
        .complete(request.clone(), None)
        .await
        .expect("completion should succeed");

    cache.store(&request, &response);

    // Second call — cache hit
    let cached = cache.lookup(&request).expect("cache should hit on second call");
    assert_eq!(cached.content, response.content, "cached content should match");
    assert_eq!(
        cached.usage.input_tokens, response.usage.input_tokens,
        "cached usage should match"
    );
}

#[tokio::test]
#[ignore]
async fn cache_miss_for_different_prompts() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();

    let provider = OpenAiProvider::new("openai-cache2", &base, key, vec![model.clone()])
        .expect("failed to build provider");
    let cache = ResponseCache::new(ResponseCacheConfig::default());

    let request1 = simple_request(&model, "What is 2+2?");
    let response1 = provider
        .complete(request1.clone(), None)
        .await
        .expect("first completion should succeed");
    cache.store(&request1, &response1);

    // Different prompt — should miss
    let request2 = simple_request(&model, "What is 3+3?");
    assert!(cache.lookup(&request2).is_none(), "different prompt should miss cache");
}

// ---------------------------------------------------------------------------
// Cost Tracking Integration
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn cost_guard_tracks_real_api_usage() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();

    let provider = OpenAiProvider::new("openai-cost", &base, key, vec![model.clone()])
        .expect("failed to build provider");
    let guard = CostGuard::new(CostGuardConfig {
        max_cost_per_day_cents: Some(100_00), // $100 — won't hit this
        max_actions_per_hour: None,
    });

    // Verify we're allowed
    assert!(guard.check_allowed().await.is_ok(), "should be allowed before any calls");

    let request = simple_request(&model, "Reply with 'ok'.");
    let response = provider
        .complete(request, None)
        .await
        .expect("completion should succeed");

    // Record the usage
    let cost = guard
        .record_llm_call(
            &model,
            response.usage.input_tokens,
            response.usage.output_tokens,
        )
        .await;

    assert!(cost > 0.0, "cost should be positive for a real API call");
    eprintln!(
        "Recorded: {} input + {} output tokens = ${:.6}",
        response.usage.input_tokens, response.usage.output_tokens, cost
    );

    // Should still be allowed (well under budget)
    assert!(guard.check_allowed().await.is_ok(), "should be allowed after one call");
}

#[tokio::test]
#[ignore]
async fn cost_guard_blocks_when_budget_exceeded() {
    let guard = CostGuard::new(CostGuardConfig {
        max_cost_per_day_cents: Some(1), // 1 cent — will exceed immediately
        max_actions_per_hour: None,
    });

    // Record a big call to exceed the budget
    let cost = guard.record_llm_call("gpt-4o", 10_000, 10_000).await;
    assert!(cost > 0.01, "gpt-4o 10k tokens should cost more than 1 cent");

    let result = guard.check_allowed().await;
    assert!(result.is_err(), "should be blocked after exceeding budget");
}

#[tokio::test]
#[ignore]
async fn cost_guard_hourly_rate_limit() {
    let guard = CostGuard::new(CostGuardConfig {
        max_cost_per_day_cents: None,
        max_actions_per_hour: Some(3),
    });

    // Record 3 actions
    for _ in 0..3 {
        assert!(guard.check_allowed().await.is_ok());
        guard.record_llm_call("gpt-4o-mini", 10, 10).await;
    }

    // 4th should be blocked
    let result = guard.check_allowed().await;
    assert!(result.is_err(), "should be rate-limited after 3 actions");
}

// ---------------------------------------------------------------------------
// Smart Routing Integration
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn smart_routing_classifies_simple_greeting() {
    let complexity = classify_message("hi there!");
    assert_eq!(
        complexity,
        TaskComplexity::Simple,
        "simple greeting should classify as Simple"
    );
}

#[tokio::test]
#[ignore]
async fn smart_routing_classifies_complex_task() {
    // Use a prompt that triggers multiple scoring dimensions: reasoning words,
    // multi-step, code, precision — should score high enough for Moderate/Complex
    let breakdown = score_complexity(
        "Step by step, analyze and compare the implementation of the Raft consensus \
         algorithm in Rust vs Go. Write code examples for both, reason through the \
         trade-offs, and explain exactly how leader election handles split-brain scenarios.",
    );
    eprintln!("Complex task: total={}, tier={:?}, hints={:?}", breakdown.total, breakdown.tier, breakdown.hints);
    // With reasoning words + multi-step + code + precision triggers, this should
    // score above the Simple threshold
    let complexity: TaskComplexity = breakdown.tier.into();
    assert_ne!(
        complexity,
        TaskComplexity::Simple,
        "multi-dimensional analysis task should not be Simple (score={})",
        breakdown.total
    );
}

#[tokio::test]
#[ignore]
async fn smart_routing_routes_code_generation_higher() {
    // Code generation with explicit reasoning request and multi-step
    let breakdown = score_complexity(
        "Write a complete implementation of a thread-safe LRU cache in Rust. \
         Step by step, reason through the design choices. Include comprehensive \
         tests, error handling, and explain the trade-offs between different \
         concurrency strategies. Compare your approach with alternatives.",
    );
    eprintln!("Code gen: total={}, tier={:?}, hints={:?}", breakdown.total, breakdown.tier, breakdown.hints);
    let complexity: TaskComplexity = breakdown.tier.into();
    assert_ne!(
        complexity,
        TaskComplexity::Simple,
        "code generation with reasoning should not be Simple (score={})",
        breakdown.total
    );
}

#[tokio::test]
#[ignore]
async fn smart_routing_score_breakdown_has_detail() {
    let breakdown = score_complexity(
        "Write a comprehensive analysis of distributed consensus algorithms, \
         comparing Raft, Paxos, and PBFT. Include implementation considerations.",
    );
    assert!(breakdown.total > 0, "total score should be positive");
    assert!(
        !breakdown.hints.is_empty(),
        "breakdown should have hints explaining the score"
    );
    eprintln!(
        "Score breakdown: total={}, tier={:?}, hints={:?}",
        breakdown.total, breakdown.tier, breakdown.hints
    );
}

// ---------------------------------------------------------------------------
// Cost Table Integration
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn cost_tables_have_known_models() {
    // Verify cost tables return values for popular models
    let gpt4o = model_cost("gpt-4o");
    assert!(gpt4o.is_some(), "gpt-4o should have cost data");
    let (input, output) = gpt4o.unwrap();
    assert!(input > 0.0, "gpt-4o input cost should be positive");
    assert!(output > 0.0, "gpt-4o output cost should be positive");
    assert!(output > input, "output tokens should cost more than input tokens");

    let haiku = model_cost("claude-haiku-4-5-20251001");
    assert!(haiku.is_some(), "claude-haiku should have cost data");

    // Unknown model should return None
    let unknown = model_cost("nonexistent-model-xyz");
    assert!(unknown.is_none(), "unknown model should return None");

    // Default cost should be reasonable
    let (def_in, def_out) = default_cost();
    assert!(def_in > 0.0 && def_out > 0.0, "default costs should be positive");
}

// ---------------------------------------------------------------------------
// Anthropic Provider with System Prompt and Streaming
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn anthropic_streaming_with_system_prompt() {
    let key = anthropic_key().expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(
        "anthropic-stream-sys",
        key,
        vec!["claude-haiku-4-5-20251001".into()],
    )
    .expect("failed to build provider");

    let request = CompletionRequest {
        model_id: "claude-haiku-4-5-20251001".into(),
        messages: vec![CompletionMessage::text(Role::User, "What color is the sky?")],
        max_tokens: Some(30),
        temperature: Some(0.0),
        system: Some("You must always reply in exactly one word.".into()),
        tools: Vec::new(),
        thinking_budget: None,
        parallel_tool_calls: None,
        seed: None,
        response_format: None,
        reasoning_effort: None,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamDelta>(64);
    let response = provider
        .complete(request, Some(tx))
        .await
        .expect("streaming with system prompt should succeed");

    assert!(!response.content.is_empty());
    assert!(response.usage.input_tokens > 0);
    assert!(response.usage.output_tokens > 0);

    // Verify streaming worked
    let mut text_chunks = Vec::new();
    let mut got_done = false;
    while let Ok(delta) = rx.try_recv() {
        match delta {
            StreamDelta::Text(t) => text_chunks.push(t),
            StreamDelta::Done { .. } => got_done = true,
            _ => {}
        }
    }
    assert!(!text_chunks.is_empty(), "should receive text deltas");
    assert!(got_done, "should receive done signal");
    eprintln!("Streamed {} chunks, response: {}", text_chunks.len(), response.content);
}

// ---------------------------------------------------------------------------
// Multi-Provider Failover with Circuit Breaker + Caching + Cost
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn end_to_end_failover_with_cache_and_cost_tracking() {
    let mut providers: Vec<Arc<dyn ModelProvider>> = Vec::new();
    let mut model = String::new();

    if let Some(key) = openai_key() {
        let m = openai_model();
        model = m.clone();
        providers.push(Arc::new(OpenAiProvider::new(
            "openai",
            &openai_base_url(),
            key,
            vec![m],
        ).expect("failed to build provider")));
    }
    if let Some(key) = anthropic_key() {
        if model.is_empty() {
            model = "claude-haiku-4-5-20251001".into();
        }
        providers.push(Arc::new(AnthropicProvider::new(
            "anthropic",
            key,
            vec!["claude-haiku-4-5-20251001".into()],
        ).expect("failed to build provider")));
    }

    assert!(!providers.is_empty(), "At least one API key must be set");

    let chain = FailoverChain::new(providers, 30);
    let cache = ResponseCache::new(ResponseCacheConfig {
        ttl: Duration::from_secs(120),
        max_entries: 100,
    });
    let guard = CostGuard::new(CostGuardConfig {
        max_cost_per_day_cents: Some(500), // $5 limit
        max_actions_per_hour: Some(100),
    });

    let request = simple_request(&model, "Reply with the word 'integration'. Nothing else.");

    // 1. Pre-check: budget OK
    guard.check_allowed().await.expect("should be allowed");

    // 2. Cache miss
    assert!(cache.lookup(&request).is_none());

    // 3. Real API call through failover chain
    let response = chain
        .complete(request.clone(), None)
        .await
        .expect("should complete through failover chain");
    assert!(!response.content.is_empty());

    // 4. Cache the response
    cache.store(&request, &response);

    // 5. Track cost
    let cost = guard
        .record_llm_call(
            &model,
            response.usage.input_tokens,
            response.usage.output_tokens,
        )
        .await;
    eprintln!(
        "E2E: model={}, tokens={}+{}, cost=${:.6}",
        model, response.usage.input_tokens, response.usage.output_tokens, cost
    );

    // 6. Cache hit on same request
    let cached = cache.lookup(&request).expect("should cache hit");
    assert_eq!(cached.content, response.content);

    // 7. Still within budget
    guard.check_allowed().await.expect("should still be within budget");

    // 8. Health check
    let health = chain.health().await;
    assert!(
        health.iter().any(|h| h.healthy),
        "at least one provider should be healthy"
    );
}

// ---------------------------------------------------------------------------
// Cross-Provider Consistency
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn both_providers_agree_on_simple_math() {
    let mut results: Vec<(String, String)> = Vec::new();

    if let Some(key) = openai_key() {
        let model = openai_model();
        let provider = OpenAiProvider::new("openai-math", &openai_base_url(), key, vec![model.clone()])
            .expect("failed to build provider");
        let request = simple_request(&model, "What is 7*8? Reply with just the number.");
        if let Ok(response) = provider.complete(request, None).await {
            results.push(("openai".into(), response.content.clone()));
            eprintln!("OpenAI says: {}", response.content);
        }
    }

    if let Some(key) = anthropic_key() {
        let provider = AnthropicProvider::new(
            "anthropic-math",
            key,
            vec!["claude-haiku-4-5-20251001".into()],
        )
        .expect("failed to build provider");
        let request = simple_request(
            "claude-haiku-4-5-20251001",
            "What is 7*8? Reply with just the number.",
        );
        if let Ok(response) = provider.complete(request, None).await {
            results.push(("anthropic".into(), response.content.clone()));
            eprintln!("Anthropic says: {}", response.content);
        }
    }

    assert!(!results.is_empty(), "at least one provider must respond");
    for (name, content) in &results {
        assert!(
            content.contains("56"),
            "{name} should answer 56, got: {content}"
        );
    }
}
