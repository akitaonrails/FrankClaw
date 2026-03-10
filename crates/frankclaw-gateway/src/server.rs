use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;

use frankclaw_core::config::{BindMode, FrankClawConfig};
use frankclaw_sessions::SqliteSessionStore;

use crate::auth::{authenticate, validate_bind_auth, AuthCredential};
use crate::rate_limit::AuthRateLimiter;
use crate::state::GatewayState;

/// Build and start the gateway server.
pub async fn run(
    config: FrankClawConfig,
    sessions: SqliteSessionStore,
) -> anyhow::Result<()> {
    // Validate that bind + auth combination is safe.
    validate_bind_auth(&config.gateway.bind, &config.gateway.auth)?;

    let rate_limiter = Arc::new(AuthRateLimiter::new(config.gateway.rate_limit.clone()));
    let bind_addr = resolve_bind_addr(&config.gateway.bind, config.gateway.port);
    let state = GatewayState::new(config, sessions);

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
    // Extract credential from Authorization header.
    let credential = extract_credential(&headers);

    // Authenticate.
    let config = state.gateway.current_config();
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
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, e.to_string()).into_response()
        }
    }
}

/// Extract auth credential from HTTP headers.
fn extract_credential(headers: &HeaderMap) -> AuthCredential {
    if let Some(auth) = headers.get("authorization") {
        if let Ok(value) = auth.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                return AuthCredential::BearerToken(secrecy::SecretString::from(token.to_string()));
            }
        }
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
