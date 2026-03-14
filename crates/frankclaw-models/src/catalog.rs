//! Static model catalog with known metadata for popular models.
//!
//! Provides accurate context windows, output limits, costs, and capabilities
//! for models from OpenAI, Anthropic, and other providers. Falls back to
//! conservative defaults for unknown models.

use std::sync::LazyLock;

use frankclaw_core::model::{
    InputModality, ModelApi, ModelCompat, ModelCost, ModelDef,
};

/// Look up a model by ID and return its known metadata.
/// Returns `None` for unknown models (caller should use defaults).
pub fn lookup(model_id: &str) -> Option<ModelDef> {
    CATALOG.iter().find(|m| m.id == model_id).cloned()
}

/// Return all known models for a given API type.
pub fn models_for_api(api: ModelApi) -> Vec<ModelDef> {
    CATALOG.iter().filter(|m| m.api == api).cloned().collect()
}

/// Enrich a model definition with catalog metadata if available.
/// Preserves the original ID but fills in context window, costs, etc.
pub fn enrich(model_id: &str, api: ModelApi) -> ModelDef {
    if let Some(known) = lookup(model_id) {
        return known;
    }

    // Default metadata by API type.
    match api {
        ModelApi::AnthropicMessages => ModelDef {
            id: model_id.to_string(),
            name: model_id.to_string(),
            api,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_output_tokens: 8192,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: true,
            },
        },
        ModelApi::OpenaiCompletions | ModelApi::OpenaiResponses => ModelDef {
            id: model_id.to_string(),
            name: model_id.to_string(),
            api,
            reasoning: false,
            input: vec![InputModality::Text],
            cost: ModelCost::default(),
            context_window: 128_000,
            max_output_tokens: 4096,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: false,
                supports_streaming: true,
                supports_json_mode: true,
                supports_system_message: true,
            },
        },
        ModelApi::Ollama => ModelDef {
            id: model_id.to_string(),
            name: model_id.to_string(),
            api,
            reasoning: false,
            input: vec![InputModality::Text],
            cost: ModelCost::default(),
            context_window: 8_192,
            max_output_tokens: 2048,
            compat: ModelCompat {
                supports_tools: false,
                supports_vision: false,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: true,
            },
        },
        _ => ModelDef {
            id: model_id.to_string(),
            name: model_id.to_string(),
            api,
            reasoning: false,
            input: vec![InputModality::Text],
            cost: ModelCost::default(),
            context_window: 32_000,
            max_output_tokens: 4096,
            compat: ModelCompat::default(),
        },
    }
}

// ── Static catalog ──────────────────────────────────────────────────────

static CATALOG: LazyLock<Vec<ModelDef>> = LazyLock::new(|| {
    vec![
        // ── OpenAI ──────────────────────────────────────────────────────
        ModelDef {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: ModelApi::OpenaiCompletions,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 2.50,
                output_per_mtok: 10.00,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            },
            context_window: 128_000,
            max_output_tokens: 0x4000,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: true,
                supports_system_message: true,
            },
        },
        ModelDef {
            id: "gpt-4o-mini".into(),
            name: "GPT-4o Mini".into(),
            api: ModelApi::OpenaiCompletions,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 0.15,
                output_per_mtok: 0.60,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            },
            context_window: 128_000,
            max_output_tokens: 0x4000,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: true,
                supports_system_message: true,
            },
        },
        ModelDef {
            id: "gpt-4-turbo".into(),
            name: "GPT-4 Turbo".into(),
            api: ModelApi::OpenaiCompletions,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 10.00,
                output_per_mtok: 30.00,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            },
            context_window: 128_000,
            max_output_tokens: 4_096,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: true,
                supports_system_message: true,
            },
        },
        ModelDef {
            id: "o1".into(),
            name: "o1".into(),
            api: ModelApi::OpenaiCompletions,
            reasoning: true,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 15.00,
                output_per_mtok: 60.00,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            },
            context_window: 200_000,
            max_output_tokens: 100_000,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: false,
            },
        },
        ModelDef {
            id: "o3-mini".into(),
            name: "o3-mini".into(),
            api: ModelApi::OpenaiCompletions,
            reasoning: true,
            input: vec![InputModality::Text],
            cost: ModelCost {
                input_per_mtok: 1.10,
                output_per_mtok: 4.40,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            },
            context_window: 200_000,
            max_output_tokens: 100_000,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: false,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: false,
            },
        },
        // ── Anthropic ───────────────────────────────────────────────────
        ModelDef {
            id: "claude-opus-4-6".into(),
            name: "Claude Opus 4.6".into(),
            api: ModelApi::AnthropicMessages,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 15.00,
                output_per_mtok: 75.00,
                cache_read_per_mtok: Some(1.50),
                cache_write_per_mtok: Some(18.75),
            },
            context_window: 200_000,
            max_output_tokens: 32_000,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: true,
            },
        },
        ModelDef {
            id: "claude-sonnet-4-6".into(),
            name: "Claude Sonnet 4.6".into(),
            api: ModelApi::AnthropicMessages,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 3.00,
                output_per_mtok: 15.00,
                cache_read_per_mtok: Some(0.30),
                cache_write_per_mtok: Some(3.75),
            },
            context_window: 200_000,
            max_output_tokens: 16_000,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: true,
            },
        },
        ModelDef {
            id: "claude-haiku-4-5-20251001".into(),
            name: "Claude Haiku 4.5".into(),
            api: ModelApi::AnthropicMessages,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 0.80,
                output_per_mtok: 4.00,
                cache_read_per_mtok: Some(0.08),
                cache_write_per_mtok: Some(1.00),
            },
            context_window: 200_000,
            max_output_tokens: 8_192,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: true,
            },
        },
        ModelDef {
            id: "claude-sonnet-4-5-20250514".into(),
            name: "Claude Sonnet 4.5".into(),
            api: ModelApi::AnthropicMessages,
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input_per_mtok: 3.00,
                output_per_mtok: 15.00,
                cache_read_per_mtok: Some(0.30),
                cache_write_per_mtok: Some(3.75),
            },
            context_window: 200_000,
            max_output_tokens: 16_000,
            compat: ModelCompat {
                supports_tools: true,
                supports_vision: true,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: true,
            },
        },
    ]
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_known_model() {
        let model = lookup("gpt-4o").expect("gpt-4o should be in catalog");
        assert_eq!(model.name, "GPT-4o");
        assert_eq!(model.context_window, 128_000);
        assert_eq!(model.max_output_tokens, 16_384);
        assert!(model.compat.supports_vision);
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("nonexistent-model-xyz").is_none());
    }

    #[test]
    fn models_for_api_openai() {
        let models = models_for_api(ModelApi::OpenaiCompletions);
        assert!(models.len() >= 3); // gpt-4o, gpt-4o-mini, gpt-4-turbo, o1, o3-mini
        assert!(models.iter().all(|m| m.api == ModelApi::OpenaiCompletions));
    }

    #[test]
    fn models_for_api_anthropic() {
        let models = models_for_api(ModelApi::AnthropicMessages);
        assert!(models.len() >= 3);
        assert!(models.iter().all(|m| m.api == ModelApi::AnthropicMessages));
        // All Anthropic models support vision
        assert!(models.iter().all(|m| m.compat.supports_vision));
    }

    #[test]
    fn enrich_known_model() {
        let model = enrich("claude-opus-4-6", ModelApi::AnthropicMessages);
        assert_eq!(model.name, "Claude Opus 4.6");
        assert_eq!(model.context_window, 200_000);
        assert_eq!(model.max_output_tokens, 32_000);
        assert_eq!(model.cost.input_per_mtok, 15.00);
    }

    #[test]
    fn enrich_unknown_anthropic_uses_defaults() {
        let model = enrich("claude-future-model", ModelApi::AnthropicMessages);
        assert_eq!(model.id, "claude-future-model");
        assert_eq!(model.context_window, 200_000);
        assert_eq!(model.max_output_tokens, 8192);
        assert!(model.compat.supports_tools);
    }

    #[test]
    fn enrich_unknown_openai_uses_defaults() {
        let model = enrich("gpt-5-future", ModelApi::OpenaiCompletions);
        assert_eq!(model.id, "gpt-5-future");
        assert_eq!(model.context_window, 128_000);
        assert!(model.compat.supports_json_mode);
    }

    #[test]
    fn enrich_unknown_ollama_uses_conservative_defaults() {
        let model = enrich("llama3:8b", ModelApi::Ollama);
        assert_eq!(model.context_window, 8_192);
        assert!(!model.compat.supports_tools);
    }

    #[test]
    fn reasoning_models_flagged() {
        let o1 = lookup("o1").unwrap();
        assert!(o1.reasoning);
        let o3 = lookup("o3-mini").unwrap();
        assert!(o3.reasoning);
        // Non-reasoning models
        let gpt4o = lookup("gpt-4o").unwrap();
        assert!(!gpt4o.reasoning);
    }

    #[test]
    fn anthropic_models_have_cache_pricing() {
        let models = models_for_api(ModelApi::AnthropicMessages);
        for m in &models {
            assert!(
                m.cost.cache_read_per_mtok.is_some(),
                "{} missing cache_read pricing",
                m.id
            );
            assert!(
                m.cost.cache_write_per_mtok.is_some(),
                "{} missing cache_write pricing",
                m.id
            );
        }
    }

    #[test]
    fn openai_models_no_cache_pricing() {
        let models = models_for_api(ModelApi::OpenaiCompletions);
        for m in &models {
            assert!(
                m.cost.cache_read_per_mtok.is_none(),
                "{} should not have cache pricing",
                m.id
            );
        }
    }
}
