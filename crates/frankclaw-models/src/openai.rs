use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::*;

/// OpenAI-compatible completions provider.
///
/// Works with OpenAI, Azure OpenAI, OpenRouter, and any OpenAI-compatible API.
pub struct OpenAiProvider {
    id: String,
    client: Client,
    base_url: String,
    api_key: SecretString,
    models: Vec<String>,
}

impl OpenAiProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: SecretString,
        models: Vec<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");

        Self {
            id: id.into(),
            client,
            base_url: base_url.into(),
            api_key,
            models,
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse> {
        let messages: Vec<serde_json::Value> = {
            let mut msgs = Vec::new();
            if let Some(system) = &request.system {
                msgs.push(serde_json::json!({
                    "role": "system",
                    "content": system,
                }));
            }
            for msg in &request.messages {
                msgs.push(serde_json::json!({
                    "role": msg.role,
                    "content": msg.content,
                }));
            }
            msgs
        };

        let mut body = serde_json::json!({
            "model": request.model_id,
            "messages": messages,
        });

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }
        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        debug!(url, model = %request.model_id, "sending completion request");

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key.expose_secret()))
            .json(&body)
            .send()
            .await
            .map_err(|e| FrankClawError::ModelProvider {
                msg: format!("request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(FrankClawError::ModelProvider {
                msg: format!("HTTP {status}: {body}"),
            });
        }

        let data: serde_json::Value = response.json().await.map_err(|e| {
            FrankClawError::ModelProvider {
                msg: format!("invalid response: {e}"),
            }
        })?;

        // Parse response.
        let choice = data["choices"]
            .get(0)
            .ok_or_else(|| FrankClawError::ModelProvider {
                msg: "no choices in response".into(),
            })?;

        let content = choice["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let finish_reason = match choice["finish_reason"].as_str() {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::MaxTokens,
            Some("tool_calls") => FinishReason::ToolUse,
            Some("content_filter") => FinishReason::ContentFilter,
            _ => FinishReason::Stop,
        };

        let usage = Usage {
            input_tokens: data["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: data["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
            ..Default::default()
        };

        // Parse tool calls if present.
        let tool_calls = choice["message"]["tool_calls"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        Some(ToolCallResponse {
                            id: tc["id"].as_str()?.to_string(),
                            name: tc["function"]["name"].as_str()?.to_string(),
                            arguments: tc["function"]["arguments"]
                                .as_str()
                                .unwrap_or("{}")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(CompletionResponse {
            content,
            tool_calls,
            usage,
            finish_reason,
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelDef>> {
        // Return configured models (not fetching from API for now).
        Ok(self
            .models
            .iter()
            .map(|id| ModelDef {
                id: id.clone(),
                name: id.clone(),
                api: ModelApi::OpenaiCompletions,
                reasoning: false,
                input: vec![InputModality::Text],
                cost: ModelCost::default(),
                context_window: 128_000,
                max_output_tokens: 4096,
                compat: ModelCompat {
                    supports_tools: true,
                    supports_streaming: true,
                    supports_system_message: true,
                    ..Default::default()
                },
            })
            .collect())
    }

    async fn health(&self) -> bool {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key.expose_secret()))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
