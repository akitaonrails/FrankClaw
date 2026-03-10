use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::{info, warn};

use frankclaw_core::channel::*;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Telegram Bot API channel adapter.
///
/// Uses long polling (`getUpdates`) to receive messages.
/// Sends messages via the Bot API REST endpoints.
pub struct TelegramChannel {
    bot_token: SecretString,
    client: Client,
    /// Offset for getUpdates long polling.
    update_offset: std::sync::atomic::AtomicI64,
}

impl TelegramChannel {
    pub fn new(bot_token: SecretString) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");

        Self::with_client(bot_token, client)
    }

    pub(crate) fn with_client(bot_token: SecretString, client: Client) -> Self {
        Self {
            bot_token,
            client,
            update_offset: std::sync::atomic::AtomicI64::new(0),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            TELEGRAM_API_BASE,
            self.bot_token.expose_secret(),
            method
        )
    }

    /// Long-poll for updates from Telegram.
    async fn poll_updates(
        &self,
        inbound_tx: &tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        let offset = self
            .update_offset
            .load(std::sync::atomic::Ordering::Relaxed);

        let body = serde_json::json!({
            "offset": offset,
            "timeout": 30,
            "allowed_updates": ["message", "edited_message"],
        });

        let resp = self
            .client
            .post(self.api_url("getUpdates"))
            .json(&body)
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("poll failed: {e}"),
            })?;

        let data: serde_json::Value = resp.json().await.map_err(|e| {
            FrankClawError::Channel {
                channel: self.id(),
                msg: format!("invalid response: {e}"),
            }
        })?;

        if let Some(updates) = data["result"].as_array() {
            for update in updates {
                // Track offset to avoid reprocessing.
                if let Some(id) = update["update_id"].as_i64() {
                    self.update_offset
                        .store(id + 1, std::sync::atomic::Ordering::Relaxed);
                }

                let msg = update.get("message").or_else(|| update.get("edited_message"));
                if let Some(msg) = msg {
                    if let Some(inbound) = self.parse_message(msg) {
                        if inbound_tx.send(inbound).await.is_err() {
                            return Ok(()); // Receiver dropped, shutting down.
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn parse_message(&self, msg: &serde_json::Value) -> Option<InboundMessage> {
        let chat_id = msg["chat"]["id"].as_i64()?;
        let sender_id = msg["from"]["id"].as_i64()?.to_string();
        let sender_name = msg["from"]["first_name"].as_str().map(String::from);
        let text = msg["text"].as_str().map(String::from);
        let topic_id = msg["message_thread_id"].as_i64();
        let is_group = matches!(
            msg["chat"]["type"].as_str(),
            Some("group") | Some("supergroup")
        );
        let message_id = msg["message_id"].as_i64()?.to_string();

        let timestamp = msg["date"]
            .as_i64()
            .map(|ts| {
                chrono::DateTime::from_timestamp(ts, 0)
                    .unwrap_or_else(|| chrono::Utc::now())
            })
            .unwrap_or_else(chrono::Utc::now);

        Some(InboundMessage {
            channel: self.id(),
            account_id: "default".to_string(),
            sender_id,
            sender_name,
            thread_id: Some(encode_thread_id(chat_id, topic_id)),
            is_group,
            is_mention: text
                .as_deref()
                .map(|t| t.contains("@"))
                .unwrap_or(false),
            text,
            attachments: vec![],
            platform_message_id: Some(message_id),
            timestamp,
        })
    }
}

#[async_trait]
impl ChannelPlugin for TelegramChannel {
    fn id(&self) -> ChannelId {
        ChannelId::new("telegram")
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            threads: true,
            groups: true,
            attachments: true,
            edit: true,
            delete: true,
            reactions: true,
            streaming: true, // Via edit-in-place
            inline_buttons: true,
            ..Default::default()
        }
    }

    fn label(&self) -> &str {
        "Telegram"
    }

    async fn start(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        info!("telegram channel starting (long polling)");
        loop {
            match self.poll_updates(&inbound_tx).await {
                Ok(()) => {}
                Err(e) => {
                    warn!(error = %e, "telegram poll error, retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn stop(&self) -> Result<()> {
        info!("telegram channel stopped");
        Ok(())
    }

    async fn health(&self) -> HealthStatus {
        match self.client.get(self.api_url("getMe")).send().await {
            Ok(resp) if resp.status().is_success() => HealthStatus::Connected,
            Ok(resp) => HealthStatus::Degraded {
                reason: format!("HTTP {}", resp.status()),
            },
            Err(e) => HealthStatus::Disconnected {
                reason: e.to_string(),
            },
        }
    }

    async fn send(&self, msg: OutboundMessage) -> Result<SendResult> {
        let body = build_send_body(&msg);

        let resp = self
            .client
            .post(self.api_url("sendMessage"))
            .json(&body)
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("send failed: {e}"),
            })?;

        let data: serde_json::Value = resp.json().await.map_err(|e| {
            FrankClawError::Channel {
                channel: self.id(),
                msg: format!("invalid response: {e}"),
            }
        })?;

        if data["ok"].as_bool() == Some(true) {
            let msg_id = data["result"]["message_id"]
                .as_i64()
                .unwrap_or(0)
                .to_string();
            Ok(SendResult::Sent {
                platform_message_id: msg_id,
            })
        } else {
            let description = data["description"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();

            // Check for rate limiting.
            if data["error_code"].as_i64() == Some(429) {
                let retry_after = data["parameters"]["retry_after"].as_u64();
                Ok(SendResult::RateLimited {
                    retry_after_secs: retry_after,
                })
            } else {
                Ok(SendResult::Failed {
                    reason: description,
                })
            }
        }
    }

    async fn edit_message(&self, target: &EditMessageTarget, new_text: &str) -> Result<()> {
        let body = build_edit_body(target, new_text)?;

        let resp = self
            .client
            .post(self.api_url("editMessageText"))
            .json(&body)
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("edit failed: {e}"),
            })?;

        let data: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid response: {e}"),
        })?;

        if data["ok"].as_bool() == Some(true) {
            Ok(())
        } else {
            Err(FrankClawError::Channel {
                channel: self.id(),
                msg: data["description"]
                    .as_str()
                    .unwrap_or("unknown telegram edit error")
                    .to_string(),
            })
        }
    }
}

fn encode_thread_id(chat_id: i64, topic_id: Option<i64>) -> String {
    match topic_id {
        Some(topic_id) => format!("{chat_id}:topic:{topic_id}"),
        None => chat_id.to_string(),
    }
}

fn build_send_body(msg: &OutboundMessage) -> serde_json::Value {
    let (chat_id, topic_id) = parse_target_thread(msg.thread_id.as_deref(), &msg.to);
    let mut body = serde_json::json!({
        "chat_id": chat_id,
        "text": msg.text,
        "parse_mode": "Markdown",
    });

    if let Some(topic_id) = topic_id {
        body["message_thread_id"] = serde_json::json!(topic_id);
    }

    body
}

fn build_edit_body(target: &EditMessageTarget, new_text: &str) -> Result<serde_json::Value> {
    let (chat_id, _) = parse_target_thread(target.thread_id.as_deref(), &target.to);
    let message_id = target
        .platform_message_id
        .parse::<i64>()
        .map_err(|_| FrankClawError::Channel {
            channel: ChannelId::new("telegram"),
            msg: "telegram edit requires a numeric platform message id".into(),
        })?;

    Ok(serde_json::json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": new_text,
        "parse_mode": "Markdown",
    }))
}

fn parse_target_thread(thread_id: Option<&str>, fallback_to: &str) -> (String, Option<i64>) {
    let raw = thread_id.unwrap_or(fallback_to);
    if let Some((chat_id, topic_id)) = raw.split_once(":topic:") {
        return (
            chat_id.to_string(),
            topic_id.parse::<i64>().ok(),
        );
    }

    (raw.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_uses_topic_thread_id_when_present() {
        let channel = TelegramChannel::new(SecretString::from("token".to_string()));
        let inbound = channel
            .parse_message(&serde_json::json!({
                "message_id": 99,
                "message_thread_id": 7,
                "date": 1_700_000_000,
                "text": "@bot hello",
                "chat": {
                    "id": -100123,
                    "type": "supergroup"
                },
                "from": {
                    "id": 42,
                    "first_name": "User"
                }
            }))
            .expect("message should parse");

        assert_eq!(inbound.thread_id.as_deref(), Some("-100123:topic:7"));
        assert!(inbound.is_group);
        assert!(inbound.is_mention);
    }

    #[test]
    fn build_send_body_uses_topic_targeting_when_thread_id_encodes_topic() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("telegram"),
            account_id: "default".into(),
            to: "42".into(),
            thread_id: Some("-100123:topic:7".into()),
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["chat_id"], serde_json::json!("-100123"));
        assert_eq!(body["message_thread_id"], serde_json::json!(7));
    }

    #[test]
    fn build_send_body_falls_back_to_recipient_without_topic() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("telegram"),
            account_id: "default".into(),
            to: "42".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["chat_id"], serde_json::json!("42"));
        assert!(body.get("message_thread_id").is_none());
    }

    #[test]
    fn build_edit_body_uses_thread_target_chat_id() {
        let body = build_edit_body(
            &EditMessageTarget {
                account_id: "default".into(),
                to: "42".into(),
                thread_id: Some("-100123:topic:7".into()),
                platform_message_id: "99".into(),
            },
            "updated",
        )
        .expect("edit body should build");

        assert_eq!(body["chat_id"], serde_json::json!("-100123"));
        assert_eq!(body["message_id"], serde_json::json!(99));
        assert_eq!(body["text"], serde_json::json!("updated"));
    }
}
