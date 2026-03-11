use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::*;

use crate::openai_compat::{self, StreamState};
use crate::sse::SseDecoder;

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
        stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse> {
        let mut body = openai_compat::build_request_body(&request);
        if stream_tx.is_some() {
            body["stream"] = serde_json::json!(true);
            body["stream_options"] = serde_json::json!({
                "include_usage": true,
            });
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
            return Err(crate::anthropic::classify_provider_error(status, &body));
        }

        if let Some(stream_tx) = stream_tx {
            let mut decoder = SseDecoder::default();
            let mut state = StreamState::default();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| FrankClawError::ModelProvider {
                    msg: format!("failed to read streaming response: {e}"),
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
            let _ = stream_tx.send(StreamDelta::Done {
                usage: Some(response.usage.clone()),
            }).await;
            return Ok(response);
        }

        let data: serde_json::Value = response.json().await.map_err(|e| FrankClawError::ModelProvider {
            msg: format!("invalid response: {e}"),
        })?;
        openai_compat::parse_completion_response(&data)
    }

    async fn list_models(&self) -> Result<Vec<ModelDef>> {
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

#[cfg(test)]
mod tests {
    use crate::openai_compat::{self, StreamState};
    use frankclaw_core::model::*;

    #[test]
    fn apply_stream_event_accumulates_text_and_usage() {
        let mut state = StreamState::default();

        let deltas = openai_compat::apply_stream_event(
            &mut state,
            r#"{"choices":[{"delta":{"content":"hel"},"finish_reason":null}]}"#,
        )
        .expect("chunk should parse");
        assert_eq!(deltas, vec![StreamDelta::Text("hel".into())]);

        let deltas = openai_compat::apply_stream_event(
            &mut state,
            r#"{"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#,
        )
        .expect("chunk should parse");
        assert_eq!(deltas, vec![StreamDelta::Text("lo".into())]);
        state.usage = openai_compat::parse_usage(&serde_json::json!({
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }));

        let response = state.finish().expect("response should build");
        assert_eq!(response.content, "hello");
        assert_eq!(response.finish_reason, FinishReason::Stop);
    }

    #[test]
    fn apply_stream_event_accumulates_tool_calls() {
        let mut state = StreamState::default();

        let deltas = openai_compat::apply_stream_event(
            &mut state,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"lookup","arguments":"{\"q\":\"op"}}]}}]}"#,
        )
        .expect("chunk should parse");
        assert_eq!(
            deltas,
            vec![
                StreamDelta::ToolCallStart {
                    id: "call_1".into(),
                    name: "lookup".into(),
                },
                StreamDelta::ToolCallDelta {
                    id: "call_1".into(),
                    arguments: "{\"q\":\"op".into(),
                }
            ]
        );

        let deltas = openai_compat::apply_stream_event(
            &mut state,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"enai\"}"}}],"content":""},"finish_reason":"tool_calls"}]}"#,
        )
        .expect("chunk should parse");
        assert_eq!(
            deltas,
            vec![
                StreamDelta::ToolCallDelta {
                    id: "call_1".into(),
                    arguments: "enai\"}".into(),
                },
                StreamDelta::ToolCallEnd {
                    id: "call_1".into(),
                }
            ]
        );

        let response = state.finish().expect("response should build");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].arguments, "{\"q\":\"openai\"}");
        assert_eq!(response.finish_reason, FinishReason::ToolUse);
    }
}
