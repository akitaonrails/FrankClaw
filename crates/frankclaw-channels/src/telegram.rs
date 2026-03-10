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
            thread_id: Some(chat_id.to_string()),
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
        let chat_id = msg
            .thread_id
            .as_deref()
            .or(Some(&msg.to))
            .unwrap_or(&msg.to);

        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": msg.text,
            "parse_mode": "Markdown",
        });

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

    async fn edit_message(&self, _platform_message_id: &str, _new_text: &str) -> Result<()> {
        // Telegram editMessageText requires chat_id — we'd need to store it.
        // For now, return unsupported. Full implementation needs message context.
        Err(FrankClawError::Channel {
            channel: self.id(),
            msg: "edit requires chat context (not yet implemented)".into(),
        })
    }
}
