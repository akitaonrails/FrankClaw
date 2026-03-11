use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use std::collections::BTreeMap;
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::*;

use crate::sse::SseDecoder;

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
        stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse> {
        let mut body = build_request_body(&request);
        if stream_tx.is_some() {
            body["stream"] = serde_json::json!(true);
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
            return Err(classify_provider_error(status, &body));
        }

        if let Some(stream_tx) = stream_tx {
            let mut decoder = SseDecoder::default();
            let mut state = AnthropicStreamState::default();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| FrankClawError::ModelProvider {
                    msg: format!("failed to read streaming response: {e}"),
                })?;
                for event in decoder.push(chunk.as_ref()) {
                    for delta in apply_stream_event(&mut state, event.event.as_deref(), &event.data)? {
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
            if !state.done {
                if let Some(event) = decoder.finish() {
                    for delta in apply_stream_event(&mut state, event.event.as_deref(), &event.data)? {
                        let _ = stream_tx.send(delta).await;
                    }
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
        parse_completion_response(&data)
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

fn build_request_body(request: &CompletionRequest) -> serde_json::Value {
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

    body
}

fn parse_completion_response(data: &serde_json::Value) -> Result<CompletionResponse> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    if let Some(blocks) = data["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("thinking") => {
                    // Anthropic thinking blocks contain internal reasoning.
                    // Preserve them in content for transcript storage.
                    if let Some(thinking) = block["thinking"].as_str() {
                        if !thinking.is_empty() {
                            if !content.is_empty() {
                                content.push_str("\n\n");
                            }
                            content.push_str("<thinking>\n");
                            content.push_str(thinking);
                            content.push_str("\n</thinking>\n\n");
                        }
                    }
                }
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

    Ok(CompletionResponse {
        content,
        tool_calls,
        usage: parse_usage(data),
        finish_reason: parse_finish_reason(data["stop_reason"].as_str()),
    })
}

#[derive(Debug)]
struct AnthropicStreamState {
    content: String,
    tool_calls: BTreeMap<usize, StreamingToolCall>,
    usage: Usage,
    finish_reason: FinishReason,
    done: bool,
}

impl Default for AnthropicStreamState {
    fn default() -> Self {
        Self {
            content: String::new(),
            tool_calls: BTreeMap::new(),
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
            done: false,
        }
    }
}

#[derive(Debug, Default)]
struct StreamingToolCall {
    id: String,
    name: String,
    arguments: String,
    ended: bool,
}

impl AnthropicStreamState {
    fn finish(self) -> Result<CompletionResponse> {
        let mut tool_calls = Vec::with_capacity(self.tool_calls.len());
        for (_, tool_call) in self.tool_calls {
            if tool_call.id.trim().is_empty() || tool_call.name.trim().is_empty() {
                return Err(FrankClawError::ModelProvider {
                    msg: "streamed anthropic tool call missing id or name".into(),
                });
            }
            tool_calls.push(ToolCallResponse {
                id: tool_call.id,
                name: tool_call.name,
                arguments: tool_call.arguments,
            });
        }
        Ok(CompletionResponse {
            content: self.content,
            tool_calls,
            usage: self.usage,
            finish_reason: self.finish_reason,
        })
    }
}

fn apply_stream_event(
    state: &mut AnthropicStreamState,
    event_type: Option<&str>,
    data: &str,
) -> Result<Vec<StreamDelta>> {
    let payload: serde_json::Value = serde_json::from_str(data).map_err(|err| FrankClawError::ModelProvider {
        msg: format!("invalid streaming response chunk: {err}"),
    })?;
    let mut deltas = Vec::new();

    match event_type.unwrap_or_default() {
        "message_start" => {
            state.usage.input_tokens = payload["message"]["usage"]["input_tokens"]
                .as_u64()
                .unwrap_or(0) as u32;
            state.usage.cache_read_tokens = payload["message"]["usage"]["cache_read_input_tokens"]
                .as_u64()
                .map(|v| v as u32);
            state.usage.cache_write_tokens = payload["message"]["usage"]["cache_creation_input_tokens"]
                .as_u64()
                .map(|v| v as u32);
        }
        "content_block_start" => {
            if payload["content_block"]["type"].as_str() == Some("tool_use") {
                if let (Some(index), Some(id), Some(name)) = (
                    payload["index"].as_u64(),
                    payload["content_block"]["id"].as_str(),
                    payload["content_block"]["name"].as_str(),
                ) {
                    state
                        .tool_calls
                        .entry(index as usize)
                        .or_insert_with(|| StreamingToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        ..Default::default()
                    });
                    deltas.push(StreamDelta::ToolCallStart {
                        id: id.to_string(),
                        name: name.to_string(),
                    });
                }
            }
        }
        "content_block_delta" => match payload["delta"]["type"].as_str() {
            Some("text_delta") => {
                if let Some(text) = payload["delta"]["text"].as_str() {
                    state.content.push_str(text);
                    deltas.push(StreamDelta::Text(text.to_string()));
                }
            }
            Some("thinking_delta") => {
                if let Some(thinking) = payload["delta"]["thinking"].as_str() {
                    state.content.push_str(thinking);
                    deltas.push(StreamDelta::Text(thinking.to_string()));
                }
            }
            Some("input_json_delta") => {
                let partial = payload["delta"]["partial_json"].as_str().unwrap_or("");
                if let Some(index) = payload["index"].as_u64().map(|value| value as usize) {
                    if let Some(tool_call) = state.tool_calls.get_mut(&index) {
                        tool_call.arguments.push_str(partial);
                        deltas.push(StreamDelta::ToolCallDelta {
                            id: tool_call.id.clone(),
                            arguments: partial.to_string(),
                        });
                    }
                }
            }
            _ => {}
        },
        "content_block_stop" => {
            if let Some(index) = payload["index"].as_u64().map(|value| value as usize) {
                if let Some(tool_call) = state.tool_calls.get_mut(&index) {
                    if !tool_call.ended {
                        tool_call.ended = true;
                        deltas.push(StreamDelta::ToolCallEnd {
                            id: tool_call.id.clone(),
                        });
                    }
                }
            }
        }
        "message_delta" => {
            state.finish_reason = parse_finish_reason(payload["delta"]["stop_reason"].as_str());
            if let Some(output_tokens) = payload["usage"]["output_tokens"].as_u64() {
                state.usage.output_tokens = output_tokens as u32;
            }
        }
        "message_stop" => {
            state.done = true;
        }
        "error" => {
            let message = payload["error"]["message"].as_str().unwrap_or("anthropic stream error");
            deltas.push(StreamDelta::Error(message.to_string()));
            return Err(FrankClawError::ModelProvider {
                msg: message.to_string(),
            });
        }
        _ => {}
    }

    Ok(deltas)
}

fn parse_usage(data: &serde_json::Value) -> Usage {
    Usage {
        input_tokens: data["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: data["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
        cache_read_tokens: data["usage"]["cache_read_input_tokens"]
            .as_u64()
            .map(|v| v as u32),
        cache_write_tokens: data["usage"]["cache_creation_input_tokens"]
            .as_u64()
            .map(|v| v as u32),
    }
}

fn parse_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("end_turn") | Some("stop_sequence") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::MaxTokens,
        Some("tool_use") => FinishReason::ToolUse,
        _ => FinishReason::Stop,
    }
}

/// Classify HTTP errors from model providers into actionable error messages.
/// Detects context overflow, billing issues, rate limits, and auth failures.
pub(crate) fn classify_provider_error(
    status: reqwest::StatusCode,
    body: &str,
) -> FrankClawError {
    let body_lower = body.to_lowercase();

    // Context overflow detection — varies across providers
    if is_context_overflow(&body_lower) {
        return FrankClawError::ModelProvider {
            msg: format!("context length exceeded (HTTP {status}): {body}"),
        };
    }

    match status.as_u16() {
        401 => FrankClawError::ModelProvider {
            msg: format!("authentication failed (invalid API key): {body}"),
        },
        402 => {
            // 402 can mean billing issue OR rate limit spend cap
            if body_lower.contains("rate_limit") || body_lower.contains("spend") {
                FrankClawError::ModelProvider {
                    msg: format!("rate limit spend cap reached (retryable): {body}"),
                }
            } else {
                FrankClawError::ModelProvider {
                    msg: format!("billing error (out of credits, non-retryable): {body}"),
                }
            }
        }
        429 => FrankClawError::ModelProvider {
            msg: format!("rate limited (HTTP 429): {body}"),
        },
        _ => FrankClawError::ModelProvider {
            msg: format!("HTTP {status}: {body}"),
        },
    }
}

/// Check if an error body indicates context length overflow.
fn is_context_overflow(body_lower: &str) -> bool {
    body_lower.contains("context_length_exceeded")
        || body_lower.contains("prompt is too long")
        || body_lower.contains("maximum context length")
        || body_lower.contains("context window")
        || body_lower.contains("token limit")
        || body_lower.contains("too many tokens")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_stream_event_accumulates_text_blocks() {
        let mut state = AnthropicStreamState::default();
        apply_stream_event(
            &mut state,
            Some("message_start"),
            r#"{"message":{"usage":{"input_tokens":12}}}"#,
        )
        .expect("message start should parse");
        let deltas = apply_stream_event(
            &mut state,
            Some("content_block_delta"),
            r#"{"delta":{"type":"text_delta","text":"hello "}}"#,
        )
        .expect("text chunk should parse");
        assert_eq!(deltas, vec![StreamDelta::Text("hello ".into())]);
        let deltas = apply_stream_event(
            &mut state,
            Some("message_delta"),
            r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        )
        .expect("message delta should parse");
        assert!(deltas.is_empty());

        let response = state.finish().expect("response should build");
        assert_eq!(response.content, "hello ");
        assert_eq!(response.usage.input_tokens, 12);
        assert_eq!(response.usage.output_tokens, 4);
    }

    #[test]
    fn parse_completion_response_preserves_thinking_blocks() {
        let data = serde_json::json!({
            "content": [
                {
                    "type": "thinking",
                    "thinking": "Let me reason about this..."
                },
                {
                    "type": "text",
                    "text": "The answer is 42."
                }
            ],
            "usage": { "input_tokens": 10, "output_tokens": 5 },
            "stop_reason": "end_turn"
        });
        let response = parse_completion_response(&data).expect("should parse");
        assert!(response.content.contains("<thinking>"));
        assert!(response.content.contains("Let me reason about this..."));
        assert!(response.content.contains("The answer is 42."));
    }

    #[test]
    fn apply_stream_event_handles_thinking_delta() {
        let mut state = AnthropicStreamState::default();
        let deltas = apply_stream_event(
            &mut state,
            Some("content_block_delta"),
            r#"{"delta":{"type":"thinking_delta","thinking":"step 1: "}}"#,
        )
        .expect("thinking delta should parse");
        assert_eq!(deltas, vec![StreamDelta::Text("step 1: ".into())]);
        assert_eq!(state.content, "step 1: ");
    }

    #[test]
    fn classify_provider_error_detects_context_overflow() {
        let err = classify_provider_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#,
        );
        let msg = format!("{err}");
        assert!(msg.contains("context length exceeded"), "got: {msg}");
    }

    #[test]
    fn classify_provider_error_distinguishes_402_billing_vs_rate_limit() {
        let billing = classify_provider_error(
            reqwest::StatusCode::PAYMENT_REQUIRED,
            r#"{"error":{"message":"Your account has insufficient credits"}}"#,
        );
        assert!(format!("{billing}").contains("billing error"));

        let rate = classify_provider_error(
            reqwest::StatusCode::PAYMENT_REQUIRED,
            r#"{"error":{"message":"rate_limit: monthly spend cap exceeded"}}"#,
        );
        assert!(format!("{rate}").contains("retryable"));
    }

    #[test]
    fn classify_provider_error_detects_auth_failure() {
        let err = classify_provider_error(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":{"message":"invalid x-api-key"}}"#,
        );
        assert!(format!("{err}").contains("authentication failed"));
    }

    #[test]
    fn apply_stream_event_accumulates_tool_use_json() {
        let mut state = AnthropicStreamState::default();
        let deltas = apply_stream_event(
            &mut state,
            Some("content_block_start"),
            r#"{"index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup"}}"#,
        )
        .expect("tool start should parse");
        assert_eq!(
            deltas,
            vec![StreamDelta::ToolCallStart {
                id: "toolu_1".into(),
                name: "lookup".into(),
            }]
        );
        let deltas = apply_stream_event(
            &mut state,
            Some("content_block_delta"),
            r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"cl"}}"#,
        )
        .expect("tool delta should parse");
        assert_eq!(
            deltas,
            vec![StreamDelta::ToolCallDelta {
                id: "toolu_1".into(),
                arguments: "{\"q\":\"cl".into(),
            }]
        );
        let deltas = apply_stream_event(
            &mut state,
            Some("content_block_stop"),
            r#"{"index":0}"#,
        )
        .expect("tool stop should parse");
        assert_eq!(
            deltas,
            vec![StreamDelta::ToolCallEnd {
                id: "toolu_1".into(),
            }]
        );
        state
            .tool_calls
            .get_mut(&0)
            .unwrap()
            .arguments
            .push_str("aw\"}");

        let response = state.finish().expect("response should build");
        assert_eq!(response.tool_calls[0].arguments, "{\"q\":\"claw\"}");
    }
}
