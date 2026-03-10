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

use frankclaw_core::channel::{InboundMessage, OutboundMessage, SendResult};
use frankclaw_core::config::{BindMode, FrankClawConfig};
use frankclaw_core::session::SessionStore;
use frankclaw_cron::{CronJob, CronService};
use frankclaw_runtime::Runtime;
use frankclaw_sessions::SqliteSessionStore;

use crate::auth::{authenticate, validate_bind_auth, AuthCredential};
use crate::audit::{log_event, log_failure};
use crate::pairing::PairingStore;
use crate::rate_limit::AuthRateLimiter;
use crate::state::GatewayState;

const MAX_OUTBOUND_ATTEMPTS: usize = 3;
const MAX_RETRY_DELAY_SECS: u64 = 30;
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
    let text = inbound
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| frankclaw_core::error::FrankClawError::InvalidRequest {
            msg: "inbound message text is required".into(),
        })?;

    if text.len() > config.security.max_webhook_body_bytes {
        return Err(frankclaw_core::error::FrankClawError::RequestTooLarge {
            max_bytes: config.security.max_webhook_body_bytes,
        });
    }

    if inbound.is_group && !inbound.is_mention {
        return Ok(());
    }

    if !inbound.is_group {
        match dm_policy(&config, &inbound.channel) {
            DmPolicy::Disabled => return Ok(()),
            DmPolicy::Open => {}
            DmPolicy::Allowlist => {
                if !sender_allowed(&config, &state, &inbound) {
                    return Ok(());
                }
            }
            DmPolicy::Pairing => {
                if !sender_allowed(&config, &state, &inbound) {
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

#[derive(Clone)]
struct DeliveryRecord {
    status: &'static str,
    platform_message_id: Option<String>,
    attempts: usize,
    retry_after_secs: Option<u64>,
    error: Option<String>,
}

async fn deliver_outbound_message(
    channel: Arc<dyn frankclaw_core::channel::ChannelPlugin>,
    outbound: OutboundMessage,
) -> frankclaw_core::error::Result<DeliveryRecord> {
    let mut attempts = 0usize;
    let mut last_retry_after = None;

    loop {
        attempts += 1;
        match channel.send(outbound.clone()).await {
            Ok(SendResult::Sent { platform_message_id }) => {
                log_event(
                    "channel.send",
                    "success",
                    serde_json::json!({
                        "channel": outbound.channel.as_str(),
                        "account_id": outbound.account_id,
                        "recipient": outbound.to,
                        "attempts": attempts,
                        "platform_message_id": platform_message_id,
                    }),
                );
                return Ok(DeliveryRecord {
                    status: "sent",
                    platform_message_id: Some(platform_message_id),
                    attempts,
                    retry_after_secs: last_retry_after,
                    error: None,
                });
            }
            Ok(SendResult::RateLimited { retry_after_secs }) => {
                last_retry_after = retry_after_secs;
                if attempts >= MAX_OUTBOUND_ATTEMPTS {
                    log_failure(
                        "channel.send",
                        serde_json::json!({
                            "channel": outbound.channel.as_str(),
                            "account_id": outbound.account_id,
                            "recipient": outbound.to,
                            "attempts": attempts,
                            "reason": "rate_limited",
                            "retry_after_secs": retry_after_secs,
                        }),
                    );
                    return Ok(DeliveryRecord {
                        status: "rate_limited",
                        platform_message_id: None,
                        attempts,
                        retry_after_secs,
                        error: Some("rate limited".to_string()),
                    });
                }

                let delay_secs = retry_after_secs
                    .unwrap_or(attempts as u64)
                    .clamp(1, MAX_RETRY_DELAY_SECS);
                tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
            }
            Ok(SendResult::Failed { reason }) => {
                if attempts >= MAX_OUTBOUND_ATTEMPTS {
                    log_failure(
                        "channel.send",
                        serde_json::json!({
                            "channel": outbound.channel.as_str(),
                            "account_id": outbound.account_id,
                            "recipient": outbound.to,
                            "attempts": attempts,
                            "reason": reason,
                        }),
                    );
                    return Ok(DeliveryRecord {
                        status: "failed",
                        platform_message_id: None,
                        attempts,
                        retry_after_secs: None,
                        error: Some(reason),
                    });
                }

                tokio::time::sleep(std::time::Duration::from_secs(attempts as u64)).await;
            }
            Err(err) => {
                if attempts >= MAX_OUTBOUND_ATTEMPTS {
                    let error_text = err.to_string();
                    log_failure(
                        "channel.send",
                        serde_json::json!({
                            "channel": outbound.channel.as_str(),
                            "account_id": outbound.account_id,
                            "recipient": outbound.to,
                            "attempts": attempts,
                            "reason": error_text,
                        }),
                    );
                    return Err(err);
                }

                tokio::time::sleep(std::time::Duration::from_secs(attempts as u64)).await;
            }
        }
    }
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

    let delivery_metadata = serde_json::json!({
        "last_reply": {
            "channel": inbound.channel.as_str(),
            "account_id": inbound.account_id.clone(),
            "recipient_id": inbound.sender_id.clone(),
            "thread_id": inbound.thread_id.clone(),
            "reply_to": inbound.platform_message_id.clone(),
            "content": content,
            "platform_message_id": delivery.platform_message_id.clone(),
            "status": delivery.status,
            "attempts": delivery.attempts,
            "retry_after_secs": delivery.retry_after_secs,
            "error": delivery.error.clone(),
            "recorded_at": chrono::Utc::now(),
        }
    });

    match &mut entry.metadata {
        serde_json::Value::Object(object) => {
            object.insert("delivery".to_string(), delivery_metadata);
        }
        _ => {
            entry.metadata = serde_json::json!({
                "delivery": delivery_metadata,
            });
        }
    }

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

#[derive(Clone, Copy)]
enum DmPolicy {
    Open,
    Allowlist,
    Pairing,
    Disabled,
}

fn dm_policy(config: &FrankClawConfig, channel_id: &frankclaw_core::types::ChannelId) -> DmPolicy {
    let Some(channel) = config.channels.get(channel_id) else {
        return DmPolicy::Disabled;
    };
    let raw = channel
        .extra
        .get("dm_policy")
        .and_then(|value| value.as_str())
        .unwrap_or("pairing");

    match raw {
        "open" => DmPolicy::Open,
        "allowlist" => DmPolicy::Allowlist,
        "disabled" => DmPolicy::Disabled,
        _ => DmPolicy::Pairing,
    }
}

fn sender_allowed(
    config: &FrankClawConfig,
    state: &GatewayState,
    inbound: &InboundMessage,
) -> bool {
    let explicit = config
        .channels
        .get(&inbound.channel)
        .and_then(|channel| channel.extra.get("allow_from"))
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries.iter().any(|entry| {
                entry
                    .as_str()
                    .map(|entry| entry == "*" || entry == inbound.sender_id)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    explicit
        || state
            .pairing
            .is_approved(inbound.channel.as_str(), &inbound.account_id, &inbound.sender_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use secrecy::ExposeSecret;

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
}
