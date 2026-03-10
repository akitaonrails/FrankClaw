use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::{ChannelId, MediaId};

/// What a channel supports. Used for capability negotiation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelCapabilities {
    pub threads: bool,
    pub groups: bool,
    pub attachments: bool,
    pub edit: bool,
    pub delete: bool,
    pub reactions: bool,
    pub streaming: bool,
    pub voice: bool,
    pub inline_buttons: bool,
}

/// Health status of a channel account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Connected,
    Degraded { reason: String },
    Disconnected { reason: String },
    NotConfigured,
}

/// A message received from a channel (normalized).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    /// Which channel this came from.
    pub channel: ChannelId,
    /// Account that received it (e.g., which Telegram bot).
    pub account_id: String,
    /// Sender identity (platform-specific user ID).
    pub sender_id: String,
    /// Human-readable sender name (for display only, never trust for auth).
    pub sender_name: Option<String>,
    /// Thread/conversation ID if applicable.
    pub thread_id: Option<String>,
    /// Whether this is a group message.
    pub is_group: bool,
    /// Whether the bot was explicitly mentioned.
    pub is_mention: bool,
    /// Text content.
    pub text: Option<String>,
    /// Attached media.
    pub attachments: Vec<InboundAttachment>,
    /// Platform-specific message ID (for reply threading).
    pub platform_message_id: Option<String>,
    /// When the message was sent (platform timestamp).
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Attachment on an inbound message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundAttachment {
    pub media_id: Option<MediaId>,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: Option<u64>,
    /// URL to fetch the attachment (platform-specific, may require auth).
    pub url: Option<String>,
}

/// A message to send via a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub channel: ChannelId,
    pub account_id: String,
    pub to: String,
    pub thread_id: Option<String>,
    pub text: String,
    pub attachments: Vec<OutboundAttachment>,
    pub reply_to: Option<String>,
}

/// Attachment on an outbound message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundAttachment {
    pub media_id: MediaId,
    pub mime_type: String,
    pub filename: Option<String>,
}

/// Result of sending a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SendResult {
    Sent {
        platform_message_id: String,
    },
    RateLimited {
        retry_after_secs: Option<u64>,
    },
    Failed {
        reason: String,
    },
}

/// Context required to edit an already-sent platform message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditMessageTarget {
    pub account_id: String,
    pub to: String,
    pub thread_id: Option<String>,
    pub platform_message_id: String,
}

/// Streaming handle for incremental message delivery.
pub struct StreamHandle {
    pub channel: ChannelId,
    pub account_id: String,
    pub to: String,
    pub thread_id: Option<String>,
    /// Platform message ID of the "draft" message being edited.
    pub draft_message_id: String,
}

/// The trait every channel adapter must implement.
///
/// Channels receive inbound messages via a tokio mpsc channel (passed at start),
/// and send outbound messages via the trait methods.
#[async_trait]
pub trait ChannelPlugin: Send + Sync + 'static {
    /// Unique channel identifier (e.g., "telegram", "discord").
    fn id(&self) -> ChannelId;

    /// What this channel supports.
    fn capabilities(&self) -> ChannelCapabilities;

    /// Human-readable label.
    fn label(&self) -> &str;

    /// Start the channel adapter. Inbound messages sent to `inbound_tx`.
    async fn start(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()>;

    /// Stop the channel adapter gracefully.
    async fn stop(&self) -> Result<()>;

    /// Health check.
    async fn health(&self) -> HealthStatus;

    /// Send a message.
    async fn send(&self, msg: OutboundMessage) -> Result<SendResult>;

    /// Edit a previously sent message (if supported).
    async fn edit_message(
        &self,
        _target: &EditMessageTarget,
        _new_text: &str,
    ) -> Result<()> {
        Err(crate::error::FrankClawError::Channel {
            channel: self.id(),
            msg: "edit not supported".into(),
        })
    }

    /// Delete a previously sent message (if supported).
    async fn delete_message(&self, _platform_message_id: &str) -> Result<()> {
        Err(crate::error::FrankClawError::Channel {
            channel: self.id(),
            msg: "delete not supported".into(),
        })
    }

    /// Start streaming a response (if supported).
    async fn stream_start(
        &self,
        _msg: &OutboundMessage,
    ) -> Result<StreamHandle> {
        Err(crate::error::FrankClawError::Channel {
            channel: self.id(),
            msg: "streaming not supported".into(),
        })
    }

    /// Update an in-progress stream.
    async fn stream_update(
        &self,
        _handle: &StreamHandle,
        _text: &str,
    ) -> Result<()> {
        Err(crate::error::FrankClawError::Channel {
            channel: self.id(),
            msg: "streaming not supported".into(),
        })
    }

    /// Finalize a stream.
    async fn stream_end(
        &self,
        _handle: &StreamHandle,
        _final_text: &str,
    ) -> Result<()> {
        Err(crate::error::FrankClawError::Channel {
            channel: self.id(),
            msg: "streaming not supported".into(),
        })
    }
}
