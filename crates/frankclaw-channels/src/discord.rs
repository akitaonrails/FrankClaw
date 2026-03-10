use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use frankclaw_core::channel::*;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DISCORD_GATEWAY_VERSION: &str = "10";
const DISCORD_INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 12) | (1 << 15);

pub struct DiscordChannel {
    bot_token: SecretString,
    client: Client,
    bot_user_id: Mutex<Option<String>>,
}

impl DiscordChannel {
    pub fn new(bot_token: SecretString) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");

        Self {
            bot_token,
            client,
            bot_user_id: Mutex::new(None),
        }
    }

    fn auth_header(&self) -> String {
        format!("Bot {}", self.bot_token.expose_secret())
    }

    async fn gateway_url(&self) -> Result<String> {
        let resp = self
            .client
            .get(format!("{DISCORD_API_BASE}/gateway/bot"))
            .header("authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("gateway discovery failed: {e}"),
            })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid gateway discovery response: {e}"),
        })?;

        let url = body["url"]
            .as_str()
            .ok_or_else(|| FrankClawError::Channel {
                channel: self.id(),
                msg: "discord gateway discovery did not return a url".into(),
            })?;

        Ok(format!("{url}/?v={DISCORD_GATEWAY_VERSION}&encoding=json"))
    }

    async fn run_gateway(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        let gateway_url = self.gateway_url().await?;
        let (socket, _) = tokio_tungstenite::connect_async(gateway_url)
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("gateway connect failed: {e}"),
            })?;
        let (mut ws_tx, mut ws_rx) = socket.split();

        let hello = next_json_frame(self.id(), &mut ws_rx).await?;
        let heartbeat_interval_ms = hello["d"]["heartbeat_interval"]
            .as_u64()
            .ok_or_else(|| FrankClawError::Channel {
                channel: self.id(),
                msg: "discord hello payload missing heartbeat interval".into(),
            })?;

        ws_tx
            .send(Message::Text(
                serde_json::json!({
                    "op": 2,
                    "d": {
                        "token": self.bot_token.expose_secret(),
                        "intents": DISCORD_INTENTS,
                        "properties": {
                            "os": std::env::consts::OS,
                            "browser": "frankclaw",
                            "device": "frankclaw",
                        }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("identify failed: {e}"),
            })?;

        let seq = Arc::new(AtomicI64::new(-1));
        let heartbeat_seq = seq.clone();
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_millis(
            heartbeat_interval_ms,
        ));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let current = heartbeat_seq.load(Ordering::Relaxed);
                    let payload = if current >= 0 {
                        serde_json::json!({ "op": 1, "d": current })
                    } else {
                        serde_json::json!({ "op": 1, "d": serde_json::Value::Null })
                    };
                    ws_tx
                        .send(Message::Text(payload.to_string().into()))
                        .await
                        .map_err(|e| FrankClawError::Channel {
                            channel: self.id(),
                            msg: format!("heartbeat failed: {e}"),
                        })?;
                }
                frame = ws_rx.next() => {
                    let Some(frame) = frame else {
                        return Err(FrankClawError::Channel {
                            channel: self.id(),
                            msg: "discord gateway closed".into(),
                        });
                    };
                    let frame = frame.map_err(|e| FrankClawError::Channel {
                        channel: self.id(),
                        msg: format!("discord gateway read failed: {e}"),
                    })?;
                    let payload = parse_gateway_message(self.id(), frame)?;
                    if let Some(next_seq) = payload["s"].as_i64() {
                        seq.store(next_seq, Ordering::Relaxed);
                    }

                    match payload["op"].as_i64() {
                        Some(0) => {
                            match payload["t"].as_str() {
                                Some("READY") => {
                                    let mut bot_user_id = self.bot_user_id.lock().await;
                                    *bot_user_id = payload["d"]["user"]["id"].as_str().map(str::to_string);
                                }
                                Some("MESSAGE_CREATE") => {
                                    let bot_user_id = self.bot_user_id.lock().await.clone();
                                    if let Some(inbound) = parse_message_create(
                                        &payload["d"],
                                        bot_user_id.as_deref(),
                                    ) {
                                        if inbound_tx.send(inbound).await.is_err() {
                                            return Ok(());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Some(7) => {
                            return Err(FrankClawError::Channel {
                                channel: self.id(),
                                msg: "discord requested reconnect".into(),
                            });
                        }
                        Some(9) => {
                            return Err(FrankClawError::Channel {
                                channel: self.id(),
                                msg: "discord gateway session invalid".into(),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[async_trait]
impl ChannelPlugin for DiscordChannel {
    fn id(&self) -> ChannelId {
        ChannelId::new("discord")
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            threads: true,
            groups: true,
            attachments: true,
            edit: false,
            delete: false,
            reactions: false,
            streaming: false,
            inline_buttons: false,
            ..Default::default()
        }
    }

    fn label(&self) -> &str {
        "Discord"
    }

    async fn start(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        info!("discord channel starting (gateway mode)");
        loop {
            match self.run_gateway(inbound_tx.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    warn!(error = %err, "discord gateway error, retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn stop(&self) -> Result<()> {
        info!("discord channel stopped");
        Ok(())
    }

    async fn health(&self) -> HealthStatus {
        match self
            .client
            .get(format!("{DISCORD_API_BASE}/users/@me"))
            .header("authorization", self.auth_header())
            .send()
            .await
        {
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
        let channel_id = msg.thread_id.as_deref().unwrap_or(&msg.to);
        let resp = self
            .client
            .post(format!("{DISCORD_API_BASE}/channels/{channel_id}/messages"))
            .header("authorization", self.auth_header())
            .json(&build_send_body(&msg))
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("send failed: {e}"),
            })?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("invalid rate limit response: {e}"),
            })?;
            return Ok(SendResult::RateLimited {
                retry_after_secs: body["retry_after"].as_f64().map(|value| value.ceil() as u64),
            });
        }

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid response: {e}"),
        })?;

        if status.is_success() {
            Ok(SendResult::Sent {
                platform_message_id: body["id"].as_str().unwrap_or_default().to_string(),
            })
        } else {
            Ok(SendResult::Failed {
                reason: body["message"]
                    .as_str()
                    .unwrap_or("unknown discord send failure")
                    .to_string(),
            })
        }
    }
}

async fn next_json_frame(
    channel_id: ChannelId,
    ws_rx: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<serde_json::Value> {
    let Some(frame) = ws_rx.next().await else {
        return Err(FrankClawError::Channel {
            channel: channel_id,
            msg: "discord gateway closed".into(),
        });
    };
    let frame = frame.map_err(|e| FrankClawError::Channel {
        channel: channel_id.clone(),
        msg: format!("discord gateway read failed: {e}"),
    })?;
    parse_gateway_message(channel_id, frame)
}

fn parse_gateway_message(channel_id: ChannelId, frame: Message) -> Result<serde_json::Value> {
    let text = match frame {
        Message::Text(text) => text,
        Message::Binary(bytes) => String::from_utf8(bytes.to_vec()).map_err(|e| FrankClawError::Channel {
            channel: channel_id.clone(),
            msg: format!("discord gateway sent invalid UTF-8: {e}"),
        })?.into(),
        Message::Close(_) => {
            return Err(FrankClawError::Channel {
                channel: channel_id,
                msg: "discord gateway closed".into(),
            });
        }
        _ => {
            return Err(FrankClawError::Channel {
                channel: channel_id,
                msg: "discord gateway sent unexpected frame type".into(),
            });
        }
    };

    serde_json::from_str(text.as_ref()).map_err(|e| FrankClawError::Channel {
        channel: channel_id,
        msg: format!("discord gateway sent invalid JSON: {e}"),
    })
}

fn parse_message_create(
    payload: &serde_json::Value,
    bot_user_id: Option<&str>,
) -> Option<InboundMessage> {
    if payload["author"]["bot"].as_bool() == Some(true) {
        return None;
    }

    let channel_id = payload["channel_id"].as_str()?.to_string();
    let sender_id = payload["author"]["id"].as_str()?.to_string();
    let sender_name = payload["author"]["username"].as_str().map(str::to_string);
    let content = payload["content"].as_str().map(str::to_string);
    let is_group = payload.get("guild_id").is_some();
    let attachments = payload["attachments"]
        .as_array()
        .map(|attachments| {
            attachments
                .iter()
                .map(|attachment| InboundAttachment {
                    media_id: None,
                    mime_type: attachment["content_type"]
                        .as_str()
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    filename: attachment["filename"].as_str().map(str::to_string),
                    size_bytes: attachment["size"].as_u64(),
                    url: attachment["url"].as_str().map(str::to_string),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let timestamp = payload["timestamp"]
        .as_str()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let is_mention = bot_user_id.map(|bot_user_id| {
        payload["mentions"]
            .as_array()
            .map(|mentions| {
                mentions.iter().any(|mention| mention["id"].as_str() == Some(bot_user_id))
            })
            .unwrap_or(false)
    }).unwrap_or(false);

    Some(InboundMessage {
        channel: ChannelId::new("discord"),
        account_id: "default".to_string(),
        sender_id,
        sender_name,
        thread_id: Some(channel_id),
        is_group,
        is_mention,
        text: content,
        attachments,
        platform_message_id: payload["id"].as_str().map(str::to_string),
        timestamp,
    })
}

fn build_send_body(msg: &OutboundMessage) -> serde_json::Value {
    let mut body = serde_json::json!({
        "content": msg.text,
        "allowed_mentions": {
            "parse": []
        }
    })
    ;
    if let Some(reply_to) = &msg.reply_to {
        body["message_reference"] = serde_json::json!({
            "message_id": reply_to
        });
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_create_detects_group_mentions() {
        let inbound = parse_message_create(
            &serde_json::json!({
                "id": "msg-1",
                "channel_id": "chan-1",
                "guild_id": "guild-1",
                "content": "<@999> hello",
                "timestamp": "2026-03-10T12:00:00Z",
                "author": {
                    "id": "user-1",
                    "username": "alice",
                    "bot": false
                },
                "mentions": [
                    { "id": "999" }
                ]
            }),
            Some("999"),
        )
        .expect("message should parse");

        assert!(inbound.is_group);
        assert!(inbound.is_mention);
        assert_eq!(inbound.thread_id.as_deref(), Some("chan-1"));
    }

    #[test]
    fn parse_message_create_skips_bot_messages() {
        let inbound = parse_message_create(
            &serde_json::json!({
                "id": "msg-1",
                "channel_id": "chan-1",
                "content": "hello",
                "author": {
                    "id": "bot-1",
                    "username": "bot",
                    "bot": true
                }
            }),
            Some("999"),
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn build_send_body_uses_content_field() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "chan-1".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["content"], serde_json::json!("hello"));
    }

    #[test]
    fn build_send_body_includes_reply_reference_when_present() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "chan-1".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: Some("msg-99".into()),
        });

        assert_eq!(
            body["message_reference"]["message_id"],
            serde_json::json!("msg-99")
        );
        assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!([]));
    }

    #[test]
    fn parse_message_create_collects_attachment_metadata() {
        let inbound = parse_message_create(
            &serde_json::json!({
                "id": "msg-1",
                "channel_id": "chan-1",
                "content": "",
                "timestamp": "2026-03-10T12:00:00Z",
                "author": {
                    "id": "user-1",
                    "username": "alice",
                    "bot": false
                },
                "attachments": [
                    {
                        "filename": "image.png",
                        "content_type": "image/png",
                        "size": 1234,
                        "url": "https://cdn.discordapp.com/file.png"
                    }
                ]
            }),
            Some("999"),
        )
        .expect("message should parse");

        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].filename.as_deref(), Some("image.png"));
        assert_eq!(inbound.attachments[0].mime_type, "image/png");
        assert_eq!(inbound.attachments[0].size_bytes, Some(1234));
    }
}
