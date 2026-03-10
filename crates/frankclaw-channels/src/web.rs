use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::info;

use frankclaw_core::channel::*;
use frankclaw_core::error::Result;
use frankclaw_core::types::ChannelId;

/// HTTP/WebSocket-based web chat channel.
///
/// Messages arrive via the gateway's HTTP API and are forwarded here.
/// This is the simplest channel — no external service dependency.
pub struct WebChannel {
    /// Pending outbound messages for HTTP long-poll clients.
    outbound: tokio::sync::Mutex<Vec<OutboundMessage>>,
}

impl WebChannel {
    pub fn new() -> Self {
        Self {
            outbound: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Retrieve pending outbound messages (called by HTTP handler).
    pub async fn drain_outbound(&self) -> Vec<OutboundMessage> {
        let mut pending = self.outbound.lock().await;
        std::mem::take(&mut *pending)
    }
}

#[async_trait]
impl ChannelPlugin for WebChannel {
    fn id(&self) -> ChannelId {
        ChannelId::new("web")
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            threads: false,
            groups: false,
            attachments: true,
            edit: false,
            delete: false,
            reactions: false,
            streaming: true, // Via WebSocket
            ..Default::default()
        }
    }

    fn label(&self) -> &str {
        "Web Chat"
    }

    async fn start(&self, _inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        info!("web channel ready (messages arrive via HTTP/WS)");
        // Web channel doesn't poll — messages come through the gateway.
        // Just keep the future alive until cancelled.
        std::future::pending::<()>().await;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn health(&self) -> HealthStatus {
        HealthStatus::Connected
    }

    async fn send(&self, msg: OutboundMessage) -> Result<SendResult> {
        let msg_id = uuid::Uuid::new_v4().to_string();
        self.outbound.lock().await.push(msg);
        Ok(SendResult::Sent {
            platform_message_id: msg_id,
        })
    }
}
