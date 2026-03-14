use std::sync::Arc;

use frankclaw_core::protocol::{EventFrame, EventType, Frame, RequestFrame, ResponseFrame};
use frankclaw_core::session::SessionStore;
use frankclaw_core::types::{AgentId, ConnId, SessionKey};

use crate::state::GatewayState;

/// Handle `sessions.list` method.
pub async fn sessions_list(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let agent_id = request
        .params
        .get("agent_id")
        .and_then(|v| v.as_str()).map_or_else(AgentId::default_agent, AgentId::new);

    let limit = request
        .params
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(50) as usize;

    let offset = request
        .params
        .get("offset")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;

    match state.sessions.list(&agent_id, limit, offset).await {
        Ok(sessions) => {
            let json = serde_json::to_value(&sessions).unwrap_or_default();
            ResponseFrame::ok(request.id, serde_json::json!({ "sessions": json }))
        }
        Err(e) => ResponseFrame::err(request.id, 500, e.to_string()),
    }
}

/// Handle `chat.history` method.
pub async fn chat_history(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let session_key = match parse_session_key_param(&request) {
        Ok(key) => key,
        Err(response) => return response,
    };

    let limit = request
        .params
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(100) as usize;

    let before_seq = request
        .params
        .get("before_seq")
        .and_then(serde_json::Value::as_u64);

    match state
        .sessions
        .get_transcript(&session_key, limit, before_seq)
        .await
    {
        Ok(entries) => {
            let json = serde_json::to_value(&entries).unwrap_or_default();
            ResponseFrame::ok(request.id, serde_json::json!({ "entries": json }))
        }
        Err(e) => ResponseFrame::err(request.id, 500, e.to_string()),
    }
}

/// Handle `sessions.get` method.
pub async fn sessions_get(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let session_key = match parse_session_key_param(&request) {
        Ok(key) => key,
        Err(response) => return response,
    };

    match state.sessions.get(&session_key).await {
        Ok(Some(session)) => {
            let json = serde_json::to_value(&session).unwrap_or_default();
            ResponseFrame::ok(request.id, serde_json::json!({ "session": json }))
        }
        Ok(None) => ResponseFrame::err(request.id, 404, "session not found"),
        Err(err) => ResponseFrame::err(request.id, 500, err.to_string()),
    }
}

/// Handle `sessions.reset` method.
pub async fn sessions_reset(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let session_key = match parse_session_key_param(&request) {
        Ok(key) => key,
        Err(response) => return response,
    };

    match state.sessions.clear_transcript(&session_key).await {
        Ok(()) => {
            let event = Frame::Event(EventFrame {
                event: EventType::SessionUpdated,
                payload: serde_json::json!({
                    "session_key": session_key.as_str(),
                    "action": "reset",
                }),
            });
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = state.broadcast.send(json);
            }
            ResponseFrame::ok(
                request.id,
                serde_json::json!({
                    "session_key": session_key.as_str(),
                    "status": "reset",
                }),
            )
        }
        Err(err) => ResponseFrame::err(request.id, 500, err.to_string()),
    }
}

/// Handle `chat.send` method.
#[expect(clippy::too_many_lines, reason = "request validation and orchestration in one handler")]
pub async fn chat_send(
    state: &Arc<GatewayState>,
    conn_id: ConnId,
    request: RequestFrame,
) -> ResponseFrame {
    let max_message_bytes = state
        .current_config()
        .security
        .max_webhook_body_bytes;

    let message = match request.params.get("message").and_then(|value| value.as_str()) {
        Some(message) if !message.trim().is_empty() => {
            if message.len() > max_message_bytes {
                return ResponseFrame::err(
                    request.id,
                    413,
                    format!("message exceeds maximum size ({max_message_bytes} bytes)"),
                );
            }
            message.to_string()
        }
        _ => return ResponseFrame::err(request.id, 400, "message is required"),
    };

    let agent_id = request
        .params
        .get("agent_id")
        .and_then(|value| value.as_str())
        .map(AgentId::new);
    let session_key = request
        .params
        .get("session_key")
        .and_then(|value| value.as_str())
        .map(frankclaw_core::types::SessionKey::from_raw);
    let model_id = request
        .params
        .get("model_id")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let max_tokens = request
        .params
        .get("max_tokens")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    let temperature = request
        .params
        .get("temperature")
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32);
    let stream = request
        .params
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let request_id = request.id.clone();

    let stream_tx = if stream {
        state.clients.get(&conn_id).map(|client| {
            let client_tx = client.tx.clone();
            let request_id = request_id.clone();
            let (delta_tx, mut delta_rx) = tokio::sync::mpsc::channel(64);
            tokio::spawn(async move {
                while let Some(delta) = delta_rx.recv().await {
                    let payload = match delta {
                        frankclaw_core::model::StreamDelta::Text(text) => serde_json::json!({
                            "request_id": request_id,
                            "kind": "text",
                            "delta": text,
                        }),
                        frankclaw_core::model::StreamDelta::ToolCallStart { id, name } => serde_json::json!({
                            "request_id": request_id,
                            "kind": "tool_call_start",
                            "tool_call_id": id,
                            "tool_name": name,
                        }),
                        frankclaw_core::model::StreamDelta::ToolCallDelta { id, arguments } => serde_json::json!({
                            "request_id": request_id,
                            "kind": "tool_call_delta",
                            "tool_call_id": id,
                            "arguments_delta": arguments,
                        }),
                        frankclaw_core::model::StreamDelta::ToolCallEnd { id } => serde_json::json!({
                            "request_id": request_id,
                            "kind": "tool_call_end",
                            "tool_call_id": id,
                        }),
                        frankclaw_core::model::StreamDelta::Done { usage } => serde_json::json!({
                            "request_id": request_id,
                            "kind": "done",
                            "usage": usage,
                        }),
                        frankclaw_core::model::StreamDelta::Error(message) => serde_json::json!({
                            "request_id": request_id,
                            "kind": "error",
                            "message": message,
                        }),
                    };
                    let frame = Frame::Event(EventFrame {
                        event: EventType::ChatDelta,
                        payload,
                    });
                    if let Ok(json) = serde_json::to_string(&frame)
                        && client_tx.send(json).await.is_err() {
                            break;
                        }
                }
            });
            delta_tx
        })
    } else {
        None
    };

    match state
        .runtime
        .chat(frankclaw_runtime::ChatRequest {
            agent_id,
            session_key,
            message,
            attachments: Vec::new(),
            model_id,
            max_tokens,
            temperature,
            stream_tx,
            thinking_budget: None,
            channel_id: None,
            channel_capabilities: None,
        })
        .await
    {
        Ok(response) => {
            let event = Frame::Event(EventFrame {
                event: EventType::ChatComplete,
                payload: serde_json::json!({
                    "request_id": request_id,
                    "session_key": response.session_key.as_str(),
                    "model_id": response.model_id,
                    "content": response.content,
                }),
            });
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = state.broadcast.send(json);
            }

            ResponseFrame::ok(
                request.id,
                serde_json::json!({
                    "session_key": response.session_key.as_str(),
                    "model_id": response.model_id,
                    "content": response.content,
                    "usage": response.usage,
                }),
            )
        }
        Err(err) => {
            let event = Frame::Event(EventFrame {
                event: EventType::ChatError,
                payload: serde_json::json!({
                    "request_id": request_id,
                    "message": err.to_string(),
                }),
            });
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = state.broadcast.send(json);
            }
            ResponseFrame::err(request.id, err.status_code(), err.to_string())
        }
    }
}

/// Handle `webhooks.test` method.
pub async fn webhooks_test(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let mapping_id = match request.params.get("mapping_id").and_then(|value| value.as_str()) {
        Some(mapping_id) if !mapping_id.trim().is_empty() => mapping_id,
        _ => return ResponseFrame::err(request.id, 400, "mapping_id is required"),
    };
    let Some(payload) = request.params.get("payload") else {
        return ResponseFrame::err(request.id, 400, "payload is required");
    };

    let config = state.current_config();
    let resolved = match crate::webhooks::resolve_request(&config, mapping_id, payload) {
        Ok(resolved) => resolved,
        Err(err) => {
            crate::audit::log_failure(
                "webhook.test",
                serde_json::json!({
                    "mapping_id": mapping_id,
                    "reason": err.to_string(),
                }),
            );
            return ResponseFrame::err(request.id, err.status_code(), err.to_string());
        }
    };

    match crate::webhooks::execute_request(state, resolved).await {
        Ok(response) => {
            crate::audit::log_event(
                "webhook.test",
                "success",
                serde_json::json!({
                    "mapping_id": mapping_id,
                    "session_key": response.session_key.as_str(),
                    "model_id": response.model_id,
                }),
            );
            ResponseFrame::ok(
                request.id,
                serde_json::json!({
                    "session_key": response.session_key.as_str(),
                    "model_id": response.model_id,
                    "content": response.content,
                    "usage": response.usage,
                }),
            )
        }
        Err(err) => {
            crate::audit::log_failure(
                "webhook.test",
                serde_json::json!({
                    "mapping_id": mapping_id,
                    "reason": err.to_string(),
                }),
            );
            ResponseFrame::err(request.id, err.status_code(), err.to_string())
        }
    }
}

/// Handle `canvas.get` method.
pub async fn canvas_get(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let canvas = state
        .canvas
        .get(&canvas_id_from_params(&request.params))
        .await;
    ResponseFrame::ok(request.id, serde_json::json!({ "canvas": canvas }))
}

/// Handle `canvas.export` method.
pub async fn canvas_export(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let canvas_id = canvas_id_from_params(&request.params);
    let Some(canvas) = state.canvas.get(&canvas_id).await else {
        return ResponseFrame::err(request.id, 404, "canvas not found");
    };
    let format = crate::canvas::CanvasExportFormat::parse(
        request.params.get("format").and_then(|value| value.as_str()),
    );
    let filename = format!(
        "{}.{}",
        sanitize_canvas_export_name(&canvas.id),
        format.extension()
    );

    ResponseFrame::ok(
        request.id,
        serde_json::json!({
            "canvas_id": canvas.id,
            "format": format.label(),
            "mime_type": format.mime_type(),
            "filename": filename,
            "content": crate::canvas::export_document(&canvas, format),
        }),
    )
}

/// Handle `canvas.set` method.
pub async fn canvas_set(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let title = request
        .params
        .get("title")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let body = request
        .params
        .get("body")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let session_key = request
        .params
        .get("session_key")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let canvas_id = canvas_id_from_params(&request.params);
    let blocks = match parse_canvas_blocks(request.params.get("blocks")) {
        Ok(blocks) => blocks,
        Err(message) => return ResponseFrame::err(request.id, 400, message),
    };

    if title.is_empty() && body.is_empty() && blocks.is_empty() {
        return ResponseFrame::err(
            request.id,
            400,
            "canvas.set requires a non-empty title, body, or blocks",
        );
    }

    let document = match state.canvas.set(crate::canvas::CanvasDocument {
        id: canvas_id,
        title,
        body,
        session_key,
        blocks,
        revision: 0,
        updated_at: chrono::Utc::now(),
    }).await {
        Ok(doc) => doc,
        Err(e) => return ResponseFrame::err(request.id, 400, e.to_string()),
    };
    broadcast_canvas_update(state, &document.id, Some(&document));

    ResponseFrame::ok(request.id, serde_json::json!({ "canvas": document }))
}

/// Handle `canvas.patch` method.
pub async fn canvas_patch(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let title = request
        .params
        .get("title")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .map(str::to_string);
    let body = request
        .params
        .get("body")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .map(str::to_string);
    let session_key = request.params.get("session_key").and_then(|value| {
        if value.is_null() {
            Some(None)
        } else {
            value.as_str().map(|session_key| Some(session_key.to_string()))
        }
    });
    let append_blocks = match parse_canvas_blocks(request.params.get("append_blocks")) {
        Ok(blocks) => blocks,
        Err(message) => return ResponseFrame::err(request.id, 400, message),
    };
    if title.is_none() && body.is_none() && session_key.is_none() && append_blocks.is_empty() {
        return ResponseFrame::err(request.id, 400, "canvas.patch requires at least one change");
    }

    let expected_revision = request
        .params
        .get("expected_revision")
        .and_then(serde_json::Value::as_u64);
    let document = match state
        .canvas
        .patch(
            &canvas_id_from_params(&request.params),
            crate::canvas::CanvasPatch {
                title,
                body,
                session_key,
                append_blocks,
                expected_revision,
            },
        )
        .await {
        Ok(doc) => doc,
        Err(e) => return ResponseFrame::err(request.id, 409, e.to_string()),
    };
    broadcast_canvas_update(state, &document.id, Some(&document));
    ResponseFrame::ok(request.id, serde_json::json!({ "canvas": document }))
}

/// Handle `canvas.clear` method.
pub async fn canvas_clear(
    state: &Arc<GatewayState>,
    request: RequestFrame,
) -> ResponseFrame {
    let canvas_id = canvas_id_from_params(&request.params);
    state.canvas.clear(&canvas_id).await;
    broadcast_canvas_update(state, &canvas_id, None);
    ResponseFrame::ok(request.id, serde_json::json!({ "cleared": true, "canvas_id": canvas_id }))
}

fn broadcast_canvas_update(
    state: &Arc<GatewayState>,
    canvas_id: &str,
    canvas: Option<&crate::canvas::CanvasDocument>,
) {
    let event = Frame::Event(EventFrame {
        event: EventType::CanvasUpdated,
        payload: serde_json::json!({
            "canvas_id": canvas_id,
            "canvas": canvas,
        }),
    });
    if let Ok(json) = serde_json::to_string(&event) {
        let _ = state.broadcast.send(json);
    }
}

fn sanitize_canvas_export_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "canvas".to_string()
    } else {
        sanitized
    }
}

fn canvas_id_from_params(params: &serde_json::Value) -> String {
    crate::canvas::CanvasStore::key_for(
        params.get("canvas_id").and_then(|value| value.as_str()),
        params.get("session_key").and_then(|value| value.as_str()),
    )
}

fn parse_canvas_blocks(
    value: Option<&serde_json::Value>,
) -> std::result::Result<Vec<crate::canvas::CanvasBlock>, &'static str> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    serde_json::from_value(value.clone()).map_err(|_| "invalid canvas blocks payload")
}

fn parse_session_key_param(
    request: &RequestFrame,
) -> std::result::Result<SessionKey, ResponseFrame> {
    request
        .params
        .get("session_key")
        .and_then(|value| value.as_str())
        .map(SessionKey::from_raw)
        .ok_or_else(|| ResponseFrame::err(request.id.clone(), 400, "session_key is required"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use async_trait::async_trait;
    use frankclaw_channels::ChannelSet;
    use frankclaw_core::model::{
        CompletionRequest, CompletionResponse, FinishReason, InputModality, ModelApi,
        ModelCompat, ModelCost, ModelDef, ModelProvider, StreamDelta, Usage,
    };
    use frankclaw_core::auth::AuthRole;
    use frankclaw_core::protocol::Method;
    use frankclaw_core::session::{SessionEntry, SessionScoping, SessionStore, TranscriptEntry};
    use frankclaw_core::types::{AgentId, ChannelId, ConnId, RequestId, Role};
    use frankclaw_media::MediaStore;
    use frankclaw_sessions::SqliteSessionStore;
    use tokio::time::{Duration, timeout};

    use crate::delivery::{StoredReplyMetadata, set_last_reply_in_metadata};
    use crate::pairing::PairingStore;

    use super::*;

    // ── RAII guard for temp directories ──────────────────────────────

    struct TempTestDir {
        path: PathBuf,
    }

    impl TempTestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{}-{}", label, uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("temp dir should create");
            Self { path }
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    // ── Test helpers ─────────────────────────────────────────────────

    fn make_request(method: Method, params: serde_json::Value) -> RequestFrame {
        RequestFrame {
            id: RequestId::Text("1".into()),
            method,
            params,
        }
    }

    fn make_session(key: SessionKey) -> SessionEntry {
        SessionEntry {
            key,
            agent_id: AgentId::default_agent(),
            channel: ChannelId::new("web"),
            account_id: "default".into(),
            scoping: SessionScoping::PerChannelPeer,
            created_at: chrono::Utc::now(),
            last_message_at: Some(chrono::Utc::now()),
            thread_id: None,
            metadata: serde_json::json!({}),
        }
    }

    async fn setup_canvas(state: &Arc<GatewayState>) {
        canvas_set(state, make_request(Method::CanvasSet, serde_json::json!({
            "canvas_id": "ops",
            "title": "Ops",
            "body": "Current deployment summary",
            "session_key": "default:web:control",
            "blocks": [{"kind": "note", "text": "deploy window open"}],
        }))).await;
    }

    // ── Mock providers ───────────────────────────────────────────────

    struct MockProvider;

    #[async_trait]
    impl ModelProvider for MockProvider {
        fn id(&self) -> &str {
            "mock"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _stream_tx: Option<tokio::sync::mpsc::Sender<frankclaw_core::model::StreamDelta>>,
        ) -> frankclaw_core::error::Result<CompletionResponse> {
            Ok(CompletionResponse {
                content: "mock reply".into(),
                tool_calls: Vec::new(),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                finish_reason: FinishReason::Stop,
            })
        }

        async fn list_models(&self) -> frankclaw_core::error::Result<Vec<ModelDef>> {
            Ok(vec![ModelDef {
                id: "mock-model".into(),
                name: "mock-model".into(),
                api: ModelApi::Ollama,
                reasoning: false,
                input: vec![InputModality::Text],
                cost: ModelCost::default(),
                context_window: 4096,
                max_output_tokens: 1024,
                compat: ModelCompat::default(),
            }])
        }

        async fn health(&self) -> bool {
            true
        }
    }

    struct StreamingMockProvider;

    #[async_trait]
    impl ModelProvider for StreamingMockProvider {
        fn id(&self) -> &str {
            "streaming-mock"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
        ) -> frankclaw_core::error::Result<CompletionResponse> {
            if let Some(stream_tx) = stream_tx {
                let _ = stream_tx.send(StreamDelta::Text("hello ".into())).await;
                let _ = stream_tx.send(StreamDelta::Text("world".into())).await;
                let _ = stream_tx
                    .send(StreamDelta::Done {
                        usage: Some(Usage {
                            input_tokens: 1,
                            output_tokens: 2,
                            cache_read_tokens: None,
                            cache_write_tokens: None,
                        }),
                    })
                    .await;
            }

            Ok(CompletionResponse {
                content: "hello world".into(),
                tool_calls: Vec::new(),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                finish_reason: FinishReason::Stop,
            })
        }

        async fn list_models(&self) -> frankclaw_core::error::Result<Vec<ModelDef>> {
            Ok(vec![ModelDef {
                id: "mock-model".into(),
                name: "mock-model".into(),
                api: ModelApi::OpenaiResponses,
                reasoning: false,
                input: vec![InputModality::Text],
                cost: ModelCost::default(),
                context_window: 4096,
                max_output_tokens: 1024,
                compat: ModelCompat::default(),
            }])
        }

        async fn health(&self) -> bool {
            true
        }
    }

    // ── State builders ───────────────────────────────────────────────

    async fn build_test_state(
        temp_dir: &PathBuf,
    ) -> (Arc<GatewayState>, Arc<SqliteSessionStore>) {
        build_test_state_with_providers(temp_dir, vec![Arc::new(MockProvider)]).await
    }

    async fn build_test_state_with_providers(
        temp_dir: &PathBuf,
        providers: Vec<Arc<dyn ModelProvider>>,
    ) -> (Arc<GatewayState>, Arc<SqliteSessionStore>) {
        std::fs::create_dir_all(temp_dir).expect("temp dir should exist");

        let sessions = Arc::new(
            SqliteSessionStore::open(&temp_dir.join("sessions.db"), None)
                .expect("sessions should open"),
        );
        let pairing = Arc::new(
            PairingStore::open(&temp_dir.join("pairings.json"))
                .expect("pairings should open"),
        );
        let media = Arc::new(
            MediaStore::new(temp_dir.join("media"), 1024 * 1024, 1)
                .expect("media store should open"),
        );
        let config = frankclaw_core::config::FrankClawConfig::default();
        let runtime = Arc::new(
            frankclaw_runtime::Runtime::from_providers(
                &config,
                sessions.clone() as Arc<dyn SessionStore>,
                providers,
            )
            .await
            .expect("runtime should build"),
        );
        let channels = Arc::new(ChannelSet::from_parts(HashMap::new(), None, None));
        (
            GatewayState::new(config, sessions.clone(), runtime, channels, pairing, media),
            sessions,
        )
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn sessions_get_returns_session_entry() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-get");
        let (state, sessions) = build_test_state(&temp.path).await;
        let session_key = SessionKey::from_raw("agent:main:web:default:user-1");
        let mut entry = make_session(session_key.clone());
        set_last_reply_in_metadata(
            &mut entry.metadata,
            &StoredReplyMetadata {
                channel: "web".into(),
                account_id: "default".into(),
                recipient_id: "user-1".into(),
                thread_id: None,
                reply_to: Some("incoming-1".into()),
                content: "mock reply".into(),
                platform_message_id: Some("outgoing-1".into()),
                status: "sent".into(),
                attempts: 1,
                retry_after_secs: None,
                error: None,
                chunks: Vec::new(),
                recorded_at: chrono::Utc::now(),
            },
        )
        .expect("metadata should serialize");
        sessions.upsert(&entry).await.expect("session should upsert");

        let response = sessions_get(
            &state,
            make_request(Method::SessionsGet, serde_json::json!({
                "session_key": session_key.as_str(),
            })),
        )
        .await;

        assert!(response.error.is_none());
        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|value| value["session"]["key"].as_str()),
            Some(session_key.as_str())
        );
    }

    #[tokio::test]
    async fn sessions_reset_clears_transcript_entries() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-reset");
        let (state, sessions) = build_test_state(&temp.path).await;
        let session_key = SessionKey::from_raw("agent:main:web:default:user-1");
        sessions
            .upsert(&make_session(session_key.clone()))
            .await
            .expect("session should upsert");
        sessions
            .append_transcript(
                &session_key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: "hello".into(),
                    timestamp: chrono::Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("transcript should append");

        let response = sessions_reset(
            &state,
            make_request(Method::SessionsReset, serde_json::json!({
                "session_key": session_key.as_str(),
            })),
        )
        .await;

        assert!(response.error.is_none());
        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert!(transcript.is_empty());
    }

    #[tokio::test]
    async fn chat_send_streams_delta_events_to_requesting_client() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-stream");
        let (state, _sessions) = build_test_state_with_providers(
            &temp.path,
            vec![Arc::new(StreamingMockProvider)],
        )
        .await;
        let conn_id = ConnId(42);
        let (client_tx, mut client_rx) = tokio::sync::mpsc::channel(16);
        state.clients.insert(
            conn_id,
            crate::state::ClientState {
                tx: client_tx,
                role: AuthRole::Editor,
                remote_addr: None,
                connected_at: chrono::Utc::now(),
            },
        );

        let response = chat_send(
            &state,
            conn_id,
            RequestFrame {
                id: RequestId::Text("stream-1".into()),
                method: Method::ChatSend,
                params: serde_json::json!({
                    "message": "hello",
                    "stream": true,
                }),
            },
        )
        .await;

        assert!(response.error.is_none());
        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|value| value["content"].as_str()),
            Some("hello world")
        );

        let first = timeout(Duration::from_secs(1), client_rx.recv())
            .await
            .expect("first delta should arrive")
            .expect("client should receive first delta");
        let second = timeout(Duration::from_secs(1), client_rx.recv())
            .await
            .expect("second delta should arrive")
            .expect("client should receive second delta");
        let third = timeout(Duration::from_secs(1), client_rx.recv())
            .await
            .expect("done event should arrive")
            .expect("client should receive done event");

        for (frame, expected_kind, expected_text) in [
            (first, "text", Some("hello ")),
            (second, "text", Some("world")),
            (third, "done", None),
        ] {
            let frame: Frame = serde_json::from_str(&frame).expect("frame should deserialize");
            let Frame::Event(event) = frame else {
                panic!("expected event frame");
            };
            assert_eq!(event.event, frankclaw_core::protocol::EventType::ChatDelta);
            assert_eq!(event.payload["request_id"], serde_json::json!("stream-1"));
            assert_eq!(event.payload["kind"], serde_json::json!(expected_kind));
            if let Some(expected_text) = expected_text {
                assert_eq!(event.payload["delta"], serde_json::json!(expected_text));
            }
        }

        state.clients.remove(&conn_id);
    }

    #[tokio::test]
    async fn webhooks_test_executes_runtime_chat() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-webhook");
        let (state, sessions) = build_test_state(&temp.path).await;
        let mut config = state.current_config().as_ref().clone();
        config.hooks.enabled = true;
        config.hooks.token = Some("secret".into());
        config.hooks.mappings = vec![serde_json::json!({
            "id": "incoming",
            "session_key": "default:web:hook-control",
        })];
        state.reload_config(config);

        let response = webhooks_test(
            &state,
            make_request(Method::WebhooksTest, serde_json::json!({
                "mapping_id": "incoming",
                "payload": {
                    "message": "hello from hook"
                }
            })),
        )
        .await;

        assert!(response.error.is_none());
        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|value| value["content"].as_str()),
            Some("mock reply")
        );

        let transcript = sessions
            .get_transcript(&SessionKey::from_raw("default:web:hook-control"), 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].content, "hello from hook");
        assert_eq!(transcript[1].content, "mock reply");
    }

    #[tokio::test]
    async fn canvas_set_creates_document() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-canvas-set");
        let (state, _sessions) = build_test_state(&temp.path).await;

        let set_response = canvas_set(
            &state,
            make_request(Method::CanvasSet, serde_json::json!({
                "canvas_id": "ops",
                "title": "Ops",
                "body": "Current deployment summary",
                "session_key": "default:web:control",
                "blocks": [{"kind": "note", "text": "deploy window open"}],
            })),
        )
        .await;

        assert!(set_response.error.is_none());
        assert_eq!(
            state.canvas.get("ops").await.expect("canvas should exist").title,
            "Ops"
        );
        assert_eq!(
            state
                .canvas
                .get("ops")
                .await
                .expect("canvas should exist")
                .blocks
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn canvas_get_returns_document() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-canvas-get");
        let (state, _sessions) = build_test_state(&temp.path).await;
        setup_canvas(&state).await;

        let get_response = canvas_get(
            &state,
            make_request(Method::CanvasGet, serde_json::json!({
                "canvas_id": "ops",
            })),
        )
        .await;

        assert!(get_response.error.is_none());
        assert_eq!(
            get_response
                .result
                .as_ref()
                .and_then(|value| value["canvas"]["body"].as_str()),
            Some("Current deployment summary")
        );
        assert_eq!(
            get_response
                .result
                .as_ref()
                .and_then(|value| value["canvas"]["revision"].as_u64()),
            Some(1)
        );
    }

    #[tokio::test]
    async fn canvas_export_produces_markdown() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-canvas-export");
        let (state, _sessions) = build_test_state(&temp.path).await;
        setup_canvas(&state).await;

        let export_response = canvas_export(
            &state,
            make_request(Method::CanvasExport, serde_json::json!({
                "canvas_id": "ops",
                "format": "markdown",
            })),
        )
        .await;

        assert!(export_response.error.is_none());
        assert_eq!(
            export_response
                .result
                .as_ref()
                .and_then(|value| value["filename"].as_str()),
            Some("ops.md")
        );
        assert!(
            export_response
                .result
                .as_ref()
                .and_then(|value| value["content"].as_str())
                .expect("export should include markdown content")
                .contains("Current deployment summary")
        );
    }

    #[tokio::test]
    async fn canvas_patch_appends_blocks() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-canvas-patch");
        let (state, _sessions) = build_test_state(&temp.path).await;
        setup_canvas(&state).await;

        let patch_response = canvas_patch(
            &state,
            make_request(Method::CanvasPatch, serde_json::json!({
                "canvas_id": "ops",
                "append_blocks": [{"kind": "markdown", "text": "## Next steps"}]
            })),
        )
        .await;

        assert!(patch_response.error.is_none());
        assert_eq!(
            state
                .canvas
                .get("ops")
                .await
                .expect("canvas should exist")
                .blocks
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn canvas_clear_removes_document() {
        let temp = TempTestDir::new("frankclaw-gateway-methods-canvas-clear");
        let (state, _sessions) = build_test_state(&temp.path).await;
        setup_canvas(&state).await;

        let clear_response = canvas_clear(
            &state,
            make_request(Method::CanvasClear, serde_json::json!({
                "canvas_id": "ops",
            })),
        )
        .await;

        assert!(clear_response.error.is_none());
        assert!(state.canvas.get("ops").await.is_none());
    }
}
