use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use frankclaw_core::protocol::{Frame, Method, RequestFrame, ResponseFrame};
use frankclaw_core::types::ConnId;

use crate::state::{ClientState, GatewayState};

/// Maximum outbound queue depth per client.
/// If a client can't keep up, we drop it rather than block the gateway.
const CLIENT_QUEUE_CAPACITY: usize = 256;

/// Handle a single WebSocket connection lifecycle.
pub async fn handle_ws_connection(
    socket: WebSocket,
    state: Arc<GatewayState>,
    conn_id: ConnId,
    role: frankclaw_core::auth::AuthRole,
    remote_addr: Option<std::net::SocketAddr>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (client_tx, mut client_rx) = mpsc::channel::<String>(CLIENT_QUEUE_CAPACITY);

    // Register client.
    state.clients.insert(
        conn_id,
        ClientState {
            tx: client_tx.clone(),
            role,
            remote_addr,
            connected_at: chrono::Utc::now(),
        },
    );

    let _conn_id_display = conn_id;
    info!(%conn_id, ?role, "client connected");

    // Subscribe to server broadcasts.
    let mut broadcast_rx = state.broadcast.subscribe();

    // Outbound task: forward messages from client_rx and broadcasts to WebSocket.
    let outbound_state = state.clone();
    let outbound_conn = conn_id;
    let outbound = tokio::spawn(async move {
        loop {
            tokio::select! {
                // Messages targeted at this client.
                msg = client_rx.recv() => {
                    match msg {
                        Some(text) => {
                            if ws_tx.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                // Broadcast events to all clients.
                msg = broadcast_rx.recv() => {
                    match msg {
                        Ok(text) => {
                            if ws_tx.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(%outbound_conn, skipped = n, "client lagging, skipped events");
                        }
                        Err(_) => break,
                    }
                }
                // Shutdown signal.
                _ = outbound_state.shutdown.cancelled() => {
                    let _ = ws_tx.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    });

    // Inbound task: read WebSocket frames, dispatch RPC methods.
    while let Some(msg) = ws_rx.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let text_str: &str = &text;
                match serde_json::from_str::<RequestFrame>(text_str) {
                    Ok(request) => {
                        let response = dispatch_method(&state, conn_id, role, request).await;
                        let response_json = serde_json::to_string(&Frame::Response(response))
                            .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string());
                        if client_tx.send(response_json).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let err_response = serde_json::json!({
                            "type": "response",
                            "id": null,
                            "error": { "code": 400, "message": format!("invalid request: {e}") }
                        });
                        if client_tx
                            .send(err_response.to_string())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            Ok(Message::Ping(_)) => {
                // Axum handles ping/pong automatically.
            }
            Ok(Message::Close(_)) => break,
            Err(e) => {
                debug!(%conn_id, error = %e, "ws error");
                break;
            }
            _ => {}
        }
    }

    // Cleanup.
    outbound.abort();
    state.clients.remove(&conn_id);
    info!(%conn_id, "client disconnected");
}

/// Dispatch an RPC method to the appropriate handler.
async fn dispatch_method(
    state: &Arc<GatewayState>,
    _conn_id: ConnId,
    role: frankclaw_core::auth::AuthRole,
    request: RequestFrame,
) -> ResponseFrame {
    // Authorization check.
    let min_role = request.method.min_role();
    if role < min_role {
        return ResponseFrame::err(
            request.id,
            403,
            format!("insufficient permissions for {:?}", request.method),
        );
    }

    // Route to handler.
    match request.method {
        Method::HealthProbe => {
            ResponseFrame::ok(request.id, serde_json::json!({ "status": "ok" }))
        }
        Method::ConfigGet => {
            let config = state.current_config();
            // Redact sensitive fields before sending.
            let safe_config = redact_config(&config);
            ResponseFrame::ok(request.id, safe_config)
        }
        Method::ChatSend => {
            crate::methods::chat_send(state, request).await
        }
        Method::SessionsList => {
            crate::methods::sessions_list(state, request).await
        }
        Method::SessionsGet => {
            crate::methods::sessions_get(state, request).await
        }
        Method::SessionsReset => {
            crate::methods::sessions_reset(state, request).await
        }
        Method::ChatHistory => {
            crate::methods::chat_history(state, request).await
        }
        Method::ChannelsList => {
            let channels: Vec<_> = state
                .channels
                .channels()
                .keys()
                .map(|channel| channel.as_str().to_string())
                .collect();
            ResponseFrame::ok(request.id, serde_json::json!({ "channels": channels }))
        }
        Method::ChannelsStatus => {
            let mut statuses = Vec::new();
            for (channel_id, channel) in state.channels.channels() {
                let health = channel.health().await;
                statuses.push(serde_json::json!({
                    "id": channel_id.as_str(),
                    "label": channel.label(),
                    "health": health,
                }));
            }
            ResponseFrame::ok(request.id, serde_json::json!({ "channels": statuses }))
        }
        Method::ModelsList => {
            let models = serde_json::to_value(state.runtime.list_models()).unwrap_or_default();
            ResponseFrame::ok(request.id, serde_json::json!({ "models": models }))
        }
        Method::CanvasGet => {
            crate::methods::canvas_get(state, request).await
        }
        Method::CanvasSet => {
            crate::methods::canvas_set(state, request).await
        }
        Method::CanvasClear => {
            crate::methods::canvas_clear(state, request).await
        }
        Method::WebhooksTest => {
            crate::methods::webhooks_test(state, request).await
        }
        _ => ResponseFrame::err(
            request.id,
            501,
            format!("{:?} not yet implemented", request.method),
        ),
    }
}

/// Remove secrets from config before sending to clients.
fn redact_config(config: &frankclaw_core::config::FrankClawConfig) -> serde_json::Value {
    let mut val = serde_json::to_value(config).unwrap_or(serde_json::json!({}));
    if let Some(obj) = val.as_object_mut() {
        // Redact auth tokens.
        if let Some(gw) = obj.get_mut("gateway") {
            if let Some(auth) = gw.get_mut("auth") {
                if let Some(token) = auth.get_mut("token") {
                    *token = serde_json::json!("[REDACTED]");
                }
                if let Some(hash) = auth.get_mut("hash") {
                    *hash = serde_json::json!("[REDACTED]");
                }
            }
        }
        // Redact model provider API keys.
        if let Some(models) = obj.get_mut("models") {
            if let Some(providers) = models.get_mut("providers") {
                if let Some(arr) = providers.as_array_mut() {
                    for p in arr {
                        if let Some(key) = p.get_mut("api_key_ref") {
                            *key = serde_json::json!("[REDACTED]");
                        }
                    }
                }
            }
        }
        if let Some(hooks) = obj.get_mut("hooks") {
            if let Some(token) = hooks.get_mut("token") {
                *token = serde_json::json!("[REDACTED]");
            }
        }
    }
    val
}
