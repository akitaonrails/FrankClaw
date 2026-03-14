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

use crate::inbound_media::infer_inbound_mime_type;
use crate::outbound_media::{attachment_bytes, attachment_filename};
use crate::outbound_text::{normalize_outbound_text, OutboundTextFlavor};

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DISCORD_GATEWAY_VERSION: &str = "10";
const DISCORD_INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 12) | (1 << 15);

/// Discord message content limit in characters (code points).
const DISCORD_MESSAGE_LIMIT: usize = 2000;

/// Maximum time to wait for the HELLO frame after connecting.
const DISCORD_HELLO_TIMEOUT_SECS: u64 = 30;

pub struct DiscordChannel {
    bot_token: SecretString,
    client: Client,
    bot_user_id: Mutex<Option<String>>,
}

impl DiscordChannel {
    pub fn new(bot_token: SecretString) -> Result<Self> {
        let client = crate::build_channel_http_client()?;

        Ok(Self {
            bot_token,
            client,
            bot_user_id: Mutex::new(None),
        })
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
            .map_err(|e| self.channel_err(format!("gateway discovery failed: {e}")))?;

        let body: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid gateway discovery response: {e}")))?;

        let url = body["url"]
            .as_str()
            .ok_or_else(|| self.channel_err("discord gateway discovery did not return a url".into()))?;

        Ok(format!("{url}/?v={DISCORD_GATEWAY_VERSION}&encoding=json"))
    }

    async fn run_gateway(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        let gateway_url = self.gateway_url().await?;
        let (socket, _) = tokio_tungstenite::connect_async(gateway_url)
            .await
            .map_err(|e| self.channel_err(format!("gateway connect failed: {e}")))?;
        let (mut ws_tx, mut ws_rx) = socket.split();

        let hello = tokio::time::timeout(
            std::time::Duration::from_secs(DISCORD_HELLO_TIMEOUT_SECS),
            next_json_frame(self.id(), &mut ws_rx),
        )
        .await
        .map_err(|_| self.channel_err("discord gateway HELLO timeout after 30s".into()))??;
        let heartbeat_interval_ms = hello["d"]["heartbeat_interval"]
            .as_u64()
            .ok_or_else(|| self.channel_err("discord hello payload missing heartbeat interval".into()))?;

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
            .map_err(|e| self.channel_err(format!("identify failed: {e}")))?;

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
                        .map_err(|e| self.channel_err(format!("heartbeat failed: {e}")))?;
                }
                frame = ws_rx.next() => {
                    let Some(frame) = frame else {
                        return Err(self.channel_err("discord gateway closed".into()));
                    };
                    let frame = frame.map_err(|e| self.channel_err(format!("discord gateway read failed: {e}")))?;
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
                                    )
                                        && inbound_tx.send(inbound).await.is_err() {
                                            return Ok(());
                                        }
                                }
                                _ => {}
                            }
                        }
                        Some(7) => {
                            return Err(self.channel_err("discord requested reconnect".into()));
                        }
                        Some(9) => {
                            return Err(self.channel_err("discord gateway session invalid".into()));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    async fn send_single(&self, msg: OutboundMessage) -> Result<SendResult> {
        let channel_id = msg.thread_id.as_deref().unwrap_or(&msg.to);
        let request = self
            .client
            .post(format!("{DISCORD_API_BASE}/channels/{channel_id}/messages"))
            .header("authorization", self.auth_header());
        let resp = if msg.attachments.is_empty() {
            request.json(&build_send_body(&msg)).send().await
        } else {
            request.multipart(build_send_form(&msg)?).send().await
        }
        .map_err(|e| self.channel_err(format!("send failed: {e}")))?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let body: serde_json::Value =
                resp.json().await.map_err(|e| self.channel_err(format!("invalid rate limit response: {e}")))?;
            return Ok(SendResult::RateLimited {
                retry_after_secs: body["retry_after"]
                    .as_f64()
                    .map(|value| value.ceil() as u64),
            });
        }

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid response: {e}")))?;

        if status.is_success() {
            Ok(SendResult::Sent {
                platform_message_id: body["id"].as_str().unwrap_or_default().to_string(),
            })
        } else {
            let error_code = body["code"].as_u64().unwrap_or(0);
            let reason = match error_code {
                50007 => "cannot send messages to this user (DM blocked)".to_string(),
                50013 => "missing permissions to send messages in this channel".to_string(),
                _ => body["message"]
                    .as_str()
                    .unwrap_or("unknown discord send failure")
                    .to_string(),
            };
            Ok(SendResult::Failed { reason })
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
            edit: true,
            delete: true,
            reactions: false,
            streaming: true,
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
                    if is_fatal_gateway_error(&err) {
                        return Err(err);
                    }
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
        // Chunk text messages that exceed Discord's 2000-char limit.
        if msg.attachments.is_empty() {
            let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
            if text.chars().count() > DISCORD_MESSAGE_LIMIT {
                let chunks = chunk_discord_text(&text, DISCORD_MESSAGE_LIMIT);
                let mut last_result = None;
                for (i, chunk) in chunks.iter().enumerate() {
                    let mut chunk_msg = msg.clone();
                    chunk_msg.text = chunk.clone();
                    if i > 0 {
                        chunk_msg.reply_to = None;
                    }
                    let result = self.send_single(chunk_msg).await?;
                    if !matches!(result, SendResult::Sent { .. }) {
                        return Ok(result);
                    }
                    last_result = Some(result);
                }
                return Ok(last_result.unwrap_or_else(|| SendResult::Failed {
                    reason: "no chunks to send".into(),
                }));
            }
        }

        self.send_single(msg).await
    }

    async fn edit_message(&self, target: &EditMessageTarget, new_text: &str) -> Result<()> {
        let (channel_id, body) = build_edit_request(target, new_text);
        let resp = self
            .client
            .patch(format!(
                "{DISCORD_API_BASE}/channels/{channel_id}/messages/{}",
                target.platform_message_id
            ))
            .header("authorization", self.auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| self.channel_err(format!("discord edit failed: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let body: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid discord edit response: {e}")))?;
            Err(self.channel_err(body["message"]
                    .as_str()
                    .unwrap_or("unknown discord edit failure")
                    .to_string()))
        }
    }

    async fn stream_start(&self, msg: &OutboundMessage) -> Result<frankclaw_core::channel::StreamHandle> {
        match self.send(msg.clone()).await? {
            SendResult::Sent { platform_message_id } => Ok(frankclaw_core::channel::StreamHandle {
                channel: self.id(),
                account_id: msg.account_id.clone(),
                to: msg.to.clone(),
                thread_id: msg.thread_id.clone(),
                draft_message_id: platform_message_id,
            }),
            SendResult::RateLimited { retry_after_secs } => Err(FrankClawError::RateLimited {
                retry_after_secs: retry_after_secs.unwrap_or(1),
            }),
            SendResult::Failed { reason } => Err(self.channel_err(reason)),
        }
    }

    async fn stream_update(
        &self,
        handle: &frankclaw_core::channel::StreamHandle,
        text: &str,
    ) -> Result<()> {
        self.edit_message(
            &EditMessageTarget {
                account_id: handle.account_id.clone(),
                to: handle.to.clone(),
                thread_id: handle.thread_id.clone(),
                platform_message_id: handle.draft_message_id.clone(),
            },
            text,
        )
        .await
    }

    async fn stream_end(
        &self,
        handle: &frankclaw_core::channel::StreamHandle,
        final_text: &str,
    ) -> Result<()> {
        self.stream_update(handle, final_text).await
    }

    async fn delete_message(&self, target: &DeleteMessageTarget) -> Result<()> {
        let channel_id = target.thread_id.as_deref().unwrap_or(&target.to);
        let resp = self
            .client
            .delete(format!(
                "{DISCORD_API_BASE}/channels/{channel_id}/messages/{}",
                target.platform_message_id
            ))
            .header("authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| self.channel_err(format!("discord delete failed: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let body: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid discord delete response: {e}")))?;
            Err(self.channel_err(body["message"]
                    .as_str()
                    .unwrap_or("unknown discord delete failure")
                    .to_string()))
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
        Message::Close(close_frame) => {
            let (code, reason) = close_frame
                .map_or((0u16, String::new()), |cf| (cf.code.into(), cf.reason.to_string()));
            return Err(FrankClawError::Channel {
                channel: channel_id,
                msg: format!("discord gateway closed (code={code}, reason={reason})"),
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
                    mime_type: infer_inbound_mime_type(
                        attachment["content_type"].as_str(),
                        attachment["filename"].as_str(),
                        attachment["url"].as_str(),
                    ),
                    filename: attachment["filename"].as_str().map(str::to_string),
                    size_bytes: attachment["size"].as_u64(),
                    url: attachment["url"].as_str().map(str::to_string),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let timestamp = payload["timestamp"]
        .as_str()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok()).map_or_else(chrono::Utc::now, |value| value.with_timezone(&chrono::Utc));
    let is_mention = bot_user_id.is_some_and(|bot_user_id| {
        payload["mentions"]
            .as_array()
            .is_some_and(|mentions| {
                mentions.iter().any(|mention| mention["id"].as_str() == Some(bot_user_id))
            })
    });

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
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
    let mut body = serde_json::json!({
        "content": text,
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

fn build_send_form(msg: &OutboundMessage) -> Result<reqwest::multipart::Form> {
    let (payload, specs) = build_send_attachment_payload(msg)?;
    let mut form = reqwest::multipart::Form::new().text("payload_json", payload.to_string());
    for spec in specs {
        let part = reqwest::multipart::Part::bytes(spec.bytes)
            .file_name(spec.filename)
            .mime_str(&spec.mime_type)
            .map_err(|e| FrankClawError::Channel {
                channel: ChannelId::new("discord"),
                msg: format!("invalid attachment mime type: {e}"),
            })?;
        form = form.part(spec.field_name, part);
    }
    Ok(form)
}

#[derive(Debug)]
struct DiscordAttachmentSpec {
    field_name: String,
    filename: String,
    mime_type: String,
    bytes: Vec<u8>,
}

fn build_send_attachment_payload(
    msg: &OutboundMessage,
) -> Result<(serde_json::Value, Vec<DiscordAttachmentSpec>)> {
    let channel = ChannelId::new("discord");
    let mut payload = build_send_body(msg);
    let mut metadata = Vec::with_capacity(msg.attachments.len());
    let mut specs = Vec::with_capacity(msg.attachments.len());

    for (index, attachment) in msg.attachments.iter().enumerate() {
        let filename = attachment_filename(attachment);
        metadata.push(serde_json::json!({
            "id": index,
            "filename": filename,
        }));
        specs.push(DiscordAttachmentSpec {
            field_name: format!("files[{index}]"),
            filename,
            mime_type: attachment.mime_type.clone(),
            bytes: attachment_bytes(&channel, attachment)?,
        });
    }

    payload["attachments"] = serde_json::Value::Array(metadata);
    Ok((payload, specs))
}

fn build_edit_request(target: &EditMessageTarget, new_text: &str) -> (String, serde_json::Value) {
    let text = normalize_outbound_text(new_text, OutboundTextFlavor::Plain);
    (
        target.thread_id.clone().unwrap_or_else(|| target.to.clone()),
        serde_json::json!({ "content": text }),
    )
}

/// Fatal Discord gateway close codes that should not be retried.
/// 4004 = auth failed, 4010 = invalid shard, 4011 = sharding required,
/// 4012 = invalid API version, 4013 = invalid intents, 4014 = disallowed intents.
fn is_fatal_gateway_error(err: &FrankClawError) -> bool {
    let msg = err.to_string();
    for code in ["4004", "4010", "4011", "4012", "4013", "4014"] {
        if msg.contains(&format!("code={code}")) {
            return true;
        }
    }
    false
}

/// Split text into chunks of at most `limit` characters, preferring newline boundaries.
fn chunk_discord_text(text: &str, limit: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + limit).min(chars.len());
        if end == chars.len() {
            chunks.push(chars[start..end].iter().collect());
            break;
        }

        // Try to split at a newline near the limit.
        let slice = &chars[start..end];
        let split_at = slice
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(end, |pos| start + pos + 1);

        chunks.push(chars[start..split_at].iter().collect());
        start = split_at;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        match name {
            "message_create_with_attachment" => serde_json::from_str(include_str!(
                "fixture_discord_message_create_with_attachment.json"
            ))
            .expect("fixture should parse"),
            _ => panic!("unknown fixture: {name}"),
        }
    }

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
    fn build_send_body_trims_plain_outbound_text() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "chan-1".into(),
            thread_id: None,
            text: "\n hello \r\n".into(),
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
    fn build_send_attachment_payload_includes_attachment_manifest() {
        let (payload, specs) = build_send_attachment_payload(&OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "chan-1".into(),
            thread_id: None,
            text: "see attached".into(),
            attachments: vec![OutboundAttachment {
                media_id: frankclaw_core::types::MediaId::new(),
                mime_type: "image/png".into(),
                filename: Some("photo.png".into()),
                url: None,
                bytes: b"png".to_vec(),
            }],
            reply_to: None,
        })
        .expect("attachment payload should build");

        assert_eq!(
            payload["attachments"],
            serde_json::json!([{ "id": 0, "filename": "photo.png" }])
        );
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].field_name, "files[0]");
    }

    #[test]
    fn build_send_attachment_payload_supports_multiple_files() {
        let (payload, specs) = build_send_attachment_payload(&OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "chan-1".into(),
            thread_id: None,
            text: "see attached".into(),
            attachments: vec![
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    url: None,
                    bytes: b"png".to_vec(),
                },
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "application/pdf".into(),
                    filename: Some("report.pdf".into()),
                    url: None,
                    bytes: b"%PDF".to_vec(),
                },
            ],
            reply_to: Some("msg-42".into()),
        })
        .expect("attachment payload should build");

        assert_eq!(
            payload["attachments"],
            serde_json::json!([
                { "id": 0, "filename": "photo.png" },
                { "id": 1, "filename": "report.pdf" }
            ])
        );
        assert_eq!(payload["message_reference"]["message_id"], serde_json::json!("msg-42"));
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].field_name, "files[0]");
        assert_eq!(specs[1].field_name, "files[1]");
    }

    #[test]
    fn build_send_attachment_payload_requires_inline_bytes() {
        let err = build_send_attachment_payload(&OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "chan-1".into(),
            thread_id: None,
            text: "see attached".into(),
            attachments: vec![OutboundAttachment {
                media_id: frankclaw_core::types::MediaId::new(),
                mime_type: "image/png".into(),
                filename: Some("photo.png".into()),
                url: None,
                bytes: Vec::new(),
            }],
            reply_to: None,
        })
        .expect_err("missing bytes should fail");

        assert!(err.to_string().contains("missing inline bytes"));
    }

    #[test]
    fn build_edit_request_prefers_thread_target() {
        let (channel_id, body) = build_edit_request(
            &EditMessageTarget {
                account_id: "default".into(),
                to: "chan-1".into(),
                thread_id: Some("thread-9".into()),
                platform_message_id: "msg-99".into(),
            },
            "updated",
        );

        assert_eq!(channel_id, "thread-9");
        assert_eq!(body["content"], serde_json::json!("updated"));
    }

    #[test]
    fn delete_uses_thread_target_channel() {
        let target = DeleteMessageTarget {
            account_id: "default".into(),
            to: "chan-1".into(),
            thread_id: Some("thread-9".into()),
            platform_message_id: "msg-99".into(),
        };

        assert_eq!(target.thread_id.as_deref().unwrap_or(&target.to), "thread-9");
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

    #[test]
    fn parse_message_create_infers_attachment_mime_type_from_filename() {
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
                        "filename": "image.jpeg",
                        "size": 1234,
                        "url": "https://cdn.discordapp.com/file.jpeg"
                    }
                ]
            }),
            Some("999"),
        )
        .expect("message should parse");

        assert_eq!(inbound.attachments[0].mime_type, "image/jpeg");
    }

    #[test]
    fn parse_message_create_matches_contract_fixture_shape() {
        let inbound = parse_message_create(&fixture("message_create_with_attachment"), Some("999"))
            .expect("fixture should parse");

        assert_eq!(inbound.channel.as_str(), "discord");
        assert_eq!(inbound.sender_id, "user-1");
        assert_eq!(inbound.text.as_deref(), Some("image upload"));
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(
            inbound.attachments[0].url.as_deref(),
            Some("https://cdn.discordapp.com/attachments/att-1/photo.png")
        );
    }

    // --- Audit regression tests ---

    #[test]
    fn chunk_discord_text_returns_single_chunk_when_within_limit() {
        let text = "hello world";
        let chunks = chunk_discord_text(text, 2000);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn chunk_discord_text_splits_at_newline_boundary() {
        let line_a = "a".repeat(1500);
        let line_b = "b".repeat(800);
        let text = format!("{line_a}\n{line_b}");
        let chunks = chunk_discord_text(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], format!("{line_a}\n"));
        assert_eq!(chunks[1], line_b);
    }

    #[test]
    fn chunk_discord_text_splits_at_limit_when_no_newline() {
        let text = "a".repeat(4500);
        let chunks = chunk_discord_text(&text, 2000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 2000);
        assert_eq!(chunks[2].len(), 500);
    }

    #[test]
    fn chunk_discord_text_handles_multibyte_characters() {
        // 2001 emoji characters (each 4 bytes in UTF-8)
        let text: String = std::iter::repeat('🦀').take(2001).collect();
        let chunks = chunk_discord_text(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), 2000);
        assert_eq!(chunks[1].chars().count(), 1);
    }

    #[test]
    fn is_fatal_gateway_error_detects_disallowed_intents() {
        let err = FrankClawError::Channel {
            channel: ChannelId::new("discord"),
            msg: "discord gateway closed (code=4014, reason=Disallowed intents)".into(),
        };
        assert!(is_fatal_gateway_error(&err));
    }

    #[test]
    fn is_fatal_gateway_error_detects_auth_failed() {
        let err = FrankClawError::Channel {
            channel: ChannelId::new("discord"),
            msg: "discord gateway closed (code=4004, reason=Authentication failed)".into(),
        };
        assert!(is_fatal_gateway_error(&err));
    }

    #[test]
    fn is_fatal_gateway_error_does_not_match_retriable_errors() {
        let err = FrankClawError::Channel {
            channel: ChannelId::new("discord"),
            msg: "discord gateway closed (code=4000, reason=Unknown error)".into(),
        };
        assert!(!is_fatal_gateway_error(&err));
    }

    #[test]
    fn is_fatal_gateway_error_does_not_match_non_close_errors() {
        let err = FrankClawError::Channel {
            channel: ChannelId::new("discord"),
            msg: "discord gateway HELLO timeout after 30s".into(),
        };
        assert!(!is_fatal_gateway_error(&err));
    }
}
