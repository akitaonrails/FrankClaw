use std::sync::Arc;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

use frankclaw_channels::{web::WebChannel, whatsapp::WhatsAppChannel, ChannelSet};
use frankclaw_core::channel::ChannelPlugin;
use frankclaw_core::config::FrankClawConfig;
use frankclaw_core::types::ChannelId;
use frankclaw_core::types::ConnId;
use frankclaw_runtime::Runtime;
use frankclaw_sessions::SqliteSessionStore;

use crate::broadcast::BroadcastHandle;
use crate::canvas::CanvasStore;
use crate::pairing::PairingStore;

/// Shared gateway state, wrapped in `Arc` for cheap cloning across tasks.
///
/// Uses lock-free `ArcSwap` for config so hot-reload never blocks request handling.
/// Uses `DashMap` (sharded concurrent map) for the client registry.
pub struct GatewayState {
    /// Current configuration. Swapped atomically on hot-reload.
    pub config: ArcSwap<FrankClawConfig>,

    /// Session store (SQLite with optional encryption).
    pub sessions: Arc<SqliteSessionStore>,

    /// Connected WebSocket clients.
    pub clients: DashMap<ConnId, ClientState>,

    /// Runtime orchestrator for model-backed chat flows.
    pub runtime: Arc<Runtime>,

    /// Loaded first-party channels.
    pub channels: Arc<ChannelSet>,

    /// Local pairing approvals and pending requests.
    pub pairing: Arc<PairingStore>,

    /// Shared canvas host state for the local console.
    pub canvas: Arc<CanvasStore>,

    /// Monotonic connection counter.
    pub next_conn_id: std::sync::atomic::AtomicU64,

    /// Broadcast channel for server-push events.
    pub broadcast: BroadcastHandle,

    /// Signals graceful shutdown to all tasks.
    pub shutdown: CancellationToken,
}

/// Per-connection state.
pub struct ClientState {
    /// Sender half of the WebSocket connection.
    pub tx: tokio::sync::mpsc::Sender<String>,
    /// Authenticated role.
    pub role: frankclaw_core::auth::AuthRole,
    /// Remote address (for rate limiting).
    pub remote_addr: Option<std::net::SocketAddr>,
    /// When this client connected.
    pub connected_at: chrono::DateTime<chrono::Utc>,
}

impl GatewayState {
    pub fn new(
        config: FrankClawConfig,
        sessions: Arc<SqliteSessionStore>,
        runtime: Arc<Runtime>,
        channels: Arc<ChannelSet>,
        pairing: Arc<PairingStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config: ArcSwap::new(Arc::new(config)),
            sessions,
            clients: DashMap::new(),
            runtime,
            channels,
            pairing,
            canvas: CanvasStore::new(),
            next_conn_id: std::sync::atomic::AtomicU64::new(1),
            broadcast: BroadcastHandle::new(256),
            shutdown: CancellationToken::new(),
        })
    }

    /// Allocate a new unique connection ID.
    pub fn alloc_conn_id(&self) -> ConnId {
        ConnId(
            self.next_conn_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Reload configuration atomically. In-flight requests see old config;
    /// new requests see the new config. No lock contention.
    pub fn reload_config(&self, new_config: FrankClawConfig) {
        self.config.store(Arc::new(new_config));
        tracing::info!("configuration reloaded");
    }

    /// Get current config snapshot (cheap Arc clone).
    pub fn current_config(&self) -> Arc<FrankClawConfig> {
        self.config.load_full()
    }

    pub fn channel(&self, id: &ChannelId) -> Option<Arc<dyn ChannelPlugin>> {
        self.channels.get(id).cloned()
    }

    pub fn web_channel(&self) -> Option<Arc<WebChannel>> {
        self.channels.web()
    }

    pub fn whatsapp_channel(&self) -> Option<Arc<WhatsAppChannel>> {
        self.channels.whatsapp()
    }
}
