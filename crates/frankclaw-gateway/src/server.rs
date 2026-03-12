use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{
        ConnectInfo, Json, Path, Query, State, WebSocketUpgrade,
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;

use frankclaw_core::channel::{InboundMessage, OutboundMessage};
use frankclaw_core::config::{BindMode, ChannelDmPolicy, FrankClawConfig};
use frankclaw_core::session::SessionStore;
use frankclaw_core::types::MediaId;
use frankclaw_cron::{CronJob, CronService};
use frankclaw_media::MediaStore;
use frankclaw_runtime::Runtime;
use frankclaw_sessions::SqliteSessionStore;

use crate::auth::{authenticate, validate_bind_auth, AuthCredential};
use crate::audit::{log_event, log_failure};
use crate::delivery::{DeliveryRecord, StoredReplyChunk, StoredReplyMetadata, deliver_outbound_message, set_last_reply_in_metadata};
use crate::pairing::PairingStore;
use crate::rate_limit::AuthRateLimiter;
use crate::state::GatewayState;

const SESSION_MAINTENANCE_INTERVAL_SECS: u64 = 15 * 60;
const CONFIG_WATCH_INTERVAL_SECS: u64 = 2;

/// Build and start the gateway server.
pub async fn run(
    config: FrankClawConfig,
    config_path: Option<PathBuf>,
    sessions: Arc<SqliteSessionStore>,
    runtime: Arc<Runtime>,
    pairing: Arc<PairingStore>,
    cron: Arc<CronService>,
    media: Arc<MediaStore>,
) -> anyhow::Result<()> {
    // Validate that bind + auth combination is safe.
    validate_bind_auth(&config.gateway.bind, &config.gateway.auth)?;

    let rate_limiter = Arc::new(AuthRateLimiter::new(config.gateway.rate_limit.clone()));
    let bind_addr = resolve_bind_addr(&config.gateway.bind, config.gateway.port);
    let channels = Arc::new(frankclaw_channels::load_from_config(&config)?);
    let state = GatewayState::new(config, sessions, runtime, channels, pairing, media);
    log_loaded_skills(&state);
    start_config_watcher(state.clone(), config_path);
    start_channel_runtime(state.clone());
    start_session_maintenance(state.clone());
    start_cron_runtime(state.clone(), cron).await?;

    let app = build_router(state.clone(), rate_limiter);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!(%bind_addr, "gateway listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(state.shutdown.clone()))
    .await?;

    info!("gateway stopped");
    Ok(())
}

fn build_router(
    state: Arc<GatewayState>,
    rate_limiter: Arc<AuthRateLimiter>,
) -> Router {
    Router::new()
        .route("/", get(crate::ui::index))
        // WebSocket endpoint.
        .route("/ws", get(ws_handler))
        // Health probes (no auth required).
        .route("/health", get(health_handler))
        .route("/ready", get(readiness_handler))
        // Local web channel ingress / polling.
        .route("/api/web/inbound", post(web_inbound_handler))
        .route("/api/web/outbound", get(web_outbound_handler))
        .route("/api/media/upload", post(media_upload_handler))
        .route("/api/media/{media_id}", get(media_download_handler))
        .route(
            "/api/whatsapp/webhook",
            get(whatsapp_webhook_verify_handler).post(whatsapp_webhook_inbound_handler),
        )
        .route("/api/pairing/pending", get(pairing_pending_handler))
        .route("/api/pairing/approve", post(pairing_approve_handler))
        .route("/hooks/{mapping_id}", post(webhook_handler))
        // State.
        .with_state(AppState {
            gateway: state,
            rate_limiter,
        })
        // Middleware layers.
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

#[derive(Clone)]
struct AppState {
    gateway: Arc<GatewayState>,
    rate_limiter: Arc<AuthRateLimiter>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct AuthQuery {
    token: Option<String>,
    password: Option<String>,
    identity: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct WebOutboundQuery {
    #[serde(flatten)]
    auth: AuthQuery,
    recipient_id: Option<String>,
    account_id: Option<String>,
}

/// WebSocket upgrade handler with auth.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<AuthQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let config = state.gateway.current_config();
    // Extract credential from the configured auth mode.
    let credential = extract_credential(&headers, Some(&query), &config.gateway.auth);

    // Authenticate.
    match authenticate(
        &config.gateway.auth,
        &credential,
        Some(&addr),
        &state.rate_limiter,
    ) {
        Ok(role) => {
            let conn_id = state.gateway.alloc_conn_id();
            let gw = state.gateway.clone();
            let max_msg = config.gateway.max_ws_message_bytes;

            ws.max_message_size(max_msg)
                .on_upgrade(move |socket| {
                    crate::ws::handle_ws_connection(socket, gw, conn_id, role, Some(addr))
                })
                .into_response()
        }
        Err(e) => {
            log_failure(
                "gateway.ws_auth",
                serde_json::json!({
                    "remote_addr": addr.to_string(),
                    "status_code": e.status_code(),
                    "reason": e.to_string(),
                }),
            );
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, e.to_string()).into_response()
        }
    }
}

/// Extract auth credential from HTTP headers.
fn extract_credential(
    headers: &HeaderMap,
    query: Option<&AuthQuery>,
    mode: &frankclaw_core::auth::AuthMode,
) -> AuthCredential {
    match mode {
        frankclaw_core::auth::AuthMode::Token { .. } => {
            if let Some(token) = query.and_then(|query| query.token.as_deref()) {
                return AuthCredential::BearerToken(secrecy::SecretString::from(
                    token.to_string(),
                ));
            }
            if let Some(auth) = headers.get("authorization") {
                if let Ok(value) = auth.to_str() {
                    if let Some(token) = value.strip_prefix("Bearer ") {
                        return AuthCredential::BearerToken(secrecy::SecretString::from(
                            token.to_string(),
                        ));
                    }
                }
            }
        }
        frankclaw_core::auth::AuthMode::Password { .. } => {
            if let Some(password) = query.and_then(|query| query.password.as_deref()) {
                return AuthCredential::Password(secrecy::SecretString::from(
                    password.to_string(),
                ));
            }
            if let Some(password) = headers.get("x-frankclaw-password") {
                if let Ok(value) = password.to_str() {
                    return AuthCredential::Password(secrecy::SecretString::from(
                        value.to_string(),
                    ));
                }
            }
            if let Some(auth) = headers.get("authorization") {
                if let Ok(value) = auth.to_str() {
                    if let Some(password) = value.strip_prefix("Password ") {
                        return AuthCredential::Password(secrecy::SecretString::from(
                            password.to_string(),
                        ));
                    }
                }
            }
        }
        frankclaw_core::auth::AuthMode::TrustedProxy { identity_header } => {
            if let Some(identity) = query.and_then(|query| query.identity.as_deref()) {
                return AuthCredential::ProxyIdentity(identity.to_string());
            }
            if let Some(identity) = headers.get(identity_header.as_str()) {
                if let Ok(value) = identity.to_str() {
                    return AuthCredential::ProxyIdentity(value.to_string());
                }
            }
        }
        frankclaw_core::auth::AuthMode::Tailscale => {
            for header_name in [
                "tailscale-user-login",
                "tailscale-user-name",
                "x-tailscale-user-login",
            ] {
                if let Some(identity) = headers.get(header_name) {
                    if let Ok(value) = identity.to_str() {
                        return AuthCredential::TailscaleIdentity(value.to_string());
                    }
                }
            }
        }
        frankclaw_core::auth::AuthMode::None => {}
    }

    AuthCredential::None
}

/// Health check (always 200 — proves the process is running).
async fn health_handler() -> StatusCode {
    StatusCode::OK
}

/// Readiness check (200 when gateway is ready to serve).
async fn readiness_handler(State(state): State<AppState>) -> StatusCode {
    if state.gateway.shutdown.is_cancelled() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    }
}

#[derive(Debug, serde::Deserialize)]
struct WebInboundRequest {
    sender_id: String,
    message: Option<String>,
    agent_id: Option<String>,
    session_key: Option<String>,
    #[serde(default = "default_web_account_id")]
    account_id: String,
    sender_name: Option<String>,
    thread_id: Option<String>,
    #[serde(default)]
    is_group: bool,
    #[serde(default)]
    is_mention: bool,
    #[serde(default)]
    attachments: Vec<WebInboundAttachment>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct WebInboundAttachment {
    media_id: String,
    mime_type: String,
    filename: Option<String>,
    size_bytes: Option<u64>,
}

fn default_web_account_id() -> String {
    "default".to_string()
}

async fn web_inbound_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<WebInboundRequest>,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers, Some(&query)) {
        return response;
    }

    // Clamp identifiers to prevent memory exhaustion from maliciously long strings.
    let sender_id = if body.sender_id.len() > 255 {
        body.sender_id[..255].to_string()
    } else {
        body.sender_id
    };
    let account_id = if body.account_id.len() > 255 {
        body.account_id[..255].to_string()
    } else {
        body.account_id
    };

    let inbound = InboundMessage {
        channel: frankclaw_core::types::ChannelId::new("web"),
        account_id,
        sender_id,
        sender_name: body.sender_name,
        thread_id: body.thread_id,
        is_group: body.is_group,
        is_mention: body.is_mention,
        text: web_inbound_text_or_placeholder(body.message.as_deref(), &body.attachments),
        attachments: body
            .attachments
            .into_iter()
            .filter_map(|attachment| {
                let media_id = MediaId::parse(&attachment.media_id)?;
                Some(frankclaw_core::channel::InboundAttachment {
                    media_id: Some(media_id),
                    mime_type: attachment.mime_type,
                    filename: attachment.filename,
                    size_bytes: attachment.size_bytes,
                    url: Some(format!("/api/media/{}", attachment.media_id)),
                })
            })
            .collect(),
        platform_message_id: None,
        timestamp: chrono::Utc::now(),
    };
    let agent_id = body.agent_id.map(frankclaw_core::types::AgentId::new);
    let session_key = body
        .session_key
        .map(frankclaw_core::types::SessionKey::from_raw);

    match process_inbound_message_with_target(
        state.gateway.clone(),
        inbound,
        agent_id,
        session_key,
    )
    .await
    {
        Ok(Some(result)) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "accepted",
                "session_key": result.session_key.as_str(),
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "status": "accepted" })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn web_outbound_handler(
    State(state): State<AppState>,
    Query(query): Query<WebOutboundQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers, Some(&query.auth)) {
        return response;
    }

    let Some(web) = state.gateway.web_channel() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "web channel not configured" })),
        )
            .into_response();
    };

    let account_id = query
        .account_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default");
    let recipient_id = query
        .recipient_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("console-browser");
    let messages = web.drain_outbound(account_id, recipient_id).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "messages": messages })),
    )
        .into_response()
}

async fn media_upload_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers, Some(&query)) {
        return response;
    }

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/octet-stream");
    let filename = headers
        .get("x-file-name")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("upload.bin");

    match state.gateway.media.store(filename, content_type, &body).await {
        Ok(media) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "media_id": media.id.to_string(),
                "filename": media.original_name,
                "mime_type": media.mime_type,
                "size_bytes": media.size_bytes,
                "url": format!("/api/media/{}", media.id),
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn media_download_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(media_id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers, Some(&query)) {
        return response;
    }

    let Some(media_id) = MediaId::parse(&media_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid media_id" })),
        )
            .into_response();
    };

    match state.gateway.media.read(&media_id) {
        Ok(Some(media)) => {
            let mut response_headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&media.mime_type) {
                response_headers.insert(axum::http::header::CONTENT_TYPE, value);
            }
            if let Ok(value) = HeaderValue::from_str(&format!(
                "inline; filename=\"{}\"",
                media.filename
            )) {
                response_headers.insert(axum::http::header::CONTENT_DISPOSITION, value);
            }

            (StatusCode::OK, response_headers, media.bytes).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "media not found" })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

fn web_inbound_text_or_placeholder(
    message: Option<&str>,
    attachments: &[WebInboundAttachment],
) -> Option<String> {
    let text = message
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if text.is_some() {
        return text;
    }
    if attachments.is_empty() {
        None
    } else if attachments.len() > 1 {
        Some("<media:attachments>".into())
    } else {
        let mime = attachments[0].mime_type.as_str();
        if mime.starts_with("image/") {
            Some("<media:image>".into())
        } else if mime.starts_with("audio/") {
            Some("<media:audio>".into())
        } else if mime.starts_with("video/") {
            Some("<media:video>".into())
        } else {
            Some("<media:attachment>".into())
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct WhatsAppVerifyQuery {
    #[serde(rename = "hub.mode")]
    hub_mode: Option<String>,
    #[serde(rename = "hub.challenge")]
    hub_challenge: Option<String>,
    #[serde(rename = "hub.verify_token")]
    hub_verify_token: Option<String>,
}

async fn whatsapp_webhook_verify_handler(
    State(state): State<AppState>,
    Query(query): Query<WhatsAppVerifyQuery>,
) -> impl IntoResponse {
    let Some(whatsapp) = state.gateway.whatsapp_channel() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if query.hub_mode.as_deref() != Some("subscribe") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(verify_token) = query.hub_verify_token.as_deref() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !whatsapp.verify_token_matches(verify_token) {
        log_failure(
            "whatsapp.webhook.verify",
            serde_json::json!({
                "reason": "verify token mismatch",
            }),
        );
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let challenge = query.hub_challenge.unwrap_or_default();
    log_event(
        "whatsapp.webhook.verify",
        "success",
        serde_json::json!({
            "challenge_len": challenge.len(),
        }),
    );
    (StatusCode::OK, challenge).into_response()
}

async fn whatsapp_webhook_inbound_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(whatsapp) = state.gateway.whatsapp_channel() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "whatsapp channel not configured" })),
        )
            .into_response();
    };

    let config = state.gateway.current_config();
    let max_body_bytes = config.security.max_webhook_body_bytes;
    if body.len() > max_body_bytes {
        log_failure(
            "whatsapp.webhook.receive",
            serde_json::json!({
                "reason": "body too large",
                "size_bytes": body.len(),
                "max_body_bytes": max_body_bytes,
            }),
        );
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({ "error": "webhook body too large" })),
        )
            .into_response();
    }

    if let Err(err) = whatsapp.verify_signature(
        &body,
        headers
            .get("x-hub-signature-256")
            .and_then(|value| value.to_str().ok()),
    ) {
        log_failure(
            "whatsapp.webhook.receive",
            serde_json::json!({
                "reason": err.to_string(),
            }),
        );
        return (
            StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::UNAUTHORIZED),
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response();
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(err) => {
            log_failure(
                "whatsapp.webhook.receive",
                serde_json::json!({
                    "reason": format!("invalid whatsapp webhook JSON: {err}"),
                }),
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid whatsapp webhook JSON: {err}") })),
            )
                .into_response();
        }
    };

    let inbound = frankclaw_channels::whatsapp::parse_webhook_payload(&payload);
    let message_count = inbound.len();
    if inbound.is_empty() {
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "status": "ignored" })),
        )
            .into_response();
    }

    for message in inbound {
        if let Err(err) = process_inbound_message(state.gateway.clone(), message).await {
            log_failure(
                "whatsapp.webhook.receive",
                serde_json::json!({
                    "reason": err.to_string(),
                }),
            );
            return (
                StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                Json(serde_json::json!({ "error": err.to_string() })),
            )
                .into_response();
        }
    }

    log_event(
        "whatsapp.webhook.receive",
        "success",
        serde_json::json!({
            "messages": message_count,
        }),
    );

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "accepted" })),
    )
        .into_response()
}

async fn pairing_pending_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers, Some(&query)) {
        return response;
    }

    let pending = state.gateway.pairing.list_pending(None);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "pending": pending })),
    )
        .into_response()
}

#[derive(Debug, serde::Deserialize)]
struct PairingApproveRequest {
    channel: String,
    code: String,
    account: Option<String>,
}

async fn pairing_approve_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PairingApproveRequest>,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers, Some(&query)) {
        return response;
    }

    match state
        .gateway
        .pairing
        .approve(Some(&body.channel), body.account.as_deref(), &body.code)
    {
        Ok(approved) => (
            StatusCode::OK,
            Json(serde_json::json!({ "approved": approved })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn webhook_handler(
    State(state): State<AppState>,
    Path(mapping_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let config = state.gateway.current_config();
    let max_body_bytes = config
        .hooks
        .max_body_bytes
        .unwrap_or(config.security.max_webhook_body_bytes)
        .min(config.security.max_webhook_body_bytes);
    if body.len() > max_body_bytes {
        log_failure(
            "webhook.receive",
            serde_json::json!({
                "mapping_id": mapping_id,
                "reason": "body too large",
                "size_bytes": body.len(),
                "max_body_bytes": max_body_bytes,
            }),
        );
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({ "error": "webhook body too large" })),
        )
            .into_response();
    }

    if let Err(err) = crate::webhooks::verify_timestamp(
        headers
            .get("x-frankclaw-timestamp")
            .and_then(|value| value.to_str().ok()),
    ) {
        log_failure(
            "webhook.receive",
            serde_json::json!({
                "mapping_id": mapping_id,
                "reason": "webhook timestamp expired or invalid",
            }),
        );
        return (
            StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::UNAUTHORIZED),
            Json(serde_json::json!({ "error": "webhook timestamp expired or invalid" })),
        )
            .into_response();
    }

    if let Err(err) = crate::webhooks::verify_signature(
        &config,
        &body,
        headers
            .get("x-frankclaw-signature")
            .and_then(|value| value.to_str().ok()),
    ) {
        log_failure(
            "webhook.receive",
            serde_json::json!({
                "mapping_id": mapping_id,
                "reason": err.to_string(),
            }),
        );
        return (
            StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::UNAUTHORIZED),
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response();
    }
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(err) => {
            log_failure(
                "webhook.receive",
                serde_json::json!({
                    "mapping_id": mapping_id,
                    "reason": format!("invalid webhook JSON: {err}"),
                }),
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid webhook JSON: {err}") })),
            )
                .into_response()
        }
    };
    let resolved = match crate::webhooks::resolve_request(&config, &mapping_id, &payload) {
        Ok(resolved) => resolved,
        Err(err) => {
            log_failure(
                "webhook.receive",
                serde_json::json!({
                    "mapping_id": mapping_id,
                    "reason": err.to_string(),
                }),
            );
            return (
                StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::BAD_REQUEST),
                Json(serde_json::json!({ "error": err.to_string() })),
            )
                .into_response();
        }
    };

    match crate::webhooks::execute_request(&state.gateway, resolved).await {
        Ok(response) => {
            log_event(
                "webhook.receive",
                "success",
                serde_json::json!({
                    "mapping_id": mapping_id,
                    "session_key": response.session_key.as_str(),
                    "model_id": response.model_id,
                }),
            );
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "session_key": response.session_key.as_str(),
                    "model_id": response.model_id,
                    "content": response.content,
                })),
            )
                .into_response()
        }
        Err(err) => {
            log_failure(
                "webhook.receive",
                serde_json::json!({
                    "mapping_id": mapping_id,
                    "reason": err.to_string(),
                }),
            );
            (
                StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                Json(serde_json::json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

fn resolve_bind_addr(mode: &BindMode, port: u16) -> String {
    match mode {
        BindMode::Loopback => format!("127.0.0.1:{port}"),
        BindMode::Lan => format!("0.0.0.0:{port}"),
        BindMode::Address(addr) => format!("{addr}:{port}"),
    }
}

async fn shutdown_signal(token: tokio_util::sync::CancellationToken) {
    tokio::select! {
        _ = token.cancelled() => {}
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, initiating graceful shutdown");
            token.cancel();
        }
    }
}

fn require_http_auth(
    state: &AppState,
    addr: SocketAddr,
    headers: &HeaderMap,
    query: Option<&AuthQuery>,
) -> std::result::Result<(), axum::response::Response> {
    let config = state.gateway.current_config();
    let credential = extract_credential(headers, query, &config.gateway.auth);
    match authenticate(
        &config.gateway.auth,
        &credential,
        Some(&addr),
        &state.rate_limiter,
    ) {
        Ok(_) => Ok(()),
        Err(err) => {
            log_failure(
                "gateway.http_auth",
                serde_json::json!({
                    "remote_addr": addr.to_string(),
                    "status_code": err.status_code(),
                    "reason": err.to_string(),
                }),
            );
            Err((
                StatusCode::from_u16(err.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                err.to_string(),
            )
                .into_response())
        }
    }
}

fn start_channel_runtime(state: Arc<GatewayState>) {
    let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel::<InboundMessage>(256);

    for plugin in state.channels.channels().values() {
        let plugin = plugin.clone();
        let tx = inbound_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = plugin.start(tx).await {
                tracing::error!(channel = %plugin.id(), error = %err, "channel stopped with error");
            }
        });
    }

    tokio::spawn(async move {
        while let Some(inbound) = inbound_rx.recv().await {
            if let Err(err) = process_inbound_message(state.clone(), inbound).await {
                tracing::warn!(error = %err, "inbound message processing failed");
            }
        }
    });
}

fn start_session_maintenance(state: Arc<GatewayState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            SESSION_MAINTENANCE_INTERVAL_SECS,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let pruning = state.current_config().session.pruning.clone();
                    match state.sessions.maintenance(&pruning).await {
                        Ok(pruned) => {
                            if pruned > 0 {
                                log_event(
                                    "session.maintenance",
                                    "success",
                                    serde_json::json!({
                                        "pruned_sessions": pruned,
                                        "max_age_days": pruning.max_age_days,
                                        "max_sessions_per_agent": pruning.max_sessions_per_agent,
                                    }),
                                );
                            }
                        }
                        Err(err) => {
                            log_failure(
                                "session.maintenance",
                                serde_json::json!({
                                    "reason": err.to_string(),
                                }),
                            );
                        }
                    }
                }
                _ = state.shutdown.cancelled() => break,
            }
        }
    });
}

async fn process_inbound_message(
    state: Arc<GatewayState>,
    inbound: InboundMessage,
) -> frankclaw_core::error::Result<()> {
    let result = process_inbound_message_with_target(state.clone(), inbound.clone(), None, None)
        .await
        .map(|_| ());

    if let Err(ref err) = result {
        // Attempt to send a brief error reply to the user so they know something went wrong.
        // This is best-effort — if the channel itself is broken (e.g. expired token),
        // we just log the failure and move on.
        send_error_reply(&state, &inbound, err).await;
    }

    result
}

/// Best-effort: send a short error message to the user when inbound processing fails.
/// Never propagates errors — if delivery fails, it just logs and returns.
async fn send_error_reply(
    state: &Arc<GatewayState>,
    inbound: &InboundMessage,
    err: &frankclaw_core::error::FrankClawError,
) {
    let Some(channel) = state.channel(&inbound.channel) else {
        return;
    };

    let error_text = match err {
        frankclaw_core::error::FrankClawError::RequestTooLarge { .. } => {
            "Your message is too long. Please shorten it and try again.".to_string()
        }
        frankclaw_core::error::FrankClawError::RateLimited { retry_after_secs } => {
            format!("Too many requests. Please wait {retry_after_secs} seconds and try again.")
        }
        _ => "Sorry, I encountered an error processing your message. Please try again later."
            .to_string(),
    };

    let outbound = OutboundMessage {
        channel: inbound.channel.clone(),
        account_id: inbound.account_id.clone(),
        to: inbound.sender_id.clone(),
        thread_id: inbound.thread_id.clone(),
        text: error_text,
        attachments: Vec::new(),
        reply_to: inbound.platform_message_id.clone(),
    };

    if let Err(send_err) = channel.send(outbound).await {
        tracing::warn!(
            channel = %inbound.channel,
            error = %send_err,
            original_error = %err,
            "failed to send error reply to user"
        );
    }
}

#[derive(Debug, Clone)]
struct InboundProcessResult {
    session_key: frankclaw_core::types::SessionKey,
}

async fn process_inbound_message_with_target(
    state: Arc<GatewayState>,
    inbound: InboundMessage,
    agent_id: Option<frankclaw_core::types::AgentId>,
    session_key_override: Option<frankclaw_core::types::SessionKey>,
) -> frankclaw_core::error::Result<Option<InboundProcessResult>> {
    let config = state.current_config();
    let policy = config
        .channels
        .get(&inbound.channel)
        .map(|channel| channel.security_policy())
        .transpose()?
        .unwrap_or_else(|| frankclaw_core::config::ChannelSecurityPolicy {
            dm_policy: ChannelDmPolicy::Disabled,
            ..Default::default()
        });
    let text = inbound
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| frankclaw_core::error::FrankClawError::InvalidRequest {
            msg: "inbound message text is required".into(),
        })?;

    let max_message_bytes = policy
        .max_message_bytes
        .unwrap_or(config.security.max_webhook_body_bytes)
        .min(config.security.max_webhook_body_bytes);
    if text.len() > max_message_bytes {
        return Err(frankclaw_core::error::FrankClawError::RequestTooLarge {
            max_bytes: max_message_bytes,
        });
    }

    if inbound.is_group && policy.require_mention_for_groups && !inbound.is_mention {
        return Ok(None);
    }

    if inbound.is_group && !group_allowed(&policy, &inbound) {
        return Ok(None);
    }

    if !inbound.is_group {
        match policy.dm_policy {
            ChannelDmPolicy::Disabled => return Ok(None),
            ChannelDmPolicy::Open => {}
            ChannelDmPolicy::Allowlist => {
                if !sender_allowed(&policy, &state, &inbound) {
                    return Ok(None);
                }
            }
            ChannelDmPolicy::Pairing => {
                if !sender_allowed(&policy, &state, &inbound) {
                    let pending = state.pairing.ensure_pending(
                        inbound.channel.as_str(),
                        &inbound.account_id,
                        &inbound.sender_id,
                    )?;
                    if let Some(channel) = state.channel(&inbound.channel) {
                        let _ = channel
                            .send(OutboundMessage {
                                channel: inbound.channel.clone(),
                                account_id: inbound.account_id.clone(),
                                to: inbound.sender_id.clone(),
                                thread_id: inbound.thread_id.clone(),
                                text: format!(
                                    "Pairing required. Approve with: frankclaw pairing approve {} {}",
                                    inbound.channel, pending.code
                                ),
                                attachments: Vec::new(),
                                reply_to: inbound.platform_message_id.clone(),
                            })
                            .await;
                    }
                    log_event(
                        "pairing.pending",
                        "created",
                        serde_json::json!({
                            "channel": inbound.channel.as_str(),
                            "account_id": inbound.account_id.clone(),
                            "sender_id": inbound.sender_id.clone(),
                        }),
                    );
                    return Ok(None);
                }
            }
        }
    }

    let session_key = session_key_override
        .clone()
        .unwrap_or_else(|| state.runtime.session_key_for_inbound(&inbound));

    // Set up streaming: if the channel supports edit-in-place, wire a stream_tx
    // so the user sees progress during multi-round tool execution.
    let channel_for_stream = state.channel(&inbound.channel);
    let supports_edit = channel_for_stream
        .as_ref()
        .map(|ch| ch.capabilities().edit)
        .unwrap_or(false);

    let (stream_tx, stream_rx) = if supports_edit {
        let (tx, rx) = tokio::sync::mpsc::channel::<frankclaw_core::model::StreamDelta>(64);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Spawn a background task that forwards stream deltas to the channel
    // by editing a "draft" message in place.
    let stream_handle = if let (Some(mut rx), Some(channel)) = (stream_rx, &channel_for_stream) {
        let channel = channel.clone();
        let outbound_base = OutboundMessage {
            channel: inbound.channel.clone(),
            account_id: inbound.account_id.clone(),
            to: inbound.sender_id.clone(),
            thread_id: inbound.thread_id.clone(),
            text: "...".into(),
            attachments: Vec::new(),
            reply_to: inbound.platform_message_id.clone(),
        };
        Some(tokio::spawn(async move {
            // Send initial "thinking" placeholder.
            let send_result = channel.send(outbound_base).await;
            let draft_id = match send_result {
                Ok(frankclaw_core::channel::SendResult::Sent { platform_message_id }) => {
                    Some(platform_message_id)
                }
                _ => None,
            };

            let mut accumulated = String::new();
            let mut last_edit = tokio::time::Instant::now();
            let edit_interval = std::time::Duration::from_millis(1500);

            while let Some(delta) = rx.recv().await {
                if let frankclaw_core::model::StreamDelta::Text(text) = delta {
                    accumulated.push_str(&text);
                    // Throttle edits to avoid rate limits.
                    if last_edit.elapsed() >= edit_interval {
                        if let Some(ref msg_id) = draft_id {
                            let target = frankclaw_core::channel::EditMessageTarget {
                                account_id: String::new(), // filled below
                                to: String::new(),
                                thread_id: None,
                                platform_message_id: msg_id.clone(),
                            };
                            let _ = channel.edit_message(&target, &accumulated).await;
                            last_edit = tokio::time::Instant::now();
                        }
                    }
                }
            }
            draft_id
        }))
    } else {
        None
    };

    let response = state
        .runtime
        .chat(frankclaw_runtime::ChatRequest {
            agent_id,
            session_key: Some(session_key.clone()),
            message: text.to_string(),
            attachments: inbound.attachments.clone(),
            model_id: None,
            max_tokens: None,
            temperature: None,
            stream_tx,
            thinking_budget: None,
            channel_id: Some(inbound.channel.clone()),
            channel_capabilities: channel_for_stream.as_ref().map(|ch| ch.capabilities()),
            canvas: Some(state.canvas.clone()),
        })
        .await?;

    // If we had a streaming draft, wait for the background task and edit with final content.
    if let Some(handle) = stream_handle {
        if let Ok(Some(draft_id)) = handle.await {
            if let Some(ref channel) = channel_for_stream {
                let target = frankclaw_core::channel::EditMessageTarget {
                    account_id: inbound.account_id.clone(),
                    to: inbound.sender_id.clone(),
                    thread_id: inbound.thread_id.clone(),
                    platform_message_id: draft_id,
                };
                let _ = channel.edit_message(&target, &response.content).await;
            }
        }
        // Skip normal delivery — message was already sent and edited in place.
    } else if let Some(channel) = channel_for_stream {
        let outbound = OutboundMessage {
            channel: inbound.channel.clone(),
            account_id: inbound.account_id.clone(),
            to: inbound.sender_id.clone(),
            thread_id: inbound.thread_id.clone(),
            text: response.content.clone(),
            attachments: Vec::new(),
            reply_to: inbound.platform_message_id.clone(),
        };
        let delivery = deliver_outbound_message(channel, outbound, Some(state.media.as_ref())).await?;
        persist_delivery_metadata(
            state.sessions.as_ref(),
            &session_key,
            &inbound,
            &response.content,
            &delivery,
        )
        .await?;
    }

    let event = frankclaw_core::protocol::Frame::Event(
        frankclaw_core::protocol::EventFrame {
            event: frankclaw_core::protocol::EventType::ChatComplete,
            payload: serde_json::json!({
                "channel": inbound.channel.as_str(),
                "account_id": inbound.account_id,
                "session_key": session_key.as_str(),
                "content": response.content,
            }),
        },
    );
    if let Ok(json) = serde_json::to_string(&event) {
        let _ = state.broadcast.send(json);
    }

    Ok(Some(InboundProcessResult { session_key }))
}

fn log_loaded_skills(state: &Arc<GatewayState>) {
    let config = state.current_config();
    for agent_id in config.agents.agents.keys() {
        let Ok(skills) = state.runtime.list_skills(Some(agent_id)) else {
            continue;
        };
        for skill in skills {
            log_event(
                "skill.enable",
                "loaded",
                serde_json::json!({
                    "agent_id": agent_id.as_str(),
                    "skill_id": skill.id,
                    "tools": skill.tools.clone(),
                    "capabilities": skill.capabilities.clone(),
                }),
            );
        }
    }
}

fn start_config_watcher(state: Arc<GatewayState>, config_path: Option<PathBuf>) {
    let Some(config_path) = config_path else {
        return;
    };
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(CONFIG_WATCH_INTERVAL_SECS));
        let mut last_stamp = config_file_stamp(&config_path);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {}
            }

            let next_stamp = config_file_stamp(&config_path);
            if next_stamp == last_stamp {
                continue;
            }
            last_stamp = next_stamp;

            match frankclaw_core::config::FrankClawConfig::load_or_default(&config_path) {
                Ok(next_config) => {
                    if let Err(err) = next_config.validate() {
                        log_failure(
                            "config.watch",
                            serde_json::json!({
                                "path": config_path.display().to_string(),
                                "reason": err.to_string(),
                            }),
                        );
                        continue;
                    }

                    let current = state.current_config();
                    let restart_required = restart_sensitive_config_changed(&current, &next_config);
                    let merged = merge_reloadable_config(&current, &next_config);
                    state.reload_config(merged);

                    let event = frankclaw_core::protocol::Frame::Event(
                        frankclaw_core::protocol::EventFrame {
                            event: frankclaw_core::protocol::EventType::ConfigChanged,
                            payload: serde_json::json!({
                                "path": config_path.display().to_string(),
                                "restart_required": restart_required,
                            }),
                        },
                    );
                    if let Ok(json) = serde_json::to_string(&event) {
                        let _ = state.broadcast.send(json);
                    }
                    log_event(
                        "config.watch",
                        if restart_required {
                            "partial_reload"
                        } else {
                            "reloaded"
                        },
                        serde_json::json!({
                            "path": config_path.display().to_string(),
                            "restart_required": restart_required,
                        }),
                    );
                }
                Err(err) => {
                    log_failure(
                        "config.watch",
                        serde_json::json!({
                            "path": config_path.display().to_string(),
                            "reason": err.to_string(),
                        }),
                    );
                }
            }
        }
    });
}

fn config_file_stamp(path: &FsPath) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    Some((metadata.len(), modified))
}

fn restart_sensitive_config_changed(
    current: &FrankClawConfig,
    next: &FrankClawConfig,
) -> bool {
    config_restart_fingerprint(current) != config_restart_fingerprint(next)
}

fn config_restart_fingerprint(config: &FrankClawConfig) -> serde_json::Value {
    serde_json::json!({
        "agents": config.agents,
        "channels": config.channels,
        "models": config.models,
        "media": config.media,
        "security": config.security,
        "session_scoping": config.session.scoping,
        "session_reset": config.session.reset,
    })
}

fn merge_reloadable_config(
    current: &FrankClawConfig,
    next: &FrankClawConfig,
) -> FrankClawConfig {
    let mut merged = current.clone();
    merged.gateway = next.gateway.clone();
    merged.hooks = next.hooks.clone();
    merged.logging = next.logging.clone();
    merged.session.pruning = next.session.pruning.clone();

    for (channel_id, current_channel) in &mut merged.channels {
        if let Some(next_channel) = next.channels.get(channel_id) {
            current_channel.extra = next_channel.extra.clone();
        }
    }

    merged
}

async fn persist_delivery_metadata(
    sessions: &SqliteSessionStore,
    session_key: &frankclaw_core::types::SessionKey,
    inbound: &InboundMessage,
    content: &str,
    delivery: &DeliveryRecord,
) -> frankclaw_core::error::Result<()> {
    let Some(mut entry) = sessions.get(session_key).await? else {
        return Ok(());
    };

    let delivery_metadata = StoredReplyMetadata {
        channel: inbound.channel.as_str().to_string(),
        account_id: inbound.account_id.clone(),
        recipient_id: inbound.sender_id.clone(),
        thread_id: inbound.thread_id.clone(),
        reply_to: inbound.platform_message_id.clone(),
        content: content.to_string(),
        platform_message_id: delivery.platform_message_id.clone(),
        status: delivery.status.to_string(),
        attempts: delivery.attempts,
        retry_after_secs: delivery.retry_after_secs,
        error: delivery.error.clone(),
        chunks: delivery
            .chunks
            .iter()
            .map(|chunk| StoredReplyChunk {
                content: chunk.text.clone(),
                platform_message_id: chunk.platform_message_id.clone(),
                status: chunk.status.to_string(),
                attempts: chunk.attempts,
                retry_after_secs: chunk.retry_after_secs,
                error: chunk.error.clone(),
            })
            .collect::<Vec<_>>(),
        recorded_at: chrono::Utc::now(),
    };
    set_last_reply_in_metadata(&mut entry.metadata, &delivery_metadata)
        .map_err(|e| frankclaw_core::error::FrankClawError::SessionStorage {
            msg: format!("failed to serialize delivery metadata: {e}"),
        })?;

    entry.thread_id = inbound.thread_id.clone();
    entry.last_message_at = Some(chrono::Utc::now());
    sessions.upsert(&entry).await?;
    Ok(())
}

async fn start_cron_runtime(
    state: Arc<GatewayState>,
    cron: Arc<CronService>,
) -> anyhow::Result<()> {
    let config = state.current_config();
    let jobs = parse_cron_jobs(&config)?;
    cron.sync_jobs(jobs).await?;
    if !config.cron.enabled {
        return Ok(());
    }

    let runner = {
        let state = state.clone();
        Arc::new(move |job: CronJob| {
            let state = state.clone();
            Box::pin(async move {
                log_event(
                    "cron.run",
                    "started",
                    serde_json::json!({
                        "job_id": job.id,
                        "agent_id": job.agent_id.as_str(),
                        "session_key": job.session_key.as_str(),
                    }),
                );

                match state
                    .runtime
                    .chat(frankclaw_runtime::ChatRequest {
                        agent_id: Some(job.agent_id.clone()),
                        session_key: Some(job.session_key.clone()),
                        message: job.prompt.clone(),
                        attachments: Vec::new(),
                        model_id: None,
                        max_tokens: None,
                        temperature: None,
                        stream_tx: None,
                        thinking_budget: None,
                        channel_id: None,
                        channel_capabilities: None,
                        canvas: None,
                    })
                    .await
                {
                    Ok(response) => {
                        let event = frankclaw_core::protocol::Frame::Event(
                            frankclaw_core::protocol::EventFrame {
                                event: frankclaw_core::protocol::EventType::CronRun,
                                payload: serde_json::json!({
                                    "job_id": job.id,
                                    "agent_id": job.agent_id.as_str(),
                                    "session_key": response.session_key.as_str(),
                                    "model_id": response.model_id,
                                }),
                            },
                        );
                        if let Ok(json) = serde_json::to_string(&event) {
                            let _ = state.broadcast.send(json);
                        }
                        log_event(
                            "cron.run",
                            "success",
                            serde_json::json!({
                                "job_id": job.id,
                                "agent_id": job.agent_id.as_str(),
                                "session_key": response.session_key.as_str(),
                                "model_id": response.model_id,
                            }),
                        );
                        Ok(())
                    }
                    Err(err) => {
                        log_failure(
                            "cron.run",
                            serde_json::json!({
                                "job_id": job.id,
                                "agent_id": job.agent_id.as_str(),
                                "session_key": job.session_key.as_str(),
                                "reason": err.to_string(),
                            }),
                        );
                        Err(err)
                    }
                }
            }) as Pin<Box<dyn Future<Output = frankclaw_core::error::Result<()>> + Send>>
        })
    };
    cron.start(runner).await;

    tokio::spawn(async move {
        state.shutdown.cancelled().await;
        cron.stop();
    });

    Ok(())
}

fn parse_cron_jobs(config: &FrankClawConfig) -> frankclaw_core::error::Result<Vec<CronJob>> {
    config
        .cron
        .jobs
        .iter()
        .cloned()
        .map(|value| {
            let parsed = serde_json::from_value::<CronJob>(value).map_err(|err| {
                frankclaw_core::error::FrankClawError::ConfigValidation {
                    msg: format!("invalid cron job configuration: {err}"),
                }
            })?;
            validate_cron_job(&parsed)?;
            Ok(parsed)
        })
        .collect()
}

fn validate_cron_job(job: &CronJob) -> frankclaw_core::error::Result<()> {
    if job.id.trim().is_empty() {
        return Err(frankclaw_core::error::FrankClawError::ConfigValidation {
            msg: "cron job id cannot be empty".into(),
        });
    }
    if job.prompt.trim().is_empty() {
        return Err(frankclaw_core::error::FrankClawError::ConfigValidation {
            msg: format!("cron job '{}' prompt cannot be empty", job.id),
        });
    }
    let Some((session_agent_id, _, _)) = job.session_key.parse() else {
        return Err(frankclaw_core::error::FrankClawError::ConfigValidation {
            msg: format!("cron job '{}' has an invalid session key", job.id),
        });
    };
    if session_agent_id.as_str() != job.agent_id.as_str() {
        return Err(frankclaw_core::error::FrankClawError::ConfigValidation {
            msg: format!(
                "cron job '{}' session key agent '{}' does not match '{}'",
                job.id,
                session_agent_id,
                job.agent_id
            ),
        });
    }
    Ok(())
}

fn sender_allowed(
    policy: &frankclaw_core::config::ChannelSecurityPolicy,
    state: &GatewayState,
    inbound: &InboundMessage,
) -> bool {
    let explicit = policy
        .allow_from
        .iter()
        .any(|entry| entry == "*" || entry == &inbound.sender_id);

    explicit
        || state
            .pairing
            .is_approved(inbound.channel.as_str(), &inbound.account_id, &inbound.sender_id)
}

fn group_allowed(
    policy: &frankclaw_core::config::ChannelSecurityPolicy,
    inbound: &InboundMessage,
) -> bool {
    let Some(allowed_groups) = policy.allowed_groups.as_ref() else {
        return true;
    };

    if allowed_groups.iter().any(|entry| entry == "*") {
        return true;
    }

    inbound
        .thread_id
        .as_deref()
        .map(|thread_id| allowed_groups.iter().any(|entry| entry == thread_id))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use super::*;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{HeaderMap, HeaderValue};
    use axum::http::Request;
    use frankclaw_channels::{ChannelSet, whatsapp::WhatsAppChannel};
    use frankclaw_core::channel::{ChannelPlugin, SendResult};
    use frankclaw_core::config::{ChannelConfig, ProviderConfig};
    use frankclaw_core::error::FrankClawError;
    use frankclaw_core::model::{
        CompletionRequest, CompletionResponse, FinishReason, InputModality, ModelApi,
        ModelCompat, ModelCost, ModelDef, ModelProvider,
    };
    use frankclaw_core::session::SessionStore;
    use frankclaw_core::types::Role;
    use frankclaw_sessions::SqliteSessionStore;
    use frankclaw_media::MediaStore;
    use secrecy::{ExposeSecret, SecretString};
    use tower::ServiceExt;

    #[test]
    fn merge_reloadable_config_updates_gateway_and_preserves_models() {
        let mut current = FrankClawConfig::default();
        current.gateway.port = 18789;
        current.models.providers = vec![ProviderConfig {
            id: "openai".into(),
            api: "openai".into(),
            base_url: Some("https://api.openai.com/v1".into()),
            api_key_ref: Some("OPENAI_API_KEY".into()),
            models: vec!["gpt-4o-mini".into()],
            cooldown_secs: 30,
        }];
        current.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({ "dm_policy": "pairing" }),
            },
        );

        let mut next = current.clone();
        next.gateway.port = 19999;
        next.models.providers[0].models = vec!["gpt-4.1".into()];
        next.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({ "dm_policy": "open" }),
            },
        );

        let merged = merge_reloadable_config(&current, &next);

        assert_eq!(merged.gateway.port, 19999);
        assert_eq!(merged.models.providers[0].models, vec!["gpt-4o-mini"]);
        assert_eq!(
            merged.channels[&frankclaw_core::types::ChannelId::new("web")].extra["dm_policy"],
            serde_json::json!("open")
        );
    }

    #[test]
    fn restart_sensitive_config_changed_detects_provider_changes() {
        let mut current = FrankClawConfig::default();
        current.models.providers = vec![ProviderConfig {
            id: "openai".into(),
            api: "openai".into(),
            base_url: Some("https://api.openai.com/v1".into()),
            api_key_ref: Some("OPENAI_API_KEY".into()),
            models: vec!["gpt-4o-mini".into()],
            cooldown_secs: 30,
        }];

        let mut next = current.clone();
        next.models.providers[0].models = vec!["gpt-4.1".into()];

        assert!(restart_sensitive_config_changed(&current, &next));
    }

    #[test]
    fn group_allowlist_matches_explicit_thread_or_wildcard() {
        let explicit_policy = frankclaw_core::config::ChannelSecurityPolicy {
            allowed_groups: Some(vec!["group:family".into()]),
            ..Default::default()
        };
        let wildcard_policy = frankclaw_core::config::ChannelSecurityPolicy {
            allowed_groups: Some(vec!["*".into()]),
            ..Default::default()
        };
        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("signal"),
            account_id: "default".into(),
            sender_id: "+15550001111".into(),
            sender_name: None,
            thread_id: Some("group:family".into()),
            is_group: true,
            is_mention: true,
            text: Some("hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("msg-1".into()),
            timestamp: chrono::Utc::now(),
        };

        assert!(group_allowed(&explicit_policy, &inbound));
        assert!(group_allowed(&wildcard_policy, &inbound));
        assert!(!group_allowed(
            &frankclaw_core::config::ChannelSecurityPolicy {
                allowed_groups: Some(vec!["group:other".into()]),
                ..Default::default()
            },
            &inbound,
        ));
    }

    struct MockProvider;
    struct CaptureChannel {
        id: frankclaw_core::types::ChannelId,
        label: &'static str,
        sent: tokio::sync::Mutex<Vec<OutboundMessage>>,
    }

    impl CaptureChannel {
        fn new(id: &'static str, label: &'static str) -> Self {
            Self {
                id: frankclaw_core::types::ChannelId::new(id),
                label,
                sent: tokio::sync::Mutex::new(Vec::new()),
            }
        }

        async fn drain(&self) -> Vec<OutboundMessage> {
            let mut sent = self.sent.lock().await;
            std::mem::take(&mut *sent)
        }
    }

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
                usage: frankclaw_core::model::Usage {
                    input_tokens: 3,
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

    #[async_trait]
    impl frankclaw_core::channel::ChannelPlugin for CaptureChannel {
        fn id(&self) -> frankclaw_core::types::ChannelId {
            self.id.clone()
        }

        fn capabilities(&self) -> frankclaw_core::channel::ChannelCapabilities {
            frankclaw_core::channel::ChannelCapabilities {
                threads: true,
                groups: true,
                ..Default::default()
            }
        }

        fn label(&self) -> &str {
            self.label
        }

        async fn start(
            &self,
            _inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        ) -> frankclaw_core::error::Result<()> {
            Ok(())
        }

        async fn stop(&self) -> frankclaw_core::error::Result<()> {
            Ok(())
        }

        async fn health(&self) -> frankclaw_core::channel::HealthStatus {
            frankclaw_core::channel::HealthStatus::Connected
        }

        async fn send(
            &self,
            msg: OutboundMessage,
        ) -> frankclaw_core::error::Result<SendResult> {
            self.sent.lock().await.push(msg);
            Ok(SendResult::Sent {
                platform_message_id: "captured-message".into(),
            })
        }
    }

    async fn build_test_state(
        temp_dir: &PathBuf,
        mut config: FrankClawConfig,
        channels: Arc<ChannelSet>,
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
        config.models.providers = vec![ProviderConfig {
            id: "mock".into(),
            api: "ollama".into(),
            base_url: None,
            api_key_ref: None,
            models: vec!["mock-model".into()],
            cooldown_secs: 1,
        }];

        let runtime = Arc::new(
            Runtime::from_providers(
                &config,
                sessions.clone() as Arc<dyn SessionStore>,
                vec![Arc::new(MockProvider)],
            )
            .await
            .expect("runtime should build"),
        );
        (
            GatewayState::new(config, sessions.clone(), runtime, channels, pairing, media),
            sessions,
        )
    }

    #[test]
    fn extracts_password_header_for_password_mode() {
        let mut headers = HeaderMap::new();
        headers.insert("x-frankclaw-password", HeaderValue::from_static("secret"));

        match extract_credential(
            &headers,
            None,
            &frankclaw_core::auth::AuthMode::Password {
                hash: "hash".into(),
            },
        ) {
            AuthCredential::Password(password) => {
                assert_eq!(password.expose_secret(), "secret");
            }
            _ => panic!("expected password credential"),
        }
    }

    #[test]
    fn extracts_trusted_proxy_identity() {
        let mut headers = HeaderMap::new();
        headers.insert("x-auth-user", HeaderValue::from_static("alice@example.com"));

        match extract_credential(
            &headers,
            None,
            &frankclaw_core::auth::AuthMode::TrustedProxy {
                identity_header: "x-auth-user".into(),
            },
        ) {
            AuthCredential::ProxyIdentity(identity) => {
                assert_eq!(identity, "alice@example.com");
            }
            _ => panic!("expected proxy identity"),
        }
    }

    #[test]
    fn extracts_token_from_query_for_browser_ws_auth() {
        let headers = HeaderMap::new();
        let query = AuthQuery {
            token: Some("browser-token".into()),
            password: None,
            identity: None,
        };

        match extract_credential(
            &headers,
            Some(&query),
            &frankclaw_core::auth::AuthMode::Token {
                token: Some(secrecy::SecretString::from("expected".to_string())),
            },
        ) {
            AuthCredential::BearerToken(token) => {
                assert_eq!(token.expose_secret(), "browser-token");
            }
            _ => panic!("expected token credential"),
        }
    }

    #[tokio::test]
    async fn web_inbound_roundtrip_persists_reply_and_metadata() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );
        let channels = Arc::new(
            frankclaw_channels::load_from_config(&config).expect("channels should load"),
        );
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("web"),
            account_id: "default".into(),
            sender_id: "user-1".into(),
            sender_name: Some("User".into()),
            thread_id: None,
            is_group: false,
            is_mention: false,
            text: Some("hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("incoming-1".into()),
            timestamp: chrono::Utc::now(),
        };
        let session_key = state.runtime.session_key_for_inbound(&inbound);

        process_inbound_message(state.clone(), inbound)
            .await
            .expect("inbound processing should succeed");

        let outbound = state
            .web_channel()
            .expect("web channel should exist")
            .drain_outbound("default", "user-1")
            .await;
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].text, "mock reply");

        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, Role::User);
        assert_eq!(transcript[1].role, Role::Assistant);
        assert_eq!(transcript[1].content, "mock reply");

        let session = sessions
            .get(&session_key)
            .await
            .expect("session lookup should work")
            .expect("session should exist");
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["status"],
            serde_json::json!("sent")
        );
        assert!(
            session.metadata["delivery"]["last_reply"]["platform_message_id"]
                .as_str()
                .is_some()
        );
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["content"],
            serde_json::json!("mock reply")
        );

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn media_upload_and_download_roundtrip_requires_auth_and_uses_store() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-media-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::Token {
            token: Some(SecretString::from("super-secret".to_string())),
        };
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );
        let channels = Arc::new(
            frankclaw_channels::load_from_config(&config).expect("channels should load"),
        );
        let (state, _sessions) = build_test_state(&temp_dir, config, channels).await;
        let app = build_router(state.clone(), Arc::new(AuthRateLimiter::new(
            state.current_config().gateway.rate_limit.clone(),
        )));

        let mut upload_request = Request::post("/api/media/upload?token=super-secret")
            .header("content-type", "text/plain")
            .header("x-file-name", "hello.txt")
            .body(Body::from("hello media"))
            .expect("request should build");
        upload_request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));

        let upload = app
            .clone()
            .oneshot(upload_request)
            .await
            .expect("upload request should succeed");
        let upload_status = upload.status();
        let upload_body = to_bytes(upload.into_body(), usize::MAX)
            .await
            .expect("upload body should read");
        assert_eq!(
            upload_status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&upload_body)
        );
        let upload_json: serde_json::Value =
            serde_json::from_slice(&upload_body).expect("upload response should be JSON");
        let media_id = upload_json["media_id"]
            .as_str()
            .expect("media id should be present");

        let mut download_request = Request::get(format!("/api/media/{media_id}?token=super-secret"))
            .body(Body::empty())
            .expect("request should build");
        download_request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));

        let download = app
            .oneshot(download_request)
            .await
            .expect("download request should succeed");
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(
            download.headers().get("content-type").and_then(|value| value.to_str().ok()),
            Some("text/plain")
        );
        let download_body = to_bytes(download.into_body(), usize::MAX)
            .await
            .expect("download body should read");
        assert_eq!(download_body, Bytes::from_static(b"hello media"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn web_inbound_accepts_attachment_only_messages() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-web-attachment-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );
        let channels = Arc::new(
            frankclaw_channels::load_from_config(&config).expect("channels should load"),
        );
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("web"),
            account_id: "default".into(),
            sender_id: "user-1".into(),
            sender_name: Some("User".into()),
            thread_id: None,
            is_group: false,
            is_mention: false,
            text: Some("<media:image>".into()),
            attachments: vec![frankclaw_core::channel::InboundAttachment {
                media_id: MediaId::parse(&uuid::Uuid::new_v4().to_string()),
                mime_type: "image/png".into(),
                filename: Some("photo.png".into()),
                size_bytes: Some(7),
                url: Some("/api/media/example".into()),
            }],
            platform_message_id: None,
            timestamp: chrono::Utc::now(),
        };
        let session_key = state.runtime.session_key_for_inbound(&inbound);

        process_inbound_message(state.clone(), inbound)
            .await
            .expect("attachment-only inbound should succeed");

        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript[0].content, "<media:image>");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn web_inbound_http_honors_explicit_session_and_returns_it() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-web-inbound-http-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::Token {
            token: Some(SecretString::from("super-secret".to_string())),
        };
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );
        let channels = Arc::new(
            frankclaw_channels::load_from_config(&config).expect("channels should load"),
        );
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;
        let app = build_router(state.clone(), Arc::new(AuthRateLimiter::new(
            state.current_config().gateway.rate_limit.clone(),
        )));
        let session_key = frankclaw_core::types::SessionKey::from_raw("default:web:console-demo");

        let mut request = Request::post("/api/web/inbound?token=super-secret")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "sender_id": "console-browser",
                    "sender_name": "Console",
                    "message": "here is a screenshot",
                    "agent_id": "default",
                    "session_key": session_key.as_str(),
                    "attachments": [{
                        "media_id": uuid::Uuid::new_v4().to_string(),
                        "mime_type": "image/png",
                        "filename": "photo.png",
                        "size_bytes": 7
                    }]
                })
                .to_string(),
            ))
            .expect("request should build");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));

        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("response should be json");
        assert_eq!(json["session_key"], serde_json::json!(session_key.as_str()));

        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].content, "here is a screenshot");
        assert_eq!(
            transcript[0]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata["attachments"].as_array())
                .map(|attachments| attachments.len()),
            Some(1)
        );
        assert_eq!(
            transcript[0]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata["attachments"][0]["filename"].as_str()),
            Some("photo.png")
        );

        let mut outbound_request =
            Request::get("/api/web/outbound?token=super-secret&recipient_id=console-browser")
            .body(Body::empty())
            .expect("request should build");
        outbound_request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let outbound_response = app
            .oneshot(outbound_request)
            .await
            .expect("outbound request should succeed");
        assert_eq!(outbound_response.status(), StatusCode::OK);
        let outbound_body = to_bytes(outbound_response.into_body(), usize::MAX)
            .await
            .expect("outbound body should read");
        let outbound_json: serde_json::Value =
            serde_json::from_slice(&outbound_body).expect("outbound response should be json");
        assert_eq!(outbound_json["messages"][0]["text"], serde_json::json!("mock reply"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn web_outbound_route_only_drains_messages_for_requested_recipient() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-web-outbound-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::Token {
            token: Some(SecretString::from("super-secret".to_string())),
        };
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );
        let channels = Arc::new(
            frankclaw_channels::load_from_config(&config).expect("channels should load"),
        );
        let (state, _sessions) = build_test_state(&temp_dir, config, channels).await;
        let web = state.web_channel().expect("web channel should exist");
        web.send(OutboundMessage {
            channel: frankclaw_core::types::ChannelId::new("web"),
            account_id: "default".into(),
            to: "browser-a".into(),
            thread_id: None,
            text: "reply for a".into(),
            attachments: Vec::new(),
            reply_to: None,
        })
        .await
        .expect("send should succeed");
        web.send(OutboundMessage {
            channel: frankclaw_core::types::ChannelId::new("web"),
            account_id: "default".into(),
            to: "browser-b".into(),
            thread_id: None,
            text: "reply for b".into(),
            attachments: vec![frankclaw_core::channel::OutboundAttachment {
                media_id: MediaId::new(),
                mime_type: "image/png".into(),
                filename: Some("photo.png".into()),
                url: Some("/api/media/test-photo".into()),
                bytes: b"png".to_vec(),
            }],
            reply_to: None,
        })
        .await
        .expect("send should succeed");

        let app = build_router(state.clone(), Arc::new(AuthRateLimiter::new(
            state.current_config().gateway.rate_limit.clone(),
        )));

        let mut request =
            Request::get("/api/web/outbound?token=super-secret&recipient_id=browser-a")
                .body(Body::empty())
                .expect("request should build");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let response = app.clone().oneshot(request).await.expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("response should be json");
        assert_eq!(json["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(json["messages"][0]["text"], serde_json::json!("reply for a"));

        let mut other_request =
            Request::get("/api/web/outbound?token=super-secret&recipient_id=browser-b")
                .body(Body::empty())
                .expect("request should build");
        other_request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let other_response = app
            .clone()
            .oneshot(other_request)
            .await
            .expect("request should succeed");
        let other_body = to_bytes(other_response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let other_json: serde_json::Value =
            serde_json::from_slice(&other_body).expect("response should be json");
        assert_eq!(other_json["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(other_json["messages"][0]["text"], serde_json::json!("reply for b"));
        assert_eq!(
            other_json["messages"][0]["attachments"][0]["filename"],
            serde_json::json!("photo.png")
        );
        assert_eq!(
            other_json["messages"][0]["attachments"][0]["url"],
            serde_json::json!("/api/media/test-photo")
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn discord_inbound_roundtrip_targets_thread_and_persists_metadata() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-discord-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("discord"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "test-token"
                })],
                extra: serde_json::json!({
                    "dm_policy": "open",
                    "require_mention_for_groups": true
                }),
            },
        );

        let capture = Arc::new(CaptureChannel::new("discord", "Discord"));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("discord"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, None));
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("discord"),
            account_id: "default".into(),
            sender_id: "user-1".into(),
            sender_name: Some("User".into()),
            thread_id: Some("channel-42".into()),
            is_group: true,
            is_mention: true,
            text: Some("<@bot> hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("discord-msg-1".into()),
            timestamp: chrono::Utc::now(),
        };
        let session_key = state.runtime.session_key_for_inbound(&inbound);

        process_inbound_message(state.clone(), inbound)
            .await
            .expect("inbound processing should succeed");

        let outbound = capture.drain().await;
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].thread_id.as_deref(), Some("channel-42"));
        assert_eq!(outbound[0].text, "mock reply");

        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, Role::User);
        assert_eq!(transcript[1].role, Role::Assistant);

        let session = sessions
            .get(&session_key)
            .await
            .expect("session lookup should work")
            .expect("session should exist");
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["thread_id"],
            serde_json::json!("channel-42")
        );
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["content"],
            serde_json::json!("mock reply")
        );

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn telegram_inbound_roundtrip_targets_topic_and_persists_metadata() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-telegram-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("telegram"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "test-token"
                })],
                extra: serde_json::json!({
                    "dm_policy": "open",
                    "require_mention_for_groups": true
                }),
            },
        );

        let capture = Arc::new(CaptureChannel::new("telegram", "Telegram"));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("telegram"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, None));
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("telegram"),
            account_id: "default".into(),
            sender_id: "user-1".into(),
            sender_name: Some("User".into()),
            thread_id: Some("-100123:topic:7".into()),
            is_group: true,
            is_mention: true,
            text: Some("@bot hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("99".into()),
            timestamp: chrono::Utc::now(),
        };
        let session_key = state.runtime.session_key_for_inbound(&inbound);

        process_inbound_message(state.clone(), inbound)
            .await
            .expect("inbound processing should succeed");

        let outbound = capture.drain().await;
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].thread_id.as_deref(), Some("-100123:topic:7"));
        assert_eq!(outbound[0].reply_to.as_deref(), Some("99"));
        assert_eq!(outbound[0].text, "mock reply");

        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, Role::User);
        assert_eq!(transcript[1].role, Role::Assistant);

        let session = sessions
            .get(&session_key)
            .await
            .expect("session lookup should work")
            .expect("session should exist");
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["thread_id"],
            serde_json::json!("-100123:topic:7")
        );
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["reply_to"],
            serde_json::json!("99")
        );

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn discord_inbound_roundtrip_ignores_unlisted_group_thread() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-discord-group-filter-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("discord"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "test-token"
                })],
                extra: serde_json::json!({
                    "dm_policy": "open",
                    "require_mention_for_groups": true,
                    "groups": ["channel-allowed"]
                }),
            },
        );

        let capture = Arc::new(CaptureChannel::new("discord", "Discord"));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("discord"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, None));
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("discord"),
            account_id: "default".into(),
            sender_id: "user-1".into(),
            sender_name: Some("User".into()),
            thread_id: Some("channel-blocked".into()),
            is_group: true,
            is_mention: true,
            text: Some("<@bot> hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("discord-msg-1".into()),
            timestamp: chrono::Utc::now(),
        };
        let session_key = state.runtime.session_key_for_inbound(&inbound);

        process_inbound_message(state.clone(), inbound)
            .await
            .expect("inbound processing should succeed");

        assert!(capture.drain().await.is_empty());
        assert!(
            sessions
                .get(&session_key)
                .await
                .expect("session lookup should work")
                .is_none()
        );

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn slack_inbound_roundtrip_targets_thread_and_persists_metadata() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-slack-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("slack"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "app_token": "xapp-test",
                    "bot_token": "xoxb-test"
                })],
                extra: serde_json::json!({
                    "dm_policy": "open",
                    "require_mention_for_groups": true
                }),
            },
        );

        let capture = Arc::new(CaptureChannel::new("slack", "Slack"));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("slack"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, None));
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("slack"),
            account_id: "default".into(),
            sender_id: "user-1".into(),
            sender_name: Some("User".into()),
            thread_id: Some("C123:thread:1710000000.000001".into()),
            is_group: true,
            is_mention: true,
            text: Some("<@bot> hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("1710000000.123456".into()),
            timestamp: chrono::Utc::now(),
        };
        let session_key = state.runtime.session_key_for_inbound(&inbound);

        process_inbound_message(state.clone(), inbound)
            .await
            .expect("inbound processing should succeed");

        let outbound = capture.drain().await;
        assert_eq!(outbound.len(), 1);
        assert_eq!(
            outbound[0].thread_id.as_deref(),
            Some("C123:thread:1710000000.000001")
        );
        assert_eq!(outbound[0].text, "mock reply");

        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, Role::User);
        assert_eq!(transcript[1].role, Role::Assistant);

        let session = sessions
            .get(&session_key)
            .await
            .expect("session lookup should work")
            .expect("session should exist");
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["thread_id"],
            serde_json::json!("C123:thread:1710000000.000001")
        );
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["reply_to"],
            serde_json::json!("1710000000.123456")
        );

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn signal_inbound_roundtrip_targets_group_and_persists_metadata() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-signal-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("signal"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "base_url": "http://127.0.0.1:8080",
                    "account": "+15551234567"
                })],
                extra: serde_json::json!({
                    "dm_policy": "open",
                    "require_mention_for_groups": true
                }),
            },
        );

        let capture = Arc::new(CaptureChannel::new("signal", "Signal"));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("signal"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, None));
        let (state, sessions) = build_test_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("signal"),
            account_id: "default".into(),
            sender_id: "+15550001111".into(),
            sender_name: Some("User".into()),
            thread_id: Some("group:group-42".into()),
            is_group: true,
            is_mention: true,
            text: Some("hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("1710000000123".into()),
            timestamp: chrono::Utc::now(),
        };
        let session_key = state.runtime.session_key_for_inbound(&inbound);

        process_inbound_message(state.clone(), inbound)
            .await
            .expect("inbound processing should succeed");

        let outbound = capture.drain().await;
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].thread_id.as_deref(), Some("group:group-42"));
        assert_eq!(outbound[0].text, "mock reply");

        let transcript = sessions
            .get_transcript(&session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, Role::User);
        assert_eq!(transcript[1].role, Role::Assistant);

        let session = sessions
            .get(&session_key)
            .await
            .expect("session lookup should work")
            .expect("session should exist");
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["thread_id"],
            serde_json::json!("group:group-42")
        );
        assert_eq!(
            session.metadata["delivery"]["last_reply"]["reply_to"],
            serde_json::json!("1710000000123")
        );

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn webhook_http_route_verifies_signature_and_executes_runtime() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-webhook-http-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.hooks.enabled = true;
        config.hooks.token = Some("secret".into());
        config.hooks.mappings = vec![serde_json::json!({
            "id": "incoming",
            "session_key": "default:web:hook-control",
        })];
        let channels = Arc::new(ChannelSet::from_parts(HashMap::new(), None, None));
        let (state, sessions) = build_test_state(&temp_dir, config.clone(), channels).await;
        let app = build_router(
            state.clone(),
            Arc::new(AuthRateLimiter::new(config.gateway.rate_limit.clone())),
        );
        let body = br#"{"message":"hello from http hook"}"#.to_vec();
        let response = app
            .oneshot(
                Request::post("/hooks/incoming")
                    .header(
                        "x-frankclaw-signature",
                        crate::webhooks::encode_signature("secret", &body),
                    )
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).expect("response should be JSON");
        assert_eq!(payload["content"], serde_json::json!("mock reply"));

        let transcript = sessions
            .get_transcript(&frankclaw_core::types::SessionKey::from_raw("default:web:hook-control"), 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].content, "hello from http hook");
        assert_eq!(transcript[1].content, "mock reply");

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn whatsapp_webhook_route_verifies_and_processes_inbound_messages() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-whatsapp-webhook-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("whatsapp"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "access_token": "test-token",
                    "phone_number_id": "12345",
                    "verify_token": "verify-me"
                })],
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );

        let capture = Arc::new(CaptureChannel::new("whatsapp", "WhatsApp"));
        let whatsapp = Arc::new(WhatsAppChannel::new(
            secrecy::SecretString::from("test-token".to_string()),
            "12345".into(),
            secrecy::SecretString::from("verify-me".to_string()),
            None,
        ));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("whatsapp"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, Some(whatsapp)));
        let (state, sessions) = build_test_state(&temp_dir, config.clone(), channels).await;
        let app = build_router(
            state.clone(),
            Arc::new(AuthRateLimiter::new(config.gateway.rate_limit.clone())),
        );

        let verify = app
            .clone()
            .oneshot(
                Request::get("/api/whatsapp/webhook?hub.mode=subscribe&hub.verify_token=verify-me&hub.challenge=abc123")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("verify request should succeed");
        assert_eq!(verify.status(), StatusCode::OK);

        let body = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "metadata": {
                            "phone_number_id": "12345"
                        },
                        "contacts": [{
                            "wa_id": "15551234567",
                            "profile": {
                                "name": "Alice"
                            }
                        }],
                        "messages": [{
                            "from": "15551234567",
                            "id": "wamid.1",
                            "timestamp": "1710000000",
                            "type": "text",
                            "text": {
                                "body": "hello from whatsapp"
                            }
                        }]
                    }
                }]
            }]
        })
        .to_string();

        let response = app
            .oneshot(
                Request::post("/api/whatsapp/webhook")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("webhook request should succeed");

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let outbound = capture.drain().await;
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].to, "15551234567");
        assert_eq!(outbound[0].text, "mock reply");

        let transcript = sessions
            .get_transcript(
                &frankclaw_core::types::SessionKey::from_raw("default:whatsapp:12345:15551234567"),
                10,
                None,
            )
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].content, "hello from whatsapp");
        assert_eq!(transcript[1].content, "mock reply");

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn whatsapp_webhook_route_rejects_unsigned_payloads_when_app_secret_is_configured() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-whatsapp-webhook-auth-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("whatsapp"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "access_token": "test-token",
                    "phone_number_id": "12345",
                    "verify_token": "verify-me",
                    "app_secret": "app-secret"
                })],
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );

        let capture = Arc::new(CaptureChannel::new("whatsapp", "WhatsApp"));
        let whatsapp = Arc::new(WhatsAppChannel::new(
            secrecy::SecretString::from("test-token".to_string()),
            "12345".into(),
            secrecy::SecretString::from("verify-me".to_string()),
            Some(secrecy::SecretString::from("app-secret".to_string())),
        ));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("whatsapp"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, Some(whatsapp)));
        let (state, sessions) = build_test_state(&temp_dir, config.clone(), channels).await;
        let app = build_router(
            state.clone(),
            Arc::new(AuthRateLimiter::new(config.gateway.rate_limit.clone())),
        );

        let body = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "metadata": {
                            "phone_number_id": "12345"
                        },
                        "messages": [{
                            "from": "15551234567",
                            "id": "wamid.1",
                            "timestamp": "1710000000",
                            "type": "text",
                            "text": {
                                "body": "hello from whatsapp"
                            }
                        }]
                    }
                }]
            }]
        })
        .to_string();

        let response = app
            .oneshot(
                Request::post("/api/whatsapp/webhook")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("webhook request should succeed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(capture.drain().await.is_empty());
        let transcript = sessions
            .get_transcript(
                &frankclaw_core::types::SessionKey::from_raw("default:whatsapp:12345:15551234567"),
                10,
                None,
            )
            .await
            .expect("transcript should load");
        assert!(transcript.is_empty());

        let _ = std::fs::remove_file(temp_dir.join("sessions.db"));
        let _ = std::fs::remove_file(temp_dir.join("pairings.json"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    struct FailingProvider;

    #[async_trait]
    impl ModelProvider for FailingProvider {
        fn id(&self) -> &str {
            "failing"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _stream_tx: Option<tokio::sync::mpsc::Sender<frankclaw_core::model::StreamDelta>>,
        ) -> frankclaw_core::error::Result<CompletionResponse> {
            Err(FrankClawError::ModelProvider {
                msg: "simulated provider failure".into(),
            })
        }

        async fn list_models(&self) -> frankclaw_core::error::Result<Vec<ModelDef>> {
            Ok(vec![ModelDef {
                id: "failing-model".into(),
                name: "failing-model".into(),
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

    async fn build_failing_state(
        temp_dir: &PathBuf,
        mut config: FrankClawConfig,
        channels: Arc<ChannelSet>,
    ) -> (Arc<GatewayState>, Arc<SqliteSessionStore>) {
        std::fs::create_dir_all(temp_dir).expect("temp dir should exist");

        let sessions = Arc::new(
            SqliteSessionStore::open(&temp_dir.join("sessions.db"), None)
                .expect("sessions should open"),
        );
        let pairing = Arc::new(
            PairingStore::open(&temp_dir.join("pairings.json")).expect("pairings should open"),
        );
        let media = Arc::new(
            MediaStore::new(temp_dir.join("media"), 1024 * 1024, 1)
                .expect("media store should open"),
        );
        config.models.providers = vec![ProviderConfig {
            id: "failing".into(),
            api: "ollama".into(),
            base_url: None,
            api_key_ref: None,
            models: vec!["failing-model".into()],
            cooldown_secs: 1,
        }];

        let runtime = Arc::new(
            Runtime::from_providers(
                &config,
                sessions.clone() as Arc<dyn SessionStore>,
                vec![Arc::new(FailingProvider)],
            )
            .await
            .expect("runtime should build"),
        );
        (
            GatewayState::new(config, sessions.clone(), runtime, channels, pairing, media),
            sessions,
        )
    }

    #[tokio::test]
    async fn process_inbound_sends_error_reply_on_model_failure() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-gateway-test-errreply-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            frankclaw_core::types::ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({
                    "dm_policy": "open"
                }),
            },
        );
        let capture = Arc::new(CaptureChannel::new("web", "Web"));
        let mut map: HashMap<
            frankclaw_core::types::ChannelId,
            Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        > = HashMap::new();
        map.insert(
            frankclaw_core::types::ChannelId::new("web"),
            capture.clone() as Arc<dyn frankclaw_core::channel::ChannelPlugin>,
        );
        let channels = Arc::new(ChannelSet::from_parts(map, None, None));
        let (state, _sessions) = build_failing_state(&temp_dir, config, channels).await;

        let inbound = InboundMessage {
            channel: frankclaw_core::types::ChannelId::new("web"),
            account_id: "default".into(),
            sender_id: "user-1".into(),
            sender_name: Some("User".into()),
            thread_id: None,
            is_group: false,
            is_mention: false,
            text: Some("hello".into()),
            attachments: Vec::new(),
            platform_message_id: Some("msg-1".into()),
            timestamp: chrono::Utc::now(),
        };

        // Processing should fail (model provider errors)
        let result = process_inbound_message(state.clone(), inbound).await;
        assert!(result.is_err());

        // But the user should have received an error reply via the channel
        let sent = capture.drain().await;
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].text.contains("error"),
            "error reply should mention an error: {}",
            sent[0].text
        );
        assert_eq!(sent[0].to, "user-1");

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
