use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::{info, warn};

use frankclaw_core::channel::{OutboundMessage, SendResult, ChannelPlugin, InboundMessage, InboundAttachment, ChannelCapabilities, HealthStatus, EditMessageTarget, DeleteMessageTarget, OutboundAttachment};
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

use crate::media_text::text_or_attachment_placeholder;
use crate::outbound_media::{
    AttachmentKind, attachment_bytes, attachment_filename, attachment_kind,
    require_single_attachment,
};
use crate::outbound_text::{normalize_outbound_text, OutboundTextFlavor};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Telegram caption limit in characters. Captions longer than this must be
/// split: media sent without caption, then a follow-up text message.
const TELEGRAM_CAPTION_LIMIT: usize = 1024;

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
    pub fn new(bot_token: SecretString) -> Result<Self> {
        let client = crate::build_channel_http_client()?;

        Ok(Self::with_client(bot_token, client))
    }

    pub(crate) fn with_client(bot_token: SecretString, client: Client) -> Self {
        Self {
            bot_token,
            client,
            update_offset: std::sync::atomic::AtomicI64::new(0),
        }
    }

    async fn send_with_caption_overflow(&self, msg: OutboundMessage) -> Result<SendResult> {
        // Send media without caption.
        let mut media_msg = msg.clone();
        media_msg.text = String::new();
        let result = self
            .send_with_retries(media_msg, SendRetryState::initial())
            .await?;

        // Best-effort follow-up text message.
        if matches!(result, SendResult::Sent { .. }) {
            let mut text_msg = msg;
            text_msg.attachments = Vec::new();
            if let Err(e) = self
                .send_with_retries(text_msg, SendRetryState::initial())
                .await
            {
                warn!(error = %e, "telegram caption overflow follow-up text failed");
            }
        }

        Ok(result)
    }

    fn send_with_retries(
        &self,
        msg: OutboundMessage,
        state: SendRetryState,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SendResult>> + Send + '_>> {
        Box::pin(self.send_with_retries_inner(msg, state))
    }

    async fn send_with_retries_inner(
        &self,
        msg: OutboundMessage,
        state: SendRetryState,
    ) -> Result<SendResult> {
        let effective_msg = if state.include_thread_id {
            msg.clone()
        } else {
            let mut m = msg.clone();
            m.thread_id = None;
            m
        };

        let resp = if effective_msg.attachments.is_empty() {
            let mut body = build_send_body(&effective_msg);
            if !state.include_parse_mode
                && let Some(obj) = body.as_object_mut() {
                    obj.remove("parse_mode");
                }
            self.client
                .post(self.api_url("sendMessage"))
                .json(&body)
                .send()
                .await
        } else {
            let request =
                build_media_send_request(&effective_msg, state.include_parse_mode)?;
            self.client
                .post(self.api_url(request.method))
                .multipart(request.form)
                .send()
                .await
        }
        .map_err(|e| self.channel_err(format!("send failed: {e}")))?;

        let data: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid response: {e}")))?;

        if data["ok"].as_bool() == Some(true) {
            let msg_id = data["result"]["message_id"]
                .as_i64()
                .unwrap_or(0)
                .to_string();
            return Ok(SendResult::Sent {
                platform_message_id: msg_id,
            });
        }

        let description = data["description"]
            .as_str()
            .unwrap_or("unknown error")
            .to_string();

        // Rate limiting.
        if data["error_code"].as_i64() == Some(429) {
            let retry_after = data["parameters"]["retry_after"].as_u64();
            return Ok(SendResult::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        // Parse error fallback: retry without parse_mode.
        if is_parse_error(&description) && state.include_parse_mode {
            warn!("telegram parse error, retrying without parse_mode");
            return self
                .send_with_retries(
                    msg,
                    SendRetryState {
                        include_parse_mode: false,
                        ..state
                    },
                )
                .await;
        }

        // Thread-not-found fallback: retry without thread_id (DMs only).
        if is_thread_not_found(&description) && state.include_thread_id {
            let (chat_id, _) = parse_target_thread(msg.thread_id.as_deref(), &msg.to);
            if is_dm_chat_id(&chat_id) {
                warn!("telegram thread not found in DM, retrying without thread_id");
                return self
                    .send_with_retries(
                        msg,
                        SendRetryState {
                            include_thread_id: false,
                            ..state
                        },
                    )
                    .await;
            }
        }

        Ok(SendResult::Failed {
            reason: description,
        })
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
            .map_err(|e| self.channel_err(format!("poll failed: {e}")))?;

        let data: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid response: {e}")))?;

        if let Some(updates) = data["result"].as_array() {
            for update in updates {
                // Track offset to avoid reprocessing.
                if let Some(id) = update["update_id"].as_i64() {
                    self.update_offset
                        .store(id + 1, std::sync::atomic::Ordering::Relaxed);
                }

                let msg = update.get("message").or_else(|| update.get("edited_message"));
                if let Some(msg) = msg
                    && let Some(inbound) = self.parse_message(msg)
                        && inbound_tx.send(inbound).await.is_err() {
                            return Ok(()); // Receiver dropped, shutting down.
                        }
            }
        }

        Ok(())
    }

    fn parse_message(&self, msg: &serde_json::Value) -> Option<InboundMessage> {
        let chat_id = msg["chat"]["id"].as_i64()?;
        let sender_id = msg["from"]["id"].as_i64()?.to_string();
        let sender_name = msg["from"]["first_name"].as_str().map(String::from);
        let attachments = build_inbound_attachments(msg);
        let text = text_or_attachment_placeholder(
            msg["text"].as_str().or_else(|| msg["caption"].as_str()),
            &attachments,
        );
        let topic_id = msg["message_thread_id"].as_i64();
        let is_group = matches!(
            msg["chat"]["type"].as_str(),
            Some("group" | "supergroup")
        );
        let message_id = msg["message_id"].as_i64()?.to_string();

        let timestamp = msg["date"]
            .as_i64().map_or_else(chrono::Utc::now, |ts| {
                chrono::DateTime::from_timestamp(ts, 0)
                    .unwrap_or_else(chrono::Utc::now)
            });

        Some(InboundMessage {
            channel: self.id(),
            account_id: "default".to_string(),
            sender_id,
            sender_name,
            thread_id: Some(encode_thread_id(chat_id, topic_id)),
            is_group,
            is_mention: text
                .as_deref()
                .is_some_and(|t| t.contains('@')),
            text,
            attachments,
            platform_message_id: Some(message_id),
            timestamp,
        })
    }
}

#[derive(Clone, Copy)]
struct SendRetryState {
    include_parse_mode: bool,
    include_thread_id: bool,
}

impl SendRetryState {
    fn initial() -> Self {
        Self {
            include_parse_mode: true,
            include_thread_id: true,
        }
    }
}

fn is_parse_error(description: &str) -> bool {
    description.contains("can't parse entities")
}

fn is_message_not_modified(description: &str) -> bool {
    description.contains("message is not modified")
}

fn is_thread_not_found(description: &str) -> bool {
    description.contains("message thread not found")
}

fn is_dm_chat_id(raw: &str) -> bool {
    raw.parse::<i64>().is_ok_and(|id| id > 0)
}

fn build_inbound_attachments(msg: &serde_json::Value) -> Vec<InboundAttachment> {
    let mut attachments = Vec::new();

    if let Some(photo) = msg["photo"].as_array().and_then(|entries| entries.last()) {
        attachments.push(InboundAttachment {
            media_id: None,
            mime_type: "image/jpeg".into(),
            filename: None,
            size_bytes: photo["file_size"].as_u64(),
            url: None,
        });
    }

    for (key, mime_fallback) in [
        ("document", "application/octet-stream"),
        ("video", "video/mp4"),
        ("audio", "audio/mpeg"),
        ("voice", "audio/ogg"),
        ("sticker", "image/webp"),
        ("animation", "video/mp4"),
    ] {
        let media = &msg[key];
        if media.is_null() {
            continue;
        }
        attachments.push(InboundAttachment {
            media_id: None,
            mime_type: media["mime_type"]
                .as_str()
                .unwrap_or(mime_fallback)
                .to_string(),
            filename: media["file_name"].as_str().map(str::to_string),
            size_bytes: media["file_size"].as_u64(),
            url: None,
        });
    }

    attachments
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

    fn label(&self) -> &'static str {
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
        // Caption overflow: if text exceeds Telegram's 1024-char caption limit
        // and we have attachments, send media without caption then follow-up text.
        if !msg.attachments.is_empty() {
            let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
            if text.len() > TELEGRAM_CAPTION_LIMIT {
                return self.send_with_caption_overflow(msg).await;
            }
        }

        self.send_with_retries(msg, SendRetryState::initial()).await
    }

    async fn edit_message(&self, target: &EditMessageTarget, new_text: &str) -> Result<()> {
        let body = build_edit_body(target, new_text)?;

        let resp = self
            .client
            .post(self.api_url("editMessageText"))
            .json(&body)
            .send()
            .await
            .map_err(|e| self.channel_err(format!("edit failed: {e}")))?;

        let data: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid response: {e}")))?;

        if data["ok"].as_bool() == Some(true) {
            return Ok(());
        }

        let description = data["description"]
            .as_str()
            .unwrap_or("unknown telegram edit error");

        // Treat "message is not modified" as success (idempotent edit).
        if is_message_not_modified(description) {
            return Ok(());
        }

        Err(self.channel_err(description.to_string()))
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
        let body = build_delete_body(target)?;
        let resp = self
            .client
            .post(self.api_url("deleteMessage"))
            .json(&body)
            .send()
            .await
            .map_err(|e| self.channel_err(format!("delete failed: {e}")))?;

        let data: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid response: {e}")))?;

        if data["ok"].as_bool() == Some(true) {
            Ok(())
        } else {
            Err(self.channel_err(data["description"]
                    .as_str()
                    .unwrap_or("unknown telegram delete error")
                    .to_string()))
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
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
    let mut body = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "Markdown",
    });

    if let Some(topic_id) = topic_id {
        body["message_thread_id"] = serde_json::json!(topic_id);
    }
    if let Some(reply_to) = msg
        .reply_to
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
    {
        body["reply_to_message_id"] = serde_json::json!(reply_to);
    }

    body
}

#[derive(Debug)]
struct TelegramMediaRequest {
    method: &'static str,
    form: reqwest::multipart::Form,
}

fn build_media_send_request(
    msg: &OutboundMessage,
    include_parse_mode: bool,
) -> Result<TelegramMediaRequest> {
    if msg.attachments.len() > 1 {
        return build_media_group_request(msg, include_parse_mode);
    }

    let channel = ChannelId::new("telegram");
    let attachment = require_single_attachment(&channel, &msg.attachments)?;
    let bytes = attachment_bytes(&channel, attachment)?;
    let filename = attachment_filename(attachment);
    let (chat_id, topic_id) = parse_target_thread(msg.thread_id.as_deref(), &msg.to);
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);

    let (method, field_name) = match attachment_kind(&attachment.mime_type) {
        AttachmentKind::Image => ("sendPhoto", "photo"),
        AttachmentKind::Audio => ("sendAudio", "audio"),
        AttachmentKind::Video => ("sendVideo", "video"),
        AttachmentKind::Document => ("sendDocument", "document"),
    };

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&attachment.mime_type)
        .map_err(|e| FrankClawError::Channel {
            channel,
            msg: format!("invalid attachment mime type: {e}"),
        })?;

    let mut form = reqwest::multipart::Form::new()
        .text("chat_id", chat_id)
        .part(field_name.to_string(), part);

    if !text.is_empty() {
        form = form.text("caption", text);
        if include_parse_mode {
            form = form.text("parse_mode", "Markdown");
        }
    }
    if let Some(topic_id) = topic_id {
        form = form.text("message_thread_id", topic_id.to_string());
    }
    if let Some(reply_to) = msg
        .reply_to
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
    {
        form = form.text("reply_to_message_id", reply_to.to_string());
    }

    Ok(TelegramMediaRequest { method, form })
}

fn build_media_group_request(
    msg: &OutboundMessage,
    include_parse_mode: bool,
) -> Result<TelegramMediaRequest> {
    let channel = ChannelId::new("telegram");
    let (chat_id, topic_id) = parse_target_thread(msg.thread_id.as_deref(), &msg.to);
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
    let media = build_media_group_items(&channel, &msg.attachments, &text, include_parse_mode)?;
    let mut form = reqwest::multipart::Form::new().text("chat_id", chat_id);

    for (index, attachment) in msg.attachments.iter().enumerate() {
        let field_name = format!("file{index}");
        let part = reqwest::multipart::Part::bytes(attachment_bytes(&channel, attachment)?)
            .file_name(attachment_filename(attachment))
            .mime_str(&attachment.mime_type)
            .map_err(|e| FrankClawError::Channel {
                channel: channel.clone(),
                msg: format!("invalid attachment mime type: {e}"),
            })?;
        form = form.part(field_name.clone(), part);
    }

    form = form.text(
        "media",
        serde_json::to_string(&media).map_err(|e| FrankClawError::Channel {
            channel: channel.clone(),
            msg: format!("failed to serialize telegram media group: {e}"),
        })?,
    );
    if let Some(topic_id) = topic_id {
        form = form.text("message_thread_id", topic_id.to_string());
    }
    if let Some(reply_to) = msg
        .reply_to
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
    {
        form = form.text("reply_to_message_id", reply_to.to_string());
    }

    Ok(TelegramMediaRequest {
        method: "sendMediaGroup",
        form,
    })
}

fn build_media_group_items(
    channel: &ChannelId,
    attachments: &[OutboundAttachment],
    text: &str,
    include_parse_mode: bool,
) -> Result<Vec<serde_json::Value>> {
    if !(2..=10).contains(&attachments.len()) {
        return Err(FrankClawError::Channel {
            channel: channel.clone(),
            msg: "telegram media groups require between 2 and 10 attachments".into(),
        });
    }

    let mut media = Vec::with_capacity(attachments.len());
    let mut group_kind = None;

    for (index, attachment) in attachments.iter().enumerate() {
        let kind = attachment_kind(&attachment.mime_type);
        let next_group_kind = telegram_media_group_kind(kind);
        if let Some(existing) = group_kind {
            if existing != next_group_kind {
                return Err(FrankClawError::Channel {
                    channel: channel.clone(),
                    msg: "telegram media groups must be all photos/videos, all audio, or all documents"
                        .into(),
                });
            }
        } else {
            group_kind = Some(next_group_kind);
        }

        let mut item = serde_json::json!({
            "type": telegram_media_item_type(kind),
            "media": format!("attach://file{index}"),
        });
        if index == 0 && !text.is_empty() {
            item["caption"] = serde_json::json!(text);
            if include_parse_mode {
                item["parse_mode"] = serde_json::json!("Markdown");
            }
        }
        media.push(item);
    }

    Ok(media)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TelegramMediaGroupKind {
    Visual,
    Audio,
    Document,
}

fn telegram_media_group_kind(kind: AttachmentKind) -> TelegramMediaGroupKind {
    match kind {
        AttachmentKind::Image | AttachmentKind::Video => TelegramMediaGroupKind::Visual,
        AttachmentKind::Audio => TelegramMediaGroupKind::Audio,
        AttachmentKind::Document => TelegramMediaGroupKind::Document,
    }
}

fn telegram_media_item_type(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "photo",
        AttachmentKind::Audio => "audio",
        AttachmentKind::Video => "video",
        AttachmentKind::Document => "document",
    }
}

fn build_edit_body(target: &EditMessageTarget, new_text: &str) -> Result<serde_json::Value> {
    let (chat_id, _) = parse_target_thread(target.thread_id.as_deref(), &target.to);
    let text = normalize_outbound_text(new_text, OutboundTextFlavor::Plain);
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
        "text": text,
        "parse_mode": "Markdown",
    }))
}

fn build_delete_body(target: &DeleteMessageTarget) -> Result<serde_json::Value> {
    let (chat_id, _) = parse_target_thread(target.thread_id.as_deref(), &target.to);
    let message_id = target
        .platform_message_id
        .parse::<i64>()
        .map_err(|_| FrankClawError::Channel {
            channel: ChannelId::new("telegram"),
            msg: "telegram delete requires a numeric platform message id".into(),
        })?;

    Ok(serde_json::json!({
        "chat_id": chat_id,
        "message_id": message_id,
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

    fn fixture(name: &str) -> serde_json::Value {
        match name {
            "message_with_photo" => serde_json::from_str(include_str!(
                "fixture_telegram_message_with_photo.json"
            ))
            .expect("fixture should parse"),
            _ => panic!("unknown fixture: {name}"),
        }
    }

    #[test]
    fn parse_message_uses_topic_thread_id_when_present() {
        let channel = TelegramChannel::new(SecretString::from("token".to_string())).expect("channel should build");
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
    fn build_send_body_includes_reply_to_message_id_when_present() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("telegram"),
            account_id: "default".into(),
            to: "42".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: Some("99".into()),
        });

        assert_eq!(body["reply_to_message_id"], serde_json::json!(99));
    }

    #[test]
    fn build_send_body_trims_plain_outbound_text() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("telegram"),
            account_id: "default".into(),
            to: "42".into(),
            thread_id: None,
            text: "\n hello \r\n".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["text"], serde_json::json!("hello"));
    }

    #[test]
    fn build_media_send_request_uses_photo_method_for_images() {
        let request = build_media_send_request(
            &OutboundMessage {
                channel: ChannelId::new("telegram"),
                account_id: "default".into(),
                to: "42".into(),
                thread_id: None,
                text: "caption".into(),
                attachments: vec![OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    url: None,
                    bytes: b"png".to_vec(),
                }],
                reply_to: Some("77".into()),
            },
            true,
        )
        .expect("media send request should build");

        assert_eq!(request.method, "sendPhoto");
    }

    #[test]
    fn build_media_send_request_rejects_missing_bytes() {
        let err = build_media_send_request(
            &OutboundMessage {
                channel: ChannelId::new("telegram"),
                account_id: "default".into(),
                to: "42".into(),
                thread_id: None,
                text: "caption".into(),
                attachments: vec![OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    url: None,
                    bytes: Vec::new(),
                }],
                reply_to: None,
            },
            true,
        )
        .expect_err("missing bytes should fail");

        assert!(err.to_string().contains("missing inline bytes"));
    }

    #[test]
    fn build_media_send_request_uses_media_group_for_multiple_images() {
        let request = build_media_send_request(
            &OutboundMessage {
                channel: ChannelId::new("telegram"),
                account_id: "default".into(),
                to: "42".into(),
                thread_id: Some("-100123:topic:7".into()),
                text: "album".into(),
                attachments: vec![
                    OutboundAttachment {
                        media_id: frankclaw_core::types::MediaId::new(),
                        mime_type: "image/png".into(),
                        filename: Some("photo-1.png".into()),
                        url: None,
                        bytes: b"png1".to_vec(),
                    },
                    OutboundAttachment {
                        media_id: frankclaw_core::types::MediaId::new(),
                        mime_type: "image/jpeg".into(),
                        filename: Some("photo-2.jpg".into()),
                        url: None,
                        bytes: b"jpg2".to_vec(),
                    },
                ],
                reply_to: Some("77".into()),
            },
            true,
        )
        .expect("media group request should build");

        assert_eq!(request.method, "sendMediaGroup");
    }

    #[test]
    fn build_media_group_items_support_document_groups() {
        let items = build_media_group_items(
            &ChannelId::new("telegram"),
            &[
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "application/pdf".into(),
                    filename: Some("report-1.pdf".into()),
                    url: None,
                    bytes: b"%PDF-1".to_vec(),
                },
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "application/pdf".into(),
                    filename: Some("report-2.pdf".into()),
                    url: None,
                    bytes: b"%PDF-2".to_vec(),
                },
            ],
            "docs",
            true,
        )
        .expect("document media group should build");

        assert_eq!(items[0]["type"], serde_json::json!("document"));
        assert_eq!(items[1]["type"], serde_json::json!("document"));
        assert_eq!(items[0]["caption"], serde_json::json!("docs"));
    }

    #[test]
    fn build_media_group_items_support_audio_groups() {
        let items = build_media_group_items(
            &ChannelId::new("telegram"),
            &[
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "audio/ogg".into(),
                    filename: Some("voice-1.ogg".into()),
                    url: None,
                    bytes: b"ogg1".to_vec(),
                },
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "audio/mpeg".into(),
                    filename: Some("voice-2.mp3".into()),
                    url: None,
                    bytes: b"mp3".to_vec(),
                },
            ],
            "audio",
            true,
        )
        .expect("audio media group should build");

        assert_eq!(items[0]["type"], serde_json::json!("audio"));
        assert_eq!(items[1]["type"], serde_json::json!("audio"));
    }

    #[test]
    fn build_media_group_items_rejects_mixed_document_and_audio_groups() {
        let err = build_media_group_items(
            &ChannelId::new("telegram"),
            &[
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "application/pdf".into(),
                    filename: Some("report.pdf".into()),
                    url: None,
                    bytes: b"%PDF".to_vec(),
                },
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "audio/ogg".into(),
                    filename: Some("voice.ogg".into()),
                    url: None,
                    bytes: b"ogg".to_vec(),
                },
            ],
            "album",
            true,
        )
        .expect_err("mixed document/audio group should fail");

        assert!(err
            .to_string()
            .contains("all photos/videos, all audio, or all documents"));
    }

    #[test]
    fn build_media_group_items_rejects_more_than_ten_attachments() {
        let attachments: Vec<_> = (0..11)
            .map(|index| OutboundAttachment {
                media_id: frankclaw_core::types::MediaId::new(),
                mime_type: "image/png".into(),
                filename: Some(format!("photo-{index}.png")),
                url: None,
                bytes: b"png".to_vec(),
            })
            .collect();
        let err = build_media_group_items(&ChannelId::new("telegram"), &attachments, "album", true)
            .expect_err("more than 10 telegram media group items should fail");

        assert!(err.to_string().contains("between 2 and 10"));
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

    #[test]
    fn build_delete_body_uses_thread_target_chat_id() {
        let body = build_delete_body(
            &DeleteMessageTarget {
                account_id: "default".into(),
                to: "42".into(),
                thread_id: Some("-100123:topic:7".into()),
                platform_message_id: "99".into(),
            },
        )
        .expect("delete body should build");

        assert_eq!(body["chat_id"], serde_json::json!("-100123"));
        assert_eq!(body["message_id"], serde_json::json!(99));
    }

    #[test]
    fn parse_message_uses_caption_and_collects_media_attachments() {
        let channel = TelegramChannel::new(SecretString::from("token".to_string())).expect("channel should build");
        let inbound = channel
            .parse_message(&serde_json::json!({
                "message_id": 100,
                "date": 1_700_000_000,
                "caption": "look at this",
                "chat": {
                    "id": 42,
                    "type": "private"
                },
                "from": {
                    "id": 7,
                    "first_name": "User"
                },
                "photo": [
                    { "file_size": 512 },
                    { "file_size": 1024 }
                ]
            }))
            .expect("message should parse");

        assert_eq!(inbound.text.as_deref(), Some("look at this"));
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].mime_type, "image/jpeg");
        assert_eq!(inbound.attachments[0].size_bytes, Some(1024));
    }

    #[test]
    fn parse_message_falls_back_to_media_placeholder_without_text() {
        let channel = TelegramChannel::new(SecretString::from("token".to_string())).expect("channel should build");
        let inbound = channel
            .parse_message(&serde_json::json!({
                "message_id": 101,
                "date": 1_700_000_000,
                "chat": {
                    "id": 42,
                    "type": "private"
                },
                "from": {
                    "id": 7,
                    "first_name": "User"
                },
                "voice": {
                    "file_size": 2048
                }
            }))
            .expect("message should parse");

        assert_eq!(inbound.text.as_deref(), Some("<media:audio>"));
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].mime_type, "audio/ogg");
    }

    #[test]
    fn parse_message_matches_contract_fixture_shape() {
        let channel = TelegramChannel::new(SecretString::from("token".to_string())).expect("channel should build");
        let inbound = channel
            .parse_message(&fixture("message_with_photo"))
            .expect("fixture should parse");

        assert_eq!(inbound.channel.as_str(), "telegram");
        assert_eq!(inbound.thread_id.as_deref(), Some("-100123:topic:7"));
        assert_eq!(inbound.text.as_deref(), Some("look"));
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].mime_type, "image/jpeg");
        assert_eq!(inbound.attachments[0].size_bytes, Some(1024));
    }

    // --- Audit regression tests ---

    #[test]
    fn is_dm_chat_id_positive_ids_are_dms() {
        assert!(is_dm_chat_id("42"));
        assert!(is_dm_chat_id("123456789"));
    }

    #[test]
    fn is_dm_chat_id_negative_ids_are_groups() {
        assert!(!is_dm_chat_id("-100123"));
        assert!(!is_dm_chat_id("-42"));
    }

    #[test]
    fn is_dm_chat_id_non_numeric_returns_false() {
        assert!(!is_dm_chat_id("abc"));
        assert!(!is_dm_chat_id(""));
    }

    #[test]
    fn error_classification_helpers_match_telegram_error_strings() {
        assert!(is_parse_error("Bad Request: can't parse entities: some detail"));
        assert!(!is_parse_error("Bad Request: chat not found"));

        assert!(is_message_not_modified(
            "Bad Request: message is not modified: specified new message content and reply markup are exactly the same"
        ));
        assert!(!is_message_not_modified("Bad Request: chat not found"));

        assert!(is_thread_not_found("Bad Request: message thread not found"));
        assert!(!is_thread_not_found("Bad Request: chat not found"));
    }

    #[test]
    fn caption_limit_constant_matches_telegram_spec() {
        assert_eq!(TELEGRAM_CAPTION_LIMIT, 1024);
    }

    #[test]
    fn build_media_send_request_omits_parse_mode_when_disabled() {
        // When include_parse_mode is false, the form should not contain parse_mode.
        // We can't inspect Form fields directly, but we can verify it builds without error.
        let request = build_media_send_request(
            &OutboundMessage {
                channel: ChannelId::new("telegram"),
                account_id: "default".into(),
                to: "42".into(),
                thread_id: None,
                text: "caption".into(),
                attachments: vec![OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    url: None,
                    bytes: b"png".to_vec(),
                }],
                reply_to: None,
            },
            false,
        )
        .expect("media send request should build without parse_mode");

        assert_eq!(request.method, "sendPhoto");
    }

    #[test]
    fn build_media_group_items_omit_parse_mode_when_disabled() {
        let items = build_media_group_items(
            &ChannelId::new("telegram"),
            &[
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("a.png".into()),
                    url: None,
                    bytes: b"png1".to_vec(),
                },
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("b.png".into()),
                    url: None,
                    bytes: b"png2".to_vec(),
                },
            ],
            "caption text",
            false,
        )
        .expect("media group items should build");

        // First item should have caption but no parse_mode.
        assert_eq!(items[0]["caption"], serde_json::json!("caption text"));
        assert!(items[0].get("parse_mode").is_none());
    }

    #[test]
    fn build_media_group_items_include_parse_mode_when_enabled() {
        let items = build_media_group_items(
            &ChannelId::new("telegram"),
            &[
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("a.png".into()),
                    url: None,
                    bytes: b"png1".to_vec(),
                },
                OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("b.png".into()),
                    url: None,
                    bytes: b"png2".to_vec(),
                },
            ],
            "caption text",
            true,
        )
        .expect("media group items should build");

        assert_eq!(items[0]["parse_mode"], serde_json::json!("Markdown"));
    }

    #[test]
    fn send_retry_state_initial_includes_everything() {
        let state = SendRetryState::initial();
        assert!(state.include_parse_mode);
        assert!(state.include_thread_id);
    }
}
