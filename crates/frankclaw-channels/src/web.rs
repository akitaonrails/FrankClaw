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
    /// Pending outbound messages keyed per web recipient.
    outbound: tokio::sync::Mutex<std::collections::HashMap<String, Vec<OutboundMessage>>>,
}

impl Default for WebChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl WebChannel {
    pub fn new() -> Self {
        Self {
            outbound: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Retrieve pending outbound messages for one web recipient.
    pub async fn drain_outbound(
        &self,
        account_id: &str,
        recipient_id: &str,
    ) -> Vec<OutboundMessage> {
        let mut pending = self.outbound.lock().await;
        pending
            .remove(&outbound_queue_key(account_id, recipient_id))
            .unwrap_or_default()
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
        let queue_key = outbound_queue_key(&msg.account_id, &msg.to);
        self.outbound
            .lock()
            .await
            .entry(queue_key)
            .or_default()
            .push(msg);
        Ok(SendResult::Sent {
            platform_message_id: msg_id,
        })
    }
}

fn outbound_queue_key(account_id: &str, recipient_id: &str) -> String {
    format!("{}:{}", account_id.trim(), recipient_id.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::types::MediaId;

    #[tokio::test]
    async fn drain_outbound_only_returns_messages_for_requested_recipient() {
        let channel = WebChannel::new();
        channel
            .send(OutboundMessage {
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                to: "browser-a".into(),
                thread_id: None,
                text: "hello a".into(),
                attachments: Vec::new(),
                reply_to: None,
            })
            .await
            .expect("send should succeed");
        channel
            .send(OutboundMessage {
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                to: "browser-b".into(),
                thread_id: None,
                text: "hello b".into(),
                attachments: vec![OutboundAttachment {
                    media_id: MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    url: Some("/api/media/test".into()),
                    bytes: b"png".to_vec(),
                }],
                reply_to: None,
            })
            .await
            .expect("send should succeed");

        let a = channel.drain_outbound("default", "browser-a").await;
        let b = channel.drain_outbound("default", "browser-b").await;

        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, "hello a");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].text, "hello b");
        assert_eq!(b[0].attachments.len(), 1);
        assert!(channel.drain_outbound("default", "browser-a").await.is_empty());
    }
}
