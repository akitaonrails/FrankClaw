use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    extract::{
        ConnectInfo, Json, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
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
use frankclaw_cron::{CronJob, CronService};
use frankclaw_runtime::Runtime;
use frankclaw_sessions::SqliteSessionStore;

use crate::auth::{authenticate, validate_bind_auth, AuthCredential};
use crate::audit::{log_event, log_failure};
use crate::delivery::{DeliveryRecord, StoredReplyChunk, StoredReplyMetadata, deliver_outbound_message, set_last_reply_in_metadata};
use crate::pairing::PairingStore;
use crate::rate_limit::AuthRateLimiter;
use crate::state::GatewayState;

const SESSION_MAINTENANCE_INTERVAL_SECS: u64 = 15 * 60;

/// Build and start the gateway server.
pub async fn run(
    config: FrankClawConfig,
    sessions: Arc<SqliteSessionStore>,
    runtime: Arc<Runtime>,
    pairing: Arc<PairingStore>,
    cron: Arc<CronService>,
) -> anyhow::Result<()> {
    // Validate that bind + auth combination is safe.
    validate_bind_auth(&config.gateway.bind, &config.gateway.auth)?;

    let rate_limiter = Arc::new(AuthRateLimiter::new(config.gateway.rate_limit.clone()));
    let bind_addr = resolve_bind_addr(&config.gateway.bind, config.gateway.port);
    let channels = Arc::new(frankclaw_channels::load_from_config(&config)?);
    let state = GatewayState::new(config, sessions, runtime, channels, pairing);
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
        // WebSocket endpoint.
        .route("/ws", get(ws_handler))
        // Health probes (no auth required).
        .route("/health", get(health_handler))
        .route("/ready", get(readiness_handler))
        // Local web channel ingress / polling.
        .route("/api/web/inbound", post(web_inbound_handler))
        .route("/api/web/outbound", get(web_outbound_handler))
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

/// WebSocket upgrade handler with auth.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let config = state.gateway.current_config();
    // Extract credential from the configured auth mode.
    let credential = extract_credential(&headers, &config.gateway.auth);

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

            ws.on_upgrade(move |socket| {
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
    mode: &frankclaw_core::auth::AuthMode,
) -> AuthCredential {
    match mode {
        frankclaw_core::auth::AuthMode::Token { .. } => {
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
    message: String,
    #[serde(default = "default_web_account_id")]
    account_id: String,
    sender_name: Option<String>,
    thread_id: Option<String>,
    #[serde(default)]
    is_group: bool,
    #[serde(default)]
    is_mention: bool,
}

fn default_web_account_id() -> String {
    "default".to_string()
}

async fn web_inbound_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<WebInboundRequest>,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers) {
        return response;
    }

    let inbound = InboundMessage {
        channel: frankclaw_core::types::ChannelId::new("web"),
        account_id: body.account_id,
        sender_id: body.sender_id,
        sender_name: body.sender_name,
        thread_id: body.thread_id,
        is_group: body.is_group,
        is_mention: body.is_mention,
        text: Some(body.message),
        attachments: Vec::new(),
        platform_message_id: None,
        timestamp: chrono::Utc::now(),
    };

    match process_inbound_message(state.gateway.clone(), inbound).await {
        Ok(()) => (
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_http_auth(&state, addr, &headers) {
        return response;
    }

    let Some(web) = state.gateway.web_channel() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "web channel not configured" })),
        )
            .into_response();
    };

    let messages = web.drain_outbound().await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "messages": messages })),
    )
        .into_response()
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
) -> std::result::Result<(), axum::response::Response> {
    let config = state.gateway.current_config();
    let credential = extract_credential(headers, &config.gateway.auth);
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
        return Ok(());
    }

    if !inbound.is_group {
        match policy.dm_policy {
            ChannelDmPolicy::Disabled => return Ok(()),
            ChannelDmPolicy::Open => {}
            ChannelDmPolicy::Allowlist => {
                if !sender_allowed(&policy, &state, &inbound) {
                    return Ok(());
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
                    return Ok(());
                }
            }
        }
    }

    let session_key = state.runtime.session_key_for_inbound(&inbound);

    let response = state
        .runtime
        .chat(frankclaw_runtime::ChatRequest {
            agent_id: None,
            session_key: Some(session_key.clone()),
            message: text.to_string(),
            model_id: None,
            max_tokens: None,
            temperature: None,
        })
        .await?;

    if let Some(channel) = state.channel(&inbound.channel) {
        let outbound = OutboundMessage {
            channel: inbound.channel.clone(),
            account_id: inbound.account_id.clone(),
            to: inbound.sender_id.clone(),
            thread_id: inbound.thread_id.clone(),
            text: response.content.clone(),
            attachments: Vec::new(),
            reply_to: inbound.platform_message_id.clone(),
        };
        let delivery = deliver_outbound_message(channel, outbound).await?;
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

    Ok(())
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
                        model_id: None,
                        max_tokens: None,
                        temperature: None,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use super::*;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::http::{HeaderMap, HeaderValue};
    use frankclaw_channels::ChannelSet;
    use frankclaw_core::channel::SendResult;
    use frankclaw_core::config::{ChannelConfig, ProviderConfig};
    use frankclaw_core::model::{
        CompletionRequest, CompletionResponse, FinishReason, InputModality, ModelApi,
        ModelCompat, ModelCost, ModelDef, ModelProvider,
    };
    use frankclaw_core::session::SessionStore;
    use frankclaw_core::types::Role;
    use frankclaw_sessions::SqliteSessionStore;
    use secrecy::ExposeSecret;

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
            GatewayState::new(config, sessions.clone(), runtime, channels, pairing),
            sessions,
        )
    }

    #[test]
    fn extracts_password_header_for_password_mode() {
        let mut headers = HeaderMap::new();
        headers.insert("x-frankclaw-password", HeaderValue::from_static("secret"));

        match extract_credential(
            &headers,
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
            .drain_outbound()
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
        let channels = Arc::new(ChannelSet::from_parts(map, None));
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
        let channels = Arc::new(ChannelSet::from_parts(map, None));
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
        let channels = Arc::new(ChannelSet::from_parts(map, None));
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
}
