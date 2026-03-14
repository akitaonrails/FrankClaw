use crate::types::{AgentId, ChannelId, SessionKey};

/// Unified error hierarchy. Every variant is explicit — no catch-all.
/// Error messages never contain secret material.
#[derive(Debug, thiserror::Error)]
pub enum FrankClawError {
    // ── Auth ──────────────────────────────────────────────
    #[error("authentication required")]
    AuthRequired,

    #[error("authentication failed")]
    AuthFailed,

    #[error("rate limited (retry after {retry_after_secs}s)")]
    RateLimited { retry_after_secs: u64 },

    #[error("insufficient permissions for method {method}")]
    Forbidden { method: String },

    // ── Session ──────────────────────────────────────────
    #[error("session not found: {key}")]
    SessionNotFound { key: SessionKey },

    #[error("session storage error: {msg}")]
    SessionStorage { msg: String },

    // ── Channel ──────────────────────────────────────────
    #[error("channel {channel} error: {msg}")]
    Channel { channel: ChannelId, msg: String },

    #[error("channel {channel} not configured")]
    ChannelNotConfigured { channel: ChannelId },

    #[error("channel {channel} is disabled")]
    ChannelDisabled { channel: ChannelId },

    #[error("sender blocked by policy on channel {channel}")]
    SenderBlocked { channel: ChannelId },

    // ── Agent ────────────────────────────────────────────
    #[error("agent {agent_id} not found")]
    AgentNotFound { agent_id: AgentId },

    #[error("agent runtime error: {msg}")]
    AgentRuntime { msg: String },

    #[error("agent turn cancelled")]
    TurnCancelled,

    #[error("sandbox error: {msg}")]
    Sandbox { msg: String },

    // ── Model ────────────────────────────────────────────
    #[error("model provider error: {msg}")]
    ModelProvider { msg: String },

    #[error("all model providers failed")]
    AllProvidersFailed,

    #[error("model not found: {model_id}")]
    ModelNotFound { model_id: String },

    // ── Config ───────────────────────────────────────────
    #[error("config validation error: {msg}")]
    ConfigValidation { msg: String },

    #[error("config I/O error: {msg}")]
    ConfigIo { msg: String },

    // ── Protocol ─────────────────────────────────────────
    #[error("invalid request: {msg}")]
    InvalidRequest { msg: String },

    #[error("unknown method: {method}")]
    UnknownMethod { method: String },

    #[error("request too large (max {max_bytes} bytes)")]
    RequestTooLarge { max_bytes: usize },

    // ── Media ────────────────────────────────────────────
    #[error("media file too large (max {max_bytes} bytes)")]
    MediaTooLarge { max_bytes: u64 },

    #[error("media fetch blocked: {reason}")]
    MediaFetchBlocked { reason: String },

    #[error("unsupported media type: {mime}")]
    UnsupportedMediaType { mime: String },

    #[error("malware detected in file '{filename}': {detail}")]
    MalwareDetected { filename: String, detail: String },

    // ── Crypto ───────────────────────────────────────────
    #[error("cryptographic operation failed: {0}")]
    Crypto(#[from] frankclaw_crypto::CryptoError),

    // ── Memory ──────────────────────────────────────────
    #[error("memory store error: {msg}")]
    MemoryStore { msg: String },

    #[error("embedding provider error: {msg}")]
    EmbeddingProvider { msg: String },

    // ── Internal ─────────────────────────────────────────
    #[error("internal error: {msg}")]
    Internal { msg: String },

    #[error("shutdown in progress")]
    ShuttingDown,
}

impl FrankClawError {
    /// Whether the client should retry this request.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::ModelProvider { .. }
                | Self::AllProvidersFailed
                | Self::Internal { .. }
        )
    }

    /// HTTP-like status code for protocol responses.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::AuthRequired => 401,
            Self::AuthFailed => 401,
            Self::RateLimited { .. } => 429,
            Self::Forbidden { .. } => 403,
            Self::SessionNotFound { .. } => 404,
            Self::AgentNotFound { .. } => 404,
            Self::ModelNotFound { .. } => 404,
            Self::ChannelNotConfigured { .. } => 404,
            Self::InvalidRequest { .. } => 400,
            Self::UnknownMethod { .. } => 400,
            Self::RequestTooLarge { .. } => 413,
            Self::MediaTooLarge { .. } => 413,
            Self::MediaFetchBlocked { .. } => 403,
            Self::MalwareDetected { .. } => 403,
            Self::ConfigValidation { .. } => 422,
            Self::SenderBlocked { .. } => 403,
            Self::TurnCancelled => 499,
            _ => 500,
        }
    }
}

pub type Result<T> = std::result::Result<T, FrankClawError>;
