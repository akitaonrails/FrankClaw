use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::*;

use crate::openai_compat::{self, StreamState};
use crate::sse::SseDecoder;

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Ollama local model provider.
///
/// Ollama runs on localhost and requires no API key.
/// Uses the OpenAI-compatible `/v1/chat/completions` endpoint,
/// which supports SSE streaming in the same format as OpenAI.
pub struct OllamaProvider {
    id: String,
    client: Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(id: impl Into<String>, base_url: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| FrankClawError::Internal {
                msg: format!("failed to build HTTP client: {e}"),
            })?;

        let base_url = normalize_ollama_url(
            &base_url.unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string()),
        );

        Ok(Self {
            id: id.into(),
            client,
            base_url,
        })
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
        stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse> {
        let mut body = openai_compat::build_request_body(&request);

        if stream_tx.is_some() {
            body["stream"] = serde_json::json!(true);
        } else {
            body["stream"] = serde_json::json!(false);
        }

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
            return Err(crate::anthropic::classify_provider_error(status, &body_text));
        }

        if let Some(stream_tx) = stream_tx {
            let mut decoder = SseDecoder::default();
            let mut state = StreamState::default();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| FrankClawError::ModelProvider {
                    msg: format!("failed to read ollama streaming response: {e}"),
                })?;
                for event in decoder.push(chunk.as_ref()) {
                    for delta in openai_compat::apply_stream_event(&mut state, &event.data)? {
                        let _ = stream_tx.send(delta).await;
                    }
                    if state.done {
                        break;
                    }
                }
                if state.done {
                    break;
                }
            }
            if !state.done && let Some(event) = decoder.finish() {
                for delta in openai_compat::apply_stream_event(&mut state, &event.data)? {
                    let _ = stream_tx.send(delta).await;
                }
            }
            let response = state.finish()?;
            let _ = stream_tx
                .send(StreamDelta::Done {
                    usage: Some(response.usage.clone()),
                })
                .await;
            return Ok(response);
        }

        let data: serde_json::Value =
            response.json().await.map_err(|e| FrankClawError::ModelProvider {
                msg: format!("invalid ollama response: {e}"),
            })?;
        openai_compat::parse_completion_response(&data)
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
                            compat: ModelCompat {
                                supports_streaming: true,
                                supports_system_message: true,
                                ..Default::default()
                            },
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
            .is_ok_and(|r| r.status().is_success())
    }
}

/// Strip `/v1` suffix from Ollama base URL.
/// Users often configure `http://localhost:11434/v1` which works for the
/// OpenAI-compatible endpoint but breaks native API calls (`/api/tags`, `/api/show`).
fn normalize_ollama_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let stripped = trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed);
    stripped.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ollama_url_strips_v1_suffix() {
        assert_eq!(
            normalize_ollama_url("http://localhost:11434/v1"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_ollama_url("http://localhost:11434/v1/"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_ollama_url("http://localhost:11434"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_ollama_url("http://localhost:11434/"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn ollama_provider_uses_normalized_url() {
        let provider = OllamaProvider::new("test", Some("http://localhost:11434/v1/".into()))
            .expect("failed to build provider");
        assert_eq!(provider.base_url, "http://localhost:11434");
    }
}
