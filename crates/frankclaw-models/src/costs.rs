//! Per-model cost lookup table for multi-provider LLM support.
//!
//! Returns (input_cost_per_token, output_cost_per_token) as f64 pairs.
//! Ollama and other local models return zero cost.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

/// Look up known per-token costs for a model by its identifier.
///
/// Returns `Some((input_cost, output_cost))` for known models, `None` otherwise.
pub fn model_cost(model_id: &str) -> Option<(f64, f64)> {
    // OpenRouter free-tier models
    if model_id.ends_with(":free") || model_id == "openrouter/free" || model_id == "free" {
        return Some((0.0, 0.0));
    }

    // Normalize: strip provider prefixes (e.g., "openai/gpt-4o" -> "gpt-4o")
    let id = model_id
        .rsplit_once('/')
        .map_or(model_id, |(_, name)| name);

    match id {
        // OpenAI — GPT-4.x
        "gpt-4.1" => Some((0.000002, 0.000008)),
        "gpt-4.1-mini" => Some((0.0000004, 0.0000016)),
        "gpt-4.1-nano" => Some((0.0000001, 0.0000004)),
        "gpt-4o" | "gpt-4o-2024-11-20" | "gpt-4o-2024-08-06" => Some((0.0000025, 0.00001)),
        "gpt-4o-mini" | "gpt-4o-mini-2024-07-18" => Some((0.00000015, 0.0000006)),
        "gpt-4-turbo" | "gpt-4-turbo-2024-04-09" => Some((0.00001, 0.00003)),
        "gpt-4" | "gpt-4-0613" => Some((0.00003, 0.00006)),
        "gpt-3.5-turbo" | "gpt-3.5-turbo-0125" => Some((0.0000005, 0.0000015)),
        // OpenAI — reasoning
        "o3" => Some((0.000002, 0.000008)),
        "o3-mini" | "o3-mini-2025-01-31" => Some((0.0000011, 0.0000044)),
        "o4-mini" => Some((0.0000011, 0.0000044)),
        "o1" | "o1-2024-12-17" => Some((0.000015, 0.00006)),
        "o1-mini" | "o1-mini-2024-09-12" => Some((0.000003, 0.000012)),

        // Anthropic
        "claude-opus-4-6"
        | "claude-opus-4-5"
        | "claude-opus-4-5-20251101"
        | "claude-opus-4-1"
        | "claude-opus-4-1-20250805"
        | "claude-opus-4-0"
        | "claude-opus-4-20250514"
        | "claude-3-opus-20240229"
        | "claude-3-opus-latest" => Some((0.000015, 0.000075)),
        "claude-sonnet-4-6"
        | "claude-sonnet-4-5"
        | "claude-sonnet-4-5-20250929"
        | "claude-sonnet-4-0"
        | "claude-sonnet-4-20250514"
        | "claude-3-7-sonnet-20250219"
        | "claude-3-7-sonnet-latest"
        | "claude-3-5-sonnet-20241022"
        | "claude-3-5-sonnet-latest" => Some((0.000003, 0.000015)),
        "claude-haiku-4-5"
        | "claude-haiku-4-5-20251001"
        | "claude-3-5-haiku-20241022"
        | "claude-3-5-haiku-latest" => Some((0.0000008, 0.000004)),
        "claude-3-haiku-20240307" => Some((0.00000025, 0.00000125)),

        // Ollama / local models -- free
        _ if is_local_model(id) => Some((0.0, 0.0)),

        _ => None,
    }
}

/// Default cost for unknown models (roughly GPT-4o pricing).
pub fn default_cost() -> (f64, f64) {
    (0.0000025, 0.00001)
}

/// Heuristic to detect local/self-hosted models (Ollama, llama.cpp, etc.).
fn is_local_model(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.starts_with("llama")
        || lower.starts_with("mistral")
        || lower.starts_with("mixtral")
        || lower.starts_with("phi")
        || lower.starts_with("gemma")
        || lower.starts_with("qwen")
        || lower.starts_with("codellama")
        || lower.starts_with("deepseek")
        || lower.starts_with("starcoder")
        || lower.starts_with("vicuna")
        || lower.starts_with("yi")
        || lower.contains(":latest")
        || lower.contains(":instruct")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("gpt-4o")]
    #[case("claude-3-5-sonnet-20241022")]
    fn known_model_has_cost(#[case] model: &str) {
        let (input, output) = model_cost(model).expect("expected cost for known model");
        assert!(input > 0.0, "expected positive input cost for {model}");
        assert!(output > input, "expected output cost > input cost for {model}");
    }

    #[test]
    fn local_model_is_free() {
        let (input, output) = model_cost("llama3").unwrap();
        assert_eq!(input, 0.0);
        assert_eq!(output, 0.0);
    }

    #[test]
    fn ollama_tagged_model_is_free() {
        let (input, output) = model_cost("mistral:latest").unwrap();
        assert_eq!(input, 0.0);
        assert_eq!(output, 0.0);
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(model_cost("some-totally-unknown-model-xyz").is_none());
    }

    #[test]
    fn default_cost_is_nonzero() {
        let (input, output) = default_cost();
        assert!(input > 0.0);
        assert!(output > 0.0);
    }

    #[test]
    fn provider_prefix_is_stripped() {
        assert_eq!(model_cost("openai/gpt-4o"), model_cost("gpt-4o"));
    }

    #[test]
    fn openrouter_free_suffix_returns_zero() {
        let (input, output) = model_cost("stepfun/step-3.5-flash:free").unwrap();
        assert_eq!(input, 0.0);
        assert_eq!(output, 0.0);
    }

    #[test]
    fn openrouter_free_router_returns_zero() {
        let (input, output) = model_cost("openrouter/free").unwrap();
        assert_eq!(input, 0.0);
        assert_eq!(output, 0.0);
    }
}
