//! Live smoke tests against real AI provider APIs.
//!
//! These tests are `#[ignore]` by default — they require real API keys and make
//! real HTTP requests that cost money. Run them explicitly:
//!
//! ```bash
//! # Run all smoke tests:
//! cargo test -p frankclaw-models --test smoke -- --ignored
//!
//! # Run only OpenAI tests:
//! cargo test -p frankclaw-models --test smoke openai -- --ignored
//!
//! # Run only Anthropic tests:
//! cargo test -p frankclaw-models --test smoke anthropic -- --ignored
//! ```
//!
//! ## Required environment variables
//!
//! | Provider   | Env var              | Example                    |
//! |------------|----------------------|----------------------------|
//! | OpenAI     | `OPENAI_API_KEY`     | `sk-proj-...`              |
//! | Anthropic  | `ANTHROPIC_API_KEY`  | `sk-ant-api03-...`         |
//! | Ollama     | (none)               | Must be running on :11434  |
//!
//! ## Quick setup
//!
//! ```bash
//! cp .env.smoke.example .env.smoke
//! # Edit .env.smoke with your keys
//! source .env.smoke
//! cargo test -p frankclaw-models --test smoke -- --ignored
//! ```

#![forbid(unsafe_code)]

use frankclaw_core::model::{CompletionMessage, CompletionRequest, ModelProvider, StreamDelta};
use frankclaw_core::types::Role;
use frankclaw_models::{AnthropicProvider, FailoverChain, OllamaProvider, OpenAiProvider};
use secrecy::SecretString;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn openai_key() -> Option<SecretString> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(SecretString::from)
}

/// Returns the OpenAI-compatible base URL. If OPENAI_API_KEY looks like an
/// OpenRouter key (sk-or-*), use the OpenRouter endpoint automatically.
fn openai_base_url() -> String {
    let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    if key.starts_with("sk-or-") {
        "https://openrouter.ai/api/v1".into()
    } else {
        std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".into())
    }
}

/// Pick a model that works with the detected endpoint.
fn openai_model() -> String {
    let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    if key.starts_with("sk-or-") {
        // OpenRouter model IDs use provider prefix
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

fn ollama_available() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:11434".parse().expect("valid addr"),
        std::time::Duration::from_secs(2),
    )
    .is_ok()
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
// OpenAI smoke tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn openai_health_check() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();
    eprintln!("Using endpoint: {base}, model: {model}");

    let provider = OpenAiProvider::new("openai-smoke", &base, key, vec![model])
        .expect("failed to build provider");
    assert!(provider.health().await, "OpenAI-compatible health check should pass");
}

#[tokio::test]
#[ignore]
async fn openai_list_models() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let provider = OpenAiProvider::new("openai-smoke", &base, key, vec![openai_model()])
        .expect("failed to build provider");

    let models = provider.list_models().await.expect("should list models");
    assert!(!models.is_empty(), "should return at least one model");
    eprintln!("Listed {} models", models.len());
}

#[tokio::test]
#[ignore]
async fn openai_simple_completion() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();
    eprintln!("Using endpoint: {base}, model: {model}");

    let provider = OpenAiProvider::new("openai-smoke", &base, key, vec![model.clone()])
        .expect("failed to build provider");

    let request = simple_request(&model, "Reply with exactly the word 'pong'. Nothing else.");
    let response = provider
        .complete(request, None)
        .await
        .expect("completion should succeed");

    assert!(!response.content.is_empty(), "response should have content");
    assert!(
        response.content.to_lowercase().contains("pong"),
        "response should contain 'pong', got: {}",
        response.content
    );
    assert!(response.usage.input_tokens > 0, "should report input tokens");
    assert!(response.usage.output_tokens > 0, "should report output tokens");
}

#[tokio::test]
#[ignore]
async fn openai_streaming_completion() {
    let key = openai_key().expect("OPENAI_API_KEY must be set");
    let base = openai_base_url();
    let model = openai_model();
    let provider = OpenAiProvider::new("openai-smoke", &base, key, vec![model.clone()])
        .expect("failed to build provider");

    let request = simple_request(&model, "Reply with exactly 'hello world'.");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamDelta>(64);

    let response = provider
        .complete(request, Some(tx))
        .await
        .expect("streaming completion should succeed");

    assert!(!response.content.is_empty());

    // Drain the channel and verify we got text deltas + done
    let mut got_text = false;
    let mut got_done = false;
    while let Ok(delta) = rx.try_recv() {
        match delta {
            StreamDelta::Text(_) => got_text = true,
            StreamDelta::Done { .. } => got_done = true,
            _ => {}
        }
    }
    assert!(got_text, "should receive text deltas");
    assert!(got_done, "should receive done signal");
}

// ---------------------------------------------------------------------------
// Anthropic smoke tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn anthropic_health_check() {
    let key = anthropic_key().expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(
        "anthropic-smoke",
        key,
        vec!["claude-haiku-4-5-20251001".into()],
    )
    .expect("failed to build provider");

    assert!(provider.health().await, "Anthropic health check should pass");
}

#[tokio::test]
#[ignore]
async fn anthropic_list_models() {
    let key = anthropic_key().expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(
        "anthropic-smoke",
        key,
        vec!["claude-haiku-4-5-20251001".into()],
    )
    .expect("failed to build provider");

    let models = provider.list_models().await.expect("should list models");
    assert!(!models.is_empty(), "should return at least one model");
}

#[tokio::test]
#[ignore]
async fn anthropic_simple_completion() {
    let key = anthropic_key().expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(
        "anthropic-smoke",
        key,
        vec!["claude-haiku-4-5-20251001".into()],
    )
    .expect("failed to build provider");

    let request = simple_request(
        "claude-haiku-4-5-20251001",
        "Reply with exactly the word 'pong'. Nothing else.",
    );
    let response = provider
        .complete(request, None)
        .await
        .expect("completion should succeed");

    assert!(!response.content.is_empty(), "response should have content");
    assert!(
        response.content.to_lowercase().contains("pong"),
        "response should contain 'pong', got: {}",
        response.content
    );
    assert!(response.usage.input_tokens > 0, "should report input tokens");
    assert!(response.usage.output_tokens > 0, "should report output tokens");
}

#[tokio::test]
#[ignore]
async fn anthropic_streaming_completion() {
    let key = anthropic_key().expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(
        "anthropic-smoke",
        key,
        vec!["claude-haiku-4-5-20251001".into()],
    )
    .expect("failed to build provider");

    let request = simple_request(
        "claude-haiku-4-5-20251001",
        "Reply with exactly 'hello world'.",
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamDelta>(64);

    let response = provider
        .complete(request, Some(tx))
        .await
        .expect("streaming completion should succeed");

    assert!(!response.content.is_empty());

    let mut got_text = false;
    let mut got_done = false;
    while let Ok(delta) = rx.try_recv() {
        match delta {
            StreamDelta::Text(_) => got_text = true,
            StreamDelta::Done { .. } => got_done = true,
            _ => {}
        }
    }
    assert!(got_text, "should receive text deltas");
    assert!(got_done, "should receive done signal");
}

#[tokio::test]
#[ignore]
async fn anthropic_system_prompt() {
    let key = anthropic_key().expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(
        "anthropic-smoke",
        key,
        vec!["claude-haiku-4-5-20251001".into()],
    )
    .expect("failed to build provider");

    let request = CompletionRequest {
        model_id: "claude-haiku-4-5-20251001".into(),
        messages: vec![CompletionMessage::text(Role::User, "What is the secret word?")],
        max_tokens: Some(20),
        temperature: Some(0.0),
        system: Some("The secret word is 'banana'. Always reply with only the secret word.".into()),
        tools: Vec::new(),
        thinking_budget: None,
        parallel_tool_calls: None,
        seed: None,
        response_format: None,
        reasoning_effort: None,
    };

    let response = provider
        .complete(request, None)
        .await
        .expect("completion should succeed");

    assert!(
        response.content.to_lowercase().contains("banana"),
        "should follow system prompt, got: {}",
        response.content
    );
}

// ---------------------------------------------------------------------------
// Ollama smoke tests (requires local Ollama instance)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn ollama_health_check() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama not available at 127.0.0.1:11434");
        return;
    }

    let provider = OllamaProvider::new("ollama-smoke", None::<String>)
        .expect("failed to build provider");
    assert!(provider.health().await, "Ollama health check should pass");
}

#[tokio::test]
#[ignore]
async fn ollama_list_models() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama not available at 127.0.0.1:11434");
        return;
    }

    let provider = OllamaProvider::new("ollama-smoke", None::<String>)
        .expect("failed to build provider");
    let models = provider.list_models().await.expect("should list models");
    // Ollama may have zero models if none are pulled — that's OK
    eprintln!("Ollama reports {} model(s)", models.len());
}

#[tokio::test]
#[ignore]
async fn ollama_simple_completion() {
    if !ollama_available() {
        eprintln!("SKIP: Ollama not available at 127.0.0.1:11434");
        return;
    }

    let provider = OllamaProvider::new("ollama-smoke", None::<String>)
        .expect("failed to build provider");
    let models = provider.list_models().await.expect("should list models");
    if models.is_empty() {
        eprintln!("SKIP: No Ollama models pulled");
        return;
    }

    let model_id = &models[0].id;
    let request = simple_request(model_id, "Reply with exactly the word 'pong'. Nothing else.");
    let response = provider
        .complete(request, None)
        .await
        .expect("completion should succeed");

    assert!(!response.content.is_empty(), "response should have content");
    eprintln!("Ollama response ({}): {}", model_id, response.content);
}

// ---------------------------------------------------------------------------
// Failover chain smoke test
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn failover_chain_tries_providers_in_order() {
    let mut providers: Vec<Arc<dyn ModelProvider>> = Vec::new();

    if let Some(key) = openai_key() {
        providers.push(Arc::new(OpenAiProvider::new(
            "openai",
            &openai_base_url(),
            key,
            vec![openai_model()],
        ).expect("failed to build provider")));
    }
    if let Some(key) = anthropic_key() {
        providers.push(Arc::new(AnthropicProvider::new(
            "anthropic",
            key,
            vec!["claude-haiku-4-5-20251001".into()],
        ).expect("failed to build provider")));
    }

    assert!(
        !providers.is_empty(),
        "At least one API key must be set for failover test"
    );

    let chain = FailoverChain::new(providers, 30);

    let health = chain.health().await;
    assert!(
        health.iter().any(|h| h.healthy),
        "at least one provider should be healthy"
    );
}

// ---------------------------------------------------------------------------
// Error handling smoke tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn openai_invalid_key_returns_auth_error() {
    let base = openai_base_url();
    let model = openai_model();
    let provider = OpenAiProvider::new(
        "openai-bad-key",
        &base,
        SecretString::from("sk-invalid-key-for-testing".to_string()),
        vec![model.clone()],
    )
    .expect("failed to build provider");

    let request = simple_request(&model, "test");
    let err = provider
        .complete(request, None)
        .await
        .expect_err("should fail with invalid key");

    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("auth") || msg.contains("401") || msg.contains("invalid") || msg.contains("key")
            || msg.contains("error") || msg.contains("denied"),
        "error should indicate auth failure, got: {}",
        msg
    );
}

#[tokio::test]
#[ignore]
async fn anthropic_invalid_key_returns_auth_error() {
    let provider = AnthropicProvider::new(
        "anthropic-bad-key",
        SecretString::from("sk-ant-invalid-key-for-testing".to_string()),
        vec!["claude-haiku-4-5-20251001".into()],
    )
    .expect("failed to build provider");

    let request = simple_request("claude-haiku-4-5-20251001", "test");
    let err = provider
        .complete(request, None)
        .await
        .expect_err("should fail with invalid key");

    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("auth") || msg.contains("401") || msg.contains("invalid") || msg.contains("key"),
        "error should mention auth failure, got: {}",
        msg
    );
}
