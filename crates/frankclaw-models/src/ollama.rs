use async_trait::async_trait;
use reqwest::Client;
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::*;

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Ollama local model provider.
///
/// Ollama runs on localhost and requires no API key.
/// This is the most private option — all inference stays on-device.
pub struct OllamaProvider {
    id: String,
    client: Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(id: impl Into<String>, base_url: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .expect("failed to build HTTP client");

        Self {
            id: id.into(),
            client,
            base_url: base_url.unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string()),
        }
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse> {
        // Use Ollama's OpenAI-compatible endpoint.
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

        let body = serde_json::json!({
            "model": request.model_id,
            "messages": messages,
            "stream": false,
        });

        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));
        debug!(url, model = %request.model_id, "sending ollama request");

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| FrankClawError::ModelProvider {
                msg: format!("ollama request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(FrankClawError::ModelProvider {
                msg: format!("ollama HTTP {status}: {body_text}"),
            });
        }

        let data: serde_json::Value = response.json().await.map_err(|e| {
            FrankClawError::ModelProvider {
                msg: format!("invalid ollama response: {e}"),
            }
        })?;

        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        Ok(CompletionResponse {
            content,
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelDef>> {
        let url = format!("{}/api/tags", self.base_url.trim_end_matches('/'));
        let response = self.client.get(&url).send().await.map_err(|e| {
            FrankClawError::ModelProvider {
                msg: format!("failed to list ollama models: {e}"),
            }
        })?;

        let data: serde_json::Value = response.json().await.map_err(|e| {
            FrankClawError::ModelProvider {
                msg: format!("invalid ollama response: {e}"),
            }
        })?;

        let models = data["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let name = m["name"].as_str()?.to_string();
                        Some(ModelDef {
                            id: name.clone(),
                            name,
                            api: ModelApi::Ollama,
                            reasoning: false,
                            input: vec![InputModality::Text],
                            cost: ModelCost::default(),
                            context_window: 8192,
                            max_output_tokens: 4096,
                            compat: ModelCompat::default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    async fn health(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url.trim_end_matches('/'));
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
