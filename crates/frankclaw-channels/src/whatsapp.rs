use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use tracing::info;

use frankclaw_core::channel::*;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

use crate::media_text::text_or_attachment_placeholder;

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
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");

        Self {
            access_token,
            phone_number_id: phone_number_id.trim().to_string(),
            verify_token,
            app_secret,
            client,
        }
    }

    pub fn verify_token_matches(&self, candidate: &str) -> bool {
        candidate.trim() == self.verify_token.expose_secret()
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

    fn health_endpoint(&self) -> String {
        format!("{WHATSAPP_GRAPH_BASE}/{}", self.phone_number_id)
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
        let resp = self
            .client
            .post(self.messages_endpoint())
            .header("authorization", self.auth_header())
            .json(&build_send_body(&msg))
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("whatsapp send failed: {e}"),
            })?;

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
        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid whatsapp send response: {e}"),
        })?;

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

        Ok(SendResult::Failed {
            reason: body["error"]["message"]
                .as_str()
                .unwrap_or("unknown whatsapp send failure")
                .to_string(),
        })
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
                let attachments = build_inbound_attachments(message);
                let text = text_or_attachment_placeholder(
                    message["text"]["body"]
                        .as_str()
                        .or_else(|| message["image"]["caption"].as_str())
                        .or_else(|| message["video"]["caption"].as_str())
                        .or_else(|| message["document"]["caption"].as_str()),
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
            mime_type: payload["mime_type"]
                .as_str()
                .unwrap_or(mime_fallback)
                .to_string(),
            filename: payload["filename"].as_str().map(str::to_string),
            size_bytes: None,
            url: None,
        });
    }

    attachments
}

pub fn build_send_body(msg: &OutboundMessage) -> serde_json::Value {
    let mut body = serde_json::json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": msg.to,
        "type": "text",
        "text": {
            "body": msg.text,
            "preview_url": false
        }
    });

    if let Some(reply_to) = msg.reply_to.as_deref() {
        body["context"] = serde_json::json!({ "message_id": reply_to });
    }

    body
}

fn parse_unix_timestamp(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    value
        .parse::<i64>()
        .ok()
        .and_then(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0))
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let value = value.trim();
    if value.len() % 2 != 0 {
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
    fn verify_signature_accepts_valid_prefixed_header() {
        let channel = WhatsAppChannel::new(
            SecretString::from("access-token".to_string()),
            "12345".into(),
            SecretString::from("verify-token".to_string()),
            Some(SecretString::from("app-secret".to_string())),
        );
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
        );
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
}
