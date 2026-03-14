use std::collections::BTreeMap;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{CompletionRequest, ResponseFormat, CompletionResponse, ToolCallResponse, Usage, FinishReason, StreamDelta};
use frankclaw_core::types::Role;

/// Sanitize a tool name for OpenAI compatibility (dots -> underscores).
/// OpenAI requires tool names to match `^[a-zA-Z0-9_-]+$`.
fn sanitize_tool_name(name: &str) -> String {
    name.replace('.', "_")
}

/// Restore a sanitized tool name back to FrankClaw's internal format (underscores → dots)
/// for names that were originally dot-separated (e.g., `browser_open` → `browser.open`).
fn restore_tool_name(name: &str) -> String {
    // Known prefixes that use dot notation internally.
    const PREFIXES: &[&str] = &["browser_", "session_"];
    for prefix in PREFIXES {
        if name.starts_with(prefix) {
            return name.replacen('_', ".", 1);
        }
    }
    name.to_string()
}

/// Build an OpenAI-compatible chat completions request body.
#[expect(clippy::too_many_lines, reason = "request body assembly with many optional fields")]
pub fn build_request_body(request: &CompletionRequest) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = {
        let mut msgs = Vec::new();
        if let Some(system) = &request.system {
            msgs.push(serde_json::json!({
                "role": "system",
                "content": system,
            }));
        }
        for msg in &request.messages {
            if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
                // Assistant message with tool calls — use OpenAI's structured format.
                let tool_calls: Vec<serde_json::Value> = msg
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": sanitize_tool_name(&tc.name),
                                "arguments": tc.arguments,
                            }
                        })
                    })
                    .collect();
                let mut m = serde_json::json!({
                    "role": "assistant",
                    "tool_calls": tool_calls,
                });
                if !msg.content.is_empty() {
                    m["content"] = serde_json::json!(msg.content);
                }
                msgs.push(m);
            } else if msg.role == Role::Tool {
                // Tool result — must include tool_call_id for OpenAI.
                msgs.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": msg.tool_call_id.as_deref().unwrap_or("unknown"),
                    "content": msg.content,
                }));
            } else if !msg.image_content.is_empty() && msg.role == Role::User {
                // User message with images — build multimodal content array.
                let mut content_parts = vec![serde_json::json!({
                    "type": "text",
                    "text": msg.content,
                })];
                for img in &msg.image_content {
                    content_parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", img.mime_type, img.data),
                        }
                    }));
                }
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": content_parts,
                }));
            } else {
                msgs.push(serde_json::json!({
                    "role": msg.role,
                    "content": msg.content,
                }));
            }
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
                        "name": sanitize_tool_name(&t.name),
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
    }
    if let Some(parallel) = request.parallel_tool_calls {
        body["parallel_tool_calls"] = serde_json::json!(parallel);
    }
    if let Some(seed) = request.seed {
        body["seed"] = serde_json::json!(seed);
    }
    if let Some(ref fmt) = request.response_format {
        body["response_format"] = match fmt {
            ResponseFormat::Text => serde_json::json!({"type": "text"}),
            ResponseFormat::JsonObject => serde_json::json!({"type": "json_object"}),
        };
    }
    if let Some(ref effort) = request.reasoning_effort {
        body["reasoning_effort"] = serde_json::json!(effort);
    }

    body
}

/// Parse a non-streaming OpenAI-compatible chat completions response.
pub fn parse_completion_response(data: &serde_json::Value) -> Result<CompletionResponse> {
    let choice = data["choices"]
        .get(0)
        .ok_or_else(|| FrankClawError::ModelProvider {
            msg: "no choices in response".into(),
        })?;

    let content = choice["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let usage = parse_usage(data);
    let tool_calls = choice["message"]["tool_calls"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| {
                    Some(ToolCallResponse {
                        id: tc["id"].as_str()?.to_string(),
                        name: restore_tool_name(tc["function"]["name"].as_str()?),
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
        finish_reason: parse_finish_reason(choice["finish_reason"].as_str()),
    })
}

/// Accumulator for OpenAI-compatible streaming responses.
#[derive(Debug)]
pub struct StreamState {
    pub(crate) content: String,
    pub(crate) tool_calls: BTreeMap<usize, StreamingToolCall>,
    pub(crate) usage: Usage,
    pub(crate) finish_reason: FinishReason,
    pub(crate) done: bool,
}

impl Default for StreamState {
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
pub struct StreamingToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
    pub(crate) started: bool,
    pub(crate) ended: bool,
}

impl StreamState {
    /// Finalize the accumulated stream into a `CompletionResponse`.
    pub(crate) fn finish(mut self) -> Result<CompletionResponse> {
        let mut tool_calls = Vec::with_capacity(self.tool_calls.len());
        for tool_call in self.tool_calls.values_mut() {
            if !tool_call.ended && tool_call.started {
                tool_call.ended = true;
            }
            if tool_call.id.trim().is_empty() || tool_call.name.trim().is_empty() {
                return Err(FrankClawError::ModelProvider {
                    msg: "streamed tool call missing id or name".into(),
                });
            }
            tool_calls.push(ToolCallResponse {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
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

/// Process a single SSE data payload and return any stream deltas.
pub fn apply_stream_event(
    state: &mut StreamState,
    data: &str,
) -> Result<Vec<StreamDelta>> {
    if data.trim() == "[DONE]" {
        state.done = true;
        return Ok(Vec::new());
    }

    let payload: serde_json::Value =
        serde_json::from_str(data).map_err(|err| FrankClawError::ModelProvider {
            msg: format!("invalid streaming response chunk: {err}"),
        })?;
    let mut deltas = Vec::new();

    if payload["choices"]
        .as_array()
        .is_some_and(std::vec::Vec::is_empty)
    {
        state.usage = parse_usage(&payload);
        return Ok(deltas);
    }

    for choice in payload["choices"].as_array().into_iter().flatten() {
        if let Some(text) = choice["delta"]["content"].as_str().filter(|t| !t.is_empty()) {
            state.content.push_str(text);
            deltas.push(StreamDelta::Text(text.to_string()));
        }

        if let Some(tool_calls) = choice["delta"]["tool_calls"].as_array() {
            for tool_call in tool_calls {
                let index = tool_call["index"].as_u64().unwrap_or(0) as usize;
                let entry = state.tool_calls.entry(index).or_default();
                if let Some(id) = tool_call["id"].as_str() {
                    entry.id = id.to_string();
                }
                if let Some(name) = tool_call["function"]["name"].as_str() {
                    entry.name = restore_tool_name(name);
                }
                if !entry.started && !entry.id.is_empty() && !entry.name.is_empty() {
                    entry.started = true;
                    deltas.push(StreamDelta::ToolCallStart {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                    });
                }
                if let Some(arguments_delta) = tool_call["function"]["arguments"]
                    .as_str()
                    .filter(|a| !a.is_empty())
                {
                    entry.arguments.push_str(arguments_delta);
                    if entry.started {
                        deltas.push(StreamDelta::ToolCallDelta {
                            id: entry.id.clone(),
                            arguments: arguments_delta.to_string(),
                        });
                    }
                }
            }
        }

        let finish_reason = parse_finish_reason(choice["finish_reason"].as_str());
        if choice["finish_reason"].as_str().is_some() {
            state.finish_reason = finish_reason;
            if matches!(finish_reason, FinishReason::ToolUse) {
                for tool_call in state.tool_calls.values_mut() {
                    if tool_call.started && !tool_call.ended {
                        tool_call.ended = true;
                        deltas.push(StreamDelta::ToolCallEnd {
                            id: tool_call.id.clone(),
                        });
                    }
                }
            }
        }
    }

    Ok(deltas)
}

pub fn parse_usage(data: &serde_json::Value) -> Usage {
    Usage {
        input_tokens: data["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: data["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
        ..Default::default()
    }
}

pub fn parse_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop") => FinishReason::Stop,
        Some("length") => FinishReason::MaxTokens,
        Some("tool_calls") => FinishReason::ToolUse,
        Some("content_filter") => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::model::CompletionMessage;

    #[test]
    fn stream_state_accumulates_text_and_usage() {
        let mut state = StreamState::default();

        let deltas = apply_stream_event(
            &mut state,
            r#"{"choices":[{"delta":{"content":"hel"},"finish_reason":null}]}"#,
        )
        .expect("chunk should parse");
        assert_eq!(deltas, vec![StreamDelta::Text("hel".into())]);

        let deltas = apply_stream_event(
            &mut state,
            r#"{"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#,
        )
        .expect("chunk should parse");
        assert_eq!(deltas, vec![StreamDelta::Text("lo".into())]);
        state.usage = parse_usage(&serde_json::json!({
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }));

        let response = state.finish().expect("response should build");
        assert_eq!(response.content, "hello");
        assert_eq!(response.finish_reason, FinishReason::Stop);
    }

    #[test]
    fn stream_state_accumulates_tool_calls() {
        let mut state = StreamState::default();

        let deltas = apply_stream_event(
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

        let deltas = apply_stream_event(
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

    #[test]
    fn done_sentinel_marks_stream_complete() {
        let mut state = StreamState::default();
        let deltas = apply_stream_event(&mut state, "[DONE]").expect("should parse");
        assert!(deltas.is_empty());
        assert!(state.done);
    }

    #[test]
    fn empty_choices_captures_usage() {
        let mut state = StreamState::default();
        let deltas = apply_stream_event(
            &mut state,
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        )
        .expect("should parse");
        assert!(deltas.is_empty());
        assert_eq!(state.usage.input_tokens, 10);
        assert_eq!(state.usage.output_tokens, 5);
    }

    #[test]
    fn build_request_body_includes_image_content() {
        use frankclaw_core::model::ImageContent;
        let request = CompletionRequest {
            model_id: "gpt-4o".into(),
            messages: vec![CompletionMessage::with_images(
                "What is in this image?",
                vec![ImageContent {
                    mime_type: "image/jpeg".into(),
                    data: "abc123base64".into(),
                }],
            )],
            max_tokens: Some(1024),
            ..Default::default()
        };
        let body = build_request_body(&request);
        let msg = &body["messages"][0];
        assert_eq!(msg["role"], "user");
        let content = msg["content"].as_array().expect("content should be array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What is in this image?");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/jpeg;base64,abc123base64"
        );
    }

    #[test]
    fn build_request_body_text_only_when_no_images() {
        let request = CompletionRequest {
            model_id: "gpt-4o".into(),
            messages: vec![CompletionMessage::text(Role::User, "hello")],
            max_tokens: Some(1024),
            ..Default::default()
        };
        let body = build_request_body(&request);
        let msg = &body["messages"][0];
        // Should be plain text string, not content array
        assert_eq!(msg["content"], "hello");
    }

    #[test]
    fn build_request_body_multiple_images() {
        use frankclaw_core::model::ImageContent;
        let request = CompletionRequest {
            model_id: "gpt-4o".into(),
            messages: vec![CompletionMessage::with_images(
                "Compare these images",
                vec![
                    ImageContent {
                        mime_type: "image/png".into(),
                        data: "img1data".into(),
                    },
                    ImageContent {
                        mime_type: "image/jpeg".into(),
                        data: "img2data".into(),
                    },
                ],
            )],
            ..Default::default()
        };
        let body = build_request_body(&request);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("content should be array");
        assert_eq!(content.len(), 3); // 1 text + 2 images
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,img1data");
        assert_eq!(content[2]["image_url"]["url"], "data:image/jpeg;base64,img2data");
    }

    #[test]
    fn build_request_body_includes_parallel_tool_calls() {
        let request = CompletionRequest {
            model_id: "gpt-4o".into(),
            messages: vec![CompletionMessage::text(Role::User, "hello")],
            parallel_tool_calls: Some(false),
            ..Default::default()
        };
        let body = build_request_body(&request);
        assert_eq!(body["parallel_tool_calls"], false);
    }

    #[test]
    fn build_request_body_includes_seed() {
        let request = CompletionRequest {
            model_id: "gpt-4o".into(),
            messages: vec![CompletionMessage::text(Role::User, "hello")],
            seed: Some(42),
            ..Default::default()
        };
        let body = build_request_body(&request);
        assert_eq!(body["seed"], 42);
    }

    #[test]
    fn build_request_body_includes_response_format_json() {
        let request = CompletionRequest {
            model_id: "gpt-4o".into(),
            messages: vec![CompletionMessage::text(Role::User, "hello")],
            response_format: Some(ResponseFormat::JsonObject),
            ..Default::default()
        };
        let body = build_request_body(&request);
        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn build_request_body_includes_reasoning_effort() {
        let request = CompletionRequest {
            model_id: "o3-mini".into(),
            messages: vec![CompletionMessage::text(Role::User, "hello")],
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        let body = build_request_body(&request);
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn build_request_body_omits_none_advanced_params() {
        let request = CompletionRequest {
            model_id: "gpt-4o".into(),
            messages: vec![CompletionMessage::text(Role::User, "hello")],
            ..Default::default()
        };
        let body = build_request_body(&request);
        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("seed").is_none());
        assert!(body.get("response_format").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }
}
