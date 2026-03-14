use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use tracing::info;

use frankclaw_core::channel::*;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

use crate::media_text::{normalize_mime_type, text_or_attachment_placeholder};
use crate::outbound_media::{
    AttachmentKind, attachment_bytes, attachment_filename, attachment_kind,
    require_single_attachment,
};
use crate::outbound_text::{normalize_outbound_text, OutboundTextFlavor};

const WHATSAPP_GRAPH_BASE: &str = "https://graph.facebook.com/v19.0";
type HmacSha256 = Hmac<Sha256>;

pub struct WhatsAppChannel {
    access_token: SecretString,
    phone_number_id: String,
    verify_token: SecretString,
    app_secret: Option<SecretString>,
    client: Client,
}

impl WhatsAppChannel {
    pub fn new(
        access_token: SecretString,
        phone_number_id: String,
        verify_token: SecretString,
        app_secret: Option<SecretString>,
    ) -> Result<Self> {
        let client = crate::build_channel_http_client()?;

        Ok(Self {
            access_token,
            phone_number_id: phone_number_id.trim().to_string(),
            verify_token,
            app_secret,
            client,
        })
    }

    pub fn verify_token_matches(&self, candidate: &str) -> bool {
        frankclaw_crypto::verify_token_eq(candidate.trim(), self.verify_token.expose_secret())
    }

    pub fn verify_signature(&self, body: &[u8], signature_header: Option<&str>) -> Result<()> {
        let Some(secret) = &self.app_secret else {
            return Ok(());
        };
        let signature = signature_header.ok_or(FrankClawError::AuthRequired)?;
        let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
        let provided = decode_hex(signature).ok_or(FrankClawError::AuthFailed)?;

        let mut mac = HmacSha256::new_from_slice(secret.expose_secret().as_bytes()).map_err(|_| {
            FrankClawError::Internal {
                msg: "failed to initialize whatsapp webhook signature verifier".into(),
            }
        })?;
        mac.update(body);
        mac.verify_slice(&provided).map_err(|_| FrankClawError::AuthFailed)
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.access_token.expose_secret())
    }

    fn messages_endpoint(&self) -> String {
        format!("{WHATSAPP_GRAPH_BASE}/{}/messages", self.phone_number_id)
    }

    fn media_endpoint(&self) -> String {
        format!("{WHATSAPP_GRAPH_BASE}/{}/media", self.phone_number_id)
    }

    fn health_endpoint(&self) -> String {
        format!("{WHATSAPP_GRAPH_BASE}/{}", self.phone_number_id)
    }

    async fn upload_media(
        &self,
        attachment: &OutboundAttachment,
    ) -> Result<String> {
        let channel = self.id();
        let bytes = attachment_bytes(&channel, attachment)?;
        let filename = attachment_filename(attachment);
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str(&attachment.mime_type)
            .map_err(|e| self.channel_err(format!("invalid attachment mime type: {e}")))?;
        let form = reqwest::multipart::Form::new()
            .text("messaging_product", "whatsapp")
            .part("file", part);
        let resp = self
            .client
            .post(self.media_endpoint())
            .header("authorization", self.auth_header())
            .multipart(form)
            .send()
            .await
            .map_err(|e| self.channel_err(format!("whatsapp media upload failed: {e}")))?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid whatsapp media upload response: {e}")))?;
        if !status.is_success() {
            return Err(self.channel_err(body["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown whatsapp media upload failure")
                    .to_string()));
        }

        body["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| self.channel_err("whatsapp media upload response missing id".into()))
    }
}

#[async_trait]
impl ChannelPlugin for WhatsAppChannel {
    fn id(&self) -> ChannelId {
        ChannelId::new("whatsapp")
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            threads: false,
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
        "WhatsApp"
    }

    async fn start(
        &self,
        _inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        info!("whatsapp channel ready (webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("whatsapp channel stopped");
        Ok(())
    }

    async fn health(&self) -> HealthStatus {
        match self
            .client
            .get(self.health_endpoint())
            .header("authorization", self.auth_header())
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthStatus::Connected,
            Ok(resp) => HealthStatus::Degraded {
                reason: format!("HTTP {}", resp.status()),
            },
            Err(err) => HealthStatus::Disconnected {
                reason: err.to_string(),
            },
        }
    }

    async fn send(&self, msg: OutboundMessage) -> Result<SendResult> {
        let body = if msg.attachments.is_empty() {
            build_send_body(&msg)
        } else {
            let attachment = require_single_attachment(&self.id(), &msg.attachments)?;
            let uploaded_media_id = self.upload_media(attachment).await?;
            build_media_send_body(&msg, attachment, &uploaded_media_id)
        };
        let resp = self
            .client
            .post(self.messages_endpoint())
            .header("authorization", self.auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| self.channel_err(format!("whatsapp send failed: {e}")))?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            return Ok(SendResult::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(|e| self.channel_err(format!("invalid whatsapp send response: {e}")))?;

        if status.is_success() {
            let message_id = body["messages"]
                .as_array()
                .and_then(|messages| messages.first())
                .and_then(|message| message["id"].as_str())
                .unwrap_or_default()
                .to_string();
            return Ok(SendResult::Sent {
                platform_message_id: message_id,
            });
        }

        let error_code = body["error"]["code"].as_u64().unwrap_or(0);
        let error_msg = body["error"]["message"]
            .as_str()
            .unwrap_or("unknown whatsapp send failure");
        let reason = classify_whatsapp_send_error(error_code, error_msg);
        Ok(SendResult::Failed { reason })
    }
}

pub fn parse_webhook_payload(payload: &serde_json::Value) -> Vec<InboundMessage> {
    let mut inbound = Vec::new();

    let Some(entries) = payload.get("entry").and_then(|value| value.as_array()) else {
        return inbound;
    };

    for entry in entries {
        let Some(changes) = entry.get("changes").and_then(|value| value.as_array()) else {
            continue;
        };
        for change in changes {
            let value = &change["value"];
            let account_id = value["metadata"]["phone_number_id"]
                .as_str()
                .unwrap_or("default")
                .to_string();
            let contact_map = value["contacts"]
                .as_array()
                .map(|contacts| {
                    contacts
                        .iter()
                        .filter_map(|contact| {
                            let wa_id = contact["wa_id"].as_str()?.to_string();
                            let name = contact["profile"]["name"].as_str().map(str::to_string);
                            Some((wa_id, name))
                        })
                        .collect::<std::collections::HashMap<_, _>>()
                })
                .unwrap_or_default();

            let Some(messages) = value.get("messages").and_then(|value| value.as_array()) else {
                continue;
            };
            for message in messages {
                let Some(sender_id) = message["from"].as_str() else {
                    continue;
                };

                // Skip non-content message types (status updates, reactions,
                // read receipts, etc.) to prevent spurious processing.
                if let Some(msg_type) = message["type"].as_str()
                    && !is_processable_message_type(msg_type) {
                        continue;
                    }

                let attachments = build_inbound_attachments(message);
                let message_text = extract_message_text(message);
                let text = text_or_attachment_placeholder(
                    message_text,
                    &attachments,
                );
                let Some(text) = text else {
                    continue;
                };
                let sender_name = contact_map
                    .get(sender_id)
                    .cloned()
                    .flatten();
                inbound.push(InboundMessage {
                    channel: ChannelId::new("whatsapp"),
                    account_id: account_id.clone(),
                    sender_id: sender_id.to_string(),
                    sender_name,
                    thread_id: None,
                    is_group: false,
                    is_mention: false,
                    text: Some(text),
                    attachments,
                    platform_message_id: message["id"].as_str().map(str::to_string),
                    timestamp: message["timestamp"]
                        .as_str()
                        .and_then(parse_unix_timestamp)
                        .unwrap_or_else(chrono::Utc::now),
                });
            }
        }
    }

    inbound
}

fn build_inbound_attachments(message: &serde_json::Value) -> Vec<InboundAttachment> {
    let mut attachments = Vec::new();

    for (key, mime_fallback) in [
        ("image", "image/jpeg"),
        ("audio", "audio/ogg"),
        ("video", "video/mp4"),
        ("document", "application/octet-stream"),
        ("sticker", "image/webp"),
    ] {
        let payload = &message[key];
        if payload.is_null() {
            continue;
        }

        attachments.push(InboundAttachment {
            media_id: None,
            mime_type: normalize_mime_type(
                payload["mime_type"]
                    .as_str()
                    .unwrap_or(mime_fallback),
            ).clone(),
            filename: payload["filename"].as_str().map(str::to_string),
            size_bytes: None,
            url: None,
        });
    }

    attachments
}

fn extract_message_text(message: &serde_json::Value) -> Option<&str> {
    message["text"]["body"]
        .as_str()
        .or_else(|| message["image"]["caption"].as_str())
        .or_else(|| message["video"]["caption"].as_str())
        .or_else(|| message["document"]["caption"].as_str())
        .or_else(|| message["button"]["text"].as_str())
        .or_else(|| message["interactive"]["button_reply"]["title"].as_str())
        .or_else(|| message["interactive"]["list_reply"]["title"].as_str())
        .or_else(|| message["interactive"]["list_reply"]["description"].as_str())
}

pub fn build_send_body(msg: &OutboundMessage) -> serde_json::Value {
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::WhatsApp);
    let mut body = serde_json::json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": msg.to,
        "type": "text",
        "text": {
            "body": text,
            "preview_url": false
        }
    });

    if let Some(reply_to) = msg.reply_to.as_deref() {
        body["context"] = serde_json::json!({ "message_id": reply_to });
    }

    body
}

fn build_media_send_body(
    msg: &OutboundMessage,
    attachment: &OutboundAttachment,
    uploaded_media_id: &str,
) -> serde_json::Value {
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::WhatsApp);
    let mut body = serde_json::json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": msg.to,
    });

    match attachment_kind(&attachment.mime_type) {
        AttachmentKind::Image => {
            body["type"] = serde_json::json!("image");
            body["image"] = serde_json::json!({
                "id": uploaded_media_id,
            });
            if !text.is_empty() {
                body["image"]["caption"] = serde_json::json!(text);
            }
        }
        AttachmentKind::Video => {
            body["type"] = serde_json::json!("video");
            body["video"] = serde_json::json!({
                "id": uploaded_media_id,
            });
            if !text.is_empty() {
                body["video"]["caption"] = serde_json::json!(text);
            }
        }
        AttachmentKind::Audio => {
            body["type"] = serde_json::json!("audio");
            body["audio"] = serde_json::json!({
                "id": uploaded_media_id,
            });
        }
        AttachmentKind::Document => {
            body["type"] = serde_json::json!("document");
            body["document"] = serde_json::json!({
                "id": uploaded_media_id,
                "filename": attachment_filename(attachment),
            });
            if !text.is_empty() {
                body["document"]["caption"] = serde_json::json!(text);
            }
        }
    }

    if let Some(reply_to) = msg.reply_to.as_deref() {
        body["context"] = serde_json::json!({ "message_id": reply_to });
    }

    body
}

/// Message types that contain user content worth processing.
/// Excludes: reaction, status, system, ephemeral, order, unknown, etc.
fn is_processable_message_type(msg_type: &str) -> bool {
    matches!(
        msg_type,
        "text" | "image" | "video" | "audio" | "document" | "sticker"
            | "interactive" | "button" | "contacts" | "location"
    )
}

/// Provide human-readable error messages for common WhatsApp API errors.
fn classify_whatsapp_send_error(code: u64, message: &str) -> String {
    match code {
        131030 => "recipient phone number is not a WhatsApp user".into(),
        131031 => "recipient cannot receive messages (blocked or opt-out)".into(),
        131047 => "message failed to send (re-engagement required)".into(),
        131051 => "unsupported message type for this recipient".into(),
        130429 => "rate limit reached for this phone number".into(),
        _ => message.to_string(),
    }
}

fn parse_unix_timestamp(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    value
        .parse::<i64>()
        .ok()
        .and_then(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0))
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let value = value.trim();
    if !value.len().is_multiple_of(2) {
        return None;
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars = value.as_bytes();
    for index in (0..chars.len()).step_by(2) {
        let hi = decode_nibble(chars[index])?;
        let lo = decode_nibble(chars[index + 1])?;
        bytes.push((hi << 4) | lo);
    }
    Some(bytes)
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    fn fixture(name: &str) -> serde_json::Value {
        match name {
            "media_webhook" => serde_json::from_str(include_str!(
                "fixture_whatsapp_media_webhook.json"
            ))
            .expect("fixture should parse"),
            _ => panic!("unknown fixture: {name}"),
        }
    }

    #[test]
    fn parse_webhook_payload_extracts_text_messages() {
        let payload = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "metadata": {
                            "phone_number_id": "12345"
                        },
                        "contacts": [{
                            "wa_id": "15551234567",
                            "profile": {
                                "name": "Alice"
                            }
                        }],
                        "messages": [{
                            "from": "15551234567",
                            "id": "wamid.1",
                            "timestamp": "1710000000",
                            "type": "text",
                            "text": {
                                "body": "hello"
                            }
                        }]
                    }
                }]
            }]
        });

        let messages = parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].account_id, "12345");
        assert_eq!(messages[0].sender_id, "15551234567");
        assert_eq!(messages[0].sender_name.as_deref(), Some("Alice"));
        assert_eq!(messages[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_webhook_payload_extracts_media_messages_without_text_body() {
        let payload = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "metadata": {
                            "phone_number_id": "12345"
                        },
                        "messages": [{
                            "from": "15551234567",
                            "id": "wamid.2",
                            "timestamp": "1710000001",
                            "type": "image",
                            "image": {
                                "mime_type": "image/png"
                            }
                        }]
                    }
                }]
            }]
        });

        let messages = parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text.as_deref(), Some("<media:image>"));
        assert_eq!(messages[0].attachments.len(), 1);
        assert_eq!(messages[0].attachments[0].mime_type, "image/png");
    }

    #[test]
    fn parse_webhook_payload_extracts_interactive_reply_titles() {
        let payload = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "metadata": {
                            "phone_number_id": "12345"
                        },
                        "messages": [{
                            "from": "15551234567",
                            "id": "wamid.3",
                            "timestamp": "1710000002",
                            "type": "interactive",
                            "interactive": {
                                "button_reply": {
                                    "id": "btn-1",
                                    "title": "Yes, continue"
                                }
                            }
                        }]
                    }
                }]
            }]
        });

        let messages = parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text.as_deref(), Some("Yes, continue"));
    }

    #[test]
    fn parse_webhook_payload_normalizes_parameterized_audio_mime() {
        let payload = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "metadata": {
                            "phone_number_id": "12345"
                        },
                        "messages": [{
                            "from": "15551234567",
                            "id": "wamid.4",
                            "timestamp": "1710000003",
                            "type": "audio",
                            "audio": {
                                "mime_type": "Audio/Ogg; codecs=opus"
                            }
                        }]
                    }
                }]
            }]
        });

        let messages = parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text.as_deref(), Some("<media:audio>"));
        assert_eq!(messages[0].attachments[0].mime_type, "audio/ogg");
    }

    #[test]
    fn parse_webhook_payload_matches_contract_fixture_shape() {
        let mut payload = fixture("media_webhook");
        payload["entry"][0]["changes"][0]["value"]["metadata"] = serde_json::json!({
            "phone_number_id": "12345"
        });

        let messages = parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].channel.as_str(), "whatsapp");
        assert_eq!(messages[0].text.as_deref(), Some("<media:image>"));
        assert_eq!(messages[0].attachments[0].mime_type, "image/jpeg");
    }

    #[test]
    fn build_send_body_includes_reply_context() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("whatsapp"),
            account_id: "12345".into(),
            to: "15551234567".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: Some("wamid.1".into()),
        });

        assert_eq!(body["to"], serde_json::json!("15551234567"));
        assert_eq!(body["text"]["body"], serde_json::json!("hello"));
        assert_eq!(body["context"]["message_id"], serde_json::json!("wamid.1"));
    }

    #[test]
    fn build_send_body_normalizes_whatsapp_text_formatting() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("whatsapp"),
            account_id: "12345".into(),
            to: "15551234567".into(),
            thread_id: None,
            text: "\n**bold** and ~~strike~~\n".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["text"]["body"], serde_json::json!("*bold* and ~strike~"));
    }

    #[test]
    fn build_send_body_drops_reasoning_preamble_when_final_text_is_present() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("whatsapp"),
            account_id: "12345".into(),
            to: "15551234567".into(),
            thread_id: None,
            text: "Reasoning:\n- private notes\n\nVisible answer".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["text"]["body"], serde_json::json!("Visible answer"));
    }

    #[test]
    fn build_media_send_body_uses_document_payload() {
        let body = build_media_send_body(
            &OutboundMessage {
                channel: ChannelId::new("whatsapp"),
                account_id: "12345".into(),
                to: "15551234567".into(),
                thread_id: None,
                text: "See attached".into(),
                attachments: Vec::new(),
                reply_to: Some("wamid.1".into()),
            },
            &OutboundAttachment {
                media_id: frankclaw_core::types::MediaId::new(),
                mime_type: "application/pdf".into(),
                filename: Some("report.pdf".into()),
                url: None,
                bytes: b"%PDF".to_vec(),
            },
            "media-1",
        );

        assert_eq!(body["type"], serde_json::json!("document"));
        assert_eq!(body["document"]["id"], serde_json::json!("media-1"));
        assert_eq!(body["document"]["filename"], serde_json::json!("report.pdf"));
        assert_eq!(body["document"]["caption"], serde_json::json!("See attached"));
        assert_eq!(body["context"]["message_id"], serde_json::json!("wamid.1"));
    }

    #[test]
    fn build_media_send_body_omits_caption_for_audio() {
        let body = build_media_send_body(
            &OutboundMessage {
                channel: ChannelId::new("whatsapp"),
                account_id: "12345".into(),
                to: "15551234567".into(),
                thread_id: None,
                text: "ignored".into(),
                attachments: Vec::new(),
                reply_to: None,
            },
            &OutboundAttachment {
                media_id: frankclaw_core::types::MediaId::new(),
                mime_type: "audio/ogg".into(),
                filename: Some("voice.ogg".into()),
                url: None,
                bytes: b"OggS".to_vec(),
            },
            "media-2",
        );

        assert_eq!(body["type"], serde_json::json!("audio"));
        assert_eq!(body["audio"]["id"], serde_json::json!("media-2"));
        assert!(body["audio"]["caption"].is_null());
    }

    #[test]
    fn verify_signature_accepts_valid_prefixed_header() {
        let channel = WhatsAppChannel::new(
            SecretString::from("access-token".to_string()),
            "12345".into(),
            SecretString::from("verify-token".to_string()),
            Some(SecretString::from("app-secret".to_string())),
        ).expect("channel should build");
        let body = br#"{"entry":[{"id":"1"}]}"#;

        let mut mac =
            HmacSha256::new_from_slice(b"app-secret").expect("hmac should initialize");
        mac.update(body);
        let bytes = mac.finalize().into_bytes();
        let signature = format!(
            "sha256={}",
            bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        );

        channel
            .verify_signature(body, Some(&signature))
            .expect("signature should verify");
    }

    #[test]
    fn verify_signature_rejects_missing_or_invalid_headers_when_secret_is_configured() {
        let channel = WhatsAppChannel::new(
            SecretString::from("access-token".to_string()),
            "12345".into(),
            SecretString::from("verify-token".to_string()),
            Some(SecretString::from("app-secret".to_string())),
        ).expect("channel should build");
        let body = br#"{"entry":[{"id":"1"}]}"#;

        assert!(matches!(
            channel.verify_signature(body, None),
            Err(FrankClawError::AuthRequired)
        ));
        assert!(matches!(
            channel.verify_signature(body, Some("sha256=deadbeef")),
            Err(FrankClawError::AuthFailed)
        ));
    }

    // --- Audit regression tests ---

    #[test]
    fn is_processable_message_type_accepts_content_types() {
        for t in ["text", "image", "video", "audio", "document", "sticker", "interactive", "button"] {
            assert!(is_processable_message_type(t), "should accept {t}");
        }
    }

    #[test]
    fn is_processable_message_type_rejects_non_content_types() {
        for t in ["reaction", "status", "system", "ephemeral", "order", "unknown"] {
            assert!(!is_processable_message_type(t), "should reject {t}");
        }
    }

    #[test]
    fn parse_webhook_payload_skips_reaction_messages() {
        let payload = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "metadata": { "phone_number_id": "12345" },
                        "contacts": [{ "wa_id": "15551234567", "profile": { "name": "User" } }],
                        "messages": [
                            {
                                "from": "15551234567",
                                "type": "reaction",
                                "id": "wamid.reaction1",
                                "timestamp": "1700000000",
                                "reaction": { "message_id": "wamid.1", "emoji": "👍" }
                            },
                            {
                                "from": "15551234567",
                                "type": "text",
                                "id": "wamid.text1",
                                "timestamp": "1700000000",
                                "text": { "body": "hello" }
                            }
                        ]
                    }
                }]
            }]
        });

        let messages = parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn classify_whatsapp_send_error_provides_helpful_messages() {
        assert!(classify_whatsapp_send_error(131030, "").contains("not a WhatsApp user"));
        assert!(classify_whatsapp_send_error(131031, "").contains("blocked"));
    }

    #[test]
    fn classify_whatsapp_send_error_passes_through_unknown_codes() {
        assert_eq!(
            classify_whatsapp_send_error(0, "some error"),
            "some error"
        );
    }
}
