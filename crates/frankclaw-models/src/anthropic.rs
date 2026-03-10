use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::*;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    id: String,
    client: Client,
    api_key: SecretString,
    models: Vec<String>,
}

impl AnthropicProvider {
    pub fn new(
        id: impl Into<String>,
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
            api_key,
            models,
        }
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse> {
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|msg| {
                serde_json::json!({
                    "role": msg.role,
                    "content": msg.content,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": request.model_id,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });

        if let Some(system) = &request.system {
            body["system"] = serde_json::json!(system);
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
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
        }

        let url = format!("{ANTHROPIC_API_URL}/messages");
        debug!(model = %request.model_id, "sending anthropic request");

        let response = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
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

        // Parse Anthropic response format.
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(blocks) = data["content"].as_array() {
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"].as_str() {
                            content.push_str(text);
                        }
                    }
                    Some("tool_use") => {
                        if let (Some(id), Some(name)) =
                            (block["id"].as_str(), block["name"].as_str())
                        {
                            tool_calls.push(ToolCallResponse {
                                id: id.to_string(),
                                name: name.to_string(),
                                arguments: block["input"].to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        let finish_reason = match data["stop_reason"].as_str() {
            Some("end_turn") | Some("stop_sequence") => FinishReason::Stop,
            Some("max_tokens") => FinishReason::MaxTokens,
            Some("tool_use") => FinishReason::ToolUse,
            _ => FinishReason::Stop,
        };

        let usage = Usage {
            input_tokens: data["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: data["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
            cache_read_tokens: data["usage"]["cache_read_input_tokens"]
                .as_u64()
                .map(|v| v as u32),
            cache_write_tokens: data["usage"]["cache_creation_input_tokens"]
                .as_u64()
                .map(|v| v as u32),
        };

        Ok(CompletionResponse {
            content,
            tool_calls,
            usage,
            finish_reason,
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelDef>> {
        Ok(self
            .models
            .iter()
            .map(|id| ModelDef {
                id: id.clone(),
                name: id.clone(),
                api: ModelApi::AnthropicMessages,
                reasoning: id.contains("opus") || id.contains("sonnet"),
                input: vec![InputModality::Text, InputModality::Image],
                cost: ModelCost::default(),
                context_window: 200_000,
                max_output_tokens: 8192,
                compat: ModelCompat {
                    supports_tools: true,
                    supports_vision: true,
                    supports_streaming: true,
                    supports_system_message: true,
                    ..Default::default()
                },
            })
            .collect())
    }

    async fn health(&self) -> bool {
        // Anthropic doesn't have a lightweight health endpoint.
        // Just check if we can reach the API.
        self.client
            .get(format!("{ANTHROPIC_API_URL}/messages"))
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .send()
            .await
            .is_ok()
    }
}
