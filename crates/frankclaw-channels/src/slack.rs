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

use crate::media_text::text_or_attachment_placeholder;
use crate::outbound_text::{normalize_outbound_text, OutboundTextFlavor};

const SLACK_API_BASE: &str = "https://slack.com/api";

pub struct SlackChannel {
    app_token: SecretString,
    bot_token: SecretString,
    client: Client,
    bot_user_id: Mutex<Option<String>>,
}

impl SlackChannel {
    pub fn new(app_token: SecretString, bot_token: SecretString) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");

        Self {
            app_token,
            bot_token,
            client,
            bot_user_id: Mutex::new(None),
        }
    }

    fn app_auth_header(&self) -> String {
        format!("Bearer {}", self.app_token.expose_secret())
    }

    fn bot_auth_header(&self) -> String {
        format!("Bearer {}", self.bot_token.expose_secret())
    }

    async fn socket_mode_url(&self) -> Result<String> {
        let resp = self
            .client
            .post(format!("{SLACK_API_BASE}/apps.connections.open"))
            .header("authorization", self.app_auth_header())
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("socket mode connection open failed: {e}"),
            })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid socket mode response: {e}"),
        })?;
        if body["ok"].as_bool() != Some(true) {
            return Err(FrankClawError::Channel {
                channel: self.id(),
                msg: body["error"]
                    .as_str()
                    .unwrap_or("slack socket mode request failed")
                    .to_string(),
            });
        }

        body["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| FrankClawError::Channel {
                channel: self.id(),
                msg: "slack socket mode response missing url".into(),
            })
    }

    async fn populate_bot_user_id(&self) -> Result<()> {
        let resp = self
            .client
            .post(format!("{SLACK_API_BASE}/auth.test"))
            .header("authorization", self.bot_auth_header())
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("slack auth test failed: {e}"),
            })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid slack auth test response: {e}"),
        })?;
        if body["ok"].as_bool() != Some(true) {
            return Err(FrankClawError::Channel {
                channel: self.id(),
                msg: body["error"]
                    .as_str()
                    .unwrap_or("slack auth test failed")
                    .to_string(),
            });
        }

        *self.bot_user_id.lock().await = body["user_id"].as_str().map(str::to_string);
        Ok(())
    }

    async fn run_socket_mode(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        self.populate_bot_user_id().await?;
        let socket_url = self.socket_mode_url().await?;
        let (socket, _) = tokio_tungstenite::connect_async(socket_url)
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("slack socket mode connect failed: {e}"),
            })?;
        let (mut ws_tx, mut ws_rx) = socket.split();

        while let Some(frame) = ws_rx.next().await {
            let frame = frame.map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("slack socket mode read failed: {e}"),
            })?;
            let payload = parse_socket_frame(self.id(), frame)?;

            if let Some(envelope_id) = payload["envelope_id"].as_str() {
                ws_tx
                    .send(Message::Text(
                        serde_json::json!({ "envelope_id": envelope_id })
                            .to_string()
                            .into(),
                    ))
                    .await
                    .map_err(|e| FrankClawError::Channel {
                        channel: self.id(),
                        msg: format!("slack socket mode ack failed: {e}"),
                    })?;
            }

            if payload["type"].as_str() != Some("events_api") {
                continue;
            }

            let bot_user_id = self.bot_user_id.lock().await.clone();
            if let Some(inbound) = parse_event_message(&payload["payload"]["event"], bot_user_id.as_deref()) {
                if inbound_tx.send(inbound).await.is_err() {
                    return Ok(());
                }
            }
        }

        Err(FrankClawError::Channel {
            channel: self.id(),
            msg: "slack socket mode closed".into(),
        })
    }
}

#[async_trait]
impl ChannelPlugin for SlackChannel {
    fn id(&self) -> ChannelId {
        ChannelId::new("slack")
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
        "Slack"
    }

    async fn start(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        info!("slack channel starting (socket mode)");
        loop {
            match self.run_socket_mode(inbound_tx.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    warn!(error = %err, "slack socket mode error, retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn stop(&self) -> Result<()> {
        info!("slack channel stopped");
        Ok(())
    }

    async fn health(&self) -> HealthStatus {
        match self
            .client
            .post(format!("{SLACK_API_BASE}/auth.test"))
            .header("authorization", self.bot_auth_header())
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
        let resp = self
            .client
            .post(format!("{SLACK_API_BASE}/chat.postMessage"))
            .header("authorization", self.bot_auth_header())
            .json(&build_send_body(&msg))
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("slack send failed: {e}"),
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

        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid slack send response: {e}"),
        })?;
        if body["ok"].as_bool() == Some(true) {
            Ok(SendResult::Sent {
                platform_message_id: body["ts"].as_str().unwrap_or_default().to_string(),
            })
        } else {
            Ok(SendResult::Failed {
                reason: body["error"]
                    .as_str()
                    .unwrap_or("unknown slack send failure")
                    .to_string(),
            })
        }
    }

    async fn edit_message(&self, target: &EditMessageTarget, new_text: &str) -> Result<()> {
        let resp = self
            .client
            .post(format!("{SLACK_API_BASE}/chat.update"))
            .header("authorization", self.bot_auth_header())
            .json(&build_edit_body(target, new_text))
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("slack edit failed: {e}"),
            })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid slack edit response: {e}"),
        })?;
        if body["ok"].as_bool() == Some(true) {
            Ok(())
        } else {
            Err(FrankClawError::Channel {
                channel: self.id(),
                msg: body["error"]
                    .as_str()
                    .unwrap_or("unknown slack edit failure")
                    .to_string(),
            })
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
            SendResult::Failed { reason } => Err(FrankClawError::Channel {
                channel: self.id(),
                msg: reason,
            }),
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
        let resp = self
            .client
            .post(format!("{SLACK_API_BASE}/chat.delete"))
            .header("authorization", self.bot_auth_header())
            .json(&build_delete_body(target))
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("slack delete failed: {e}"),
            })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid slack delete response: {e}"),
        })?;
        if body["ok"].as_bool() == Some(true) {
            Ok(())
        } else {
            Err(FrankClawError::Channel {
                channel: self.id(),
                msg: body["error"]
                    .as_str()
                    .unwrap_or("unknown slack delete failure")
                    .to_string(),
            })
        }
    }
}

fn parse_socket_frame(channel_id: ChannelId, frame: Message) -> Result<serde_json::Value> {
    let text = match frame {
        Message::Text(text) => text,
        Message::Binary(bytes) => String::from_utf8(bytes.to_vec())
            .map_err(|e| FrankClawError::Channel {
                channel: channel_id.clone(),
                msg: format!("slack socket mode sent invalid UTF-8: {e}"),
            })?
            .into(),
        Message::Close(_) => {
            return Err(FrankClawError::Channel {
                channel: channel_id,
                msg: "slack socket mode closed".into(),
            });
        }
        _ => {
            return Err(FrankClawError::Channel {
                channel: channel_id,
                msg: "slack socket mode sent unexpected frame type".into(),
            });
        }
    };

    serde_json::from_str(text.as_ref()).map_err(|e| FrankClawError::Channel {
        channel: channel_id,
        msg: format!("slack socket mode sent invalid JSON: {e}"),
    })
}

fn parse_event_message(event: &serde_json::Value, bot_user_id: Option<&str>) -> Option<InboundMessage> {
    if event["type"].as_str() != Some("message") {
        return None;
    }
    if event["subtype"].is_string() || event["bot_id"].is_string() {
        return None;
    }

    let channel_id = event["channel"].as_str()?.to_string();
    let sender_id = event["user"].as_str()?.to_string();
    let attachments = event["files"]
        .as_array()
        .map(|files| {
            files.iter()
                .map(|file| InboundAttachment {
                    media_id: None,
                    mime_type: file["mimetype"]
                        .as_str()
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    filename: file["name"].as_str().map(str::to_string),
                    size_bytes: file["size"].as_u64(),
                    url: file["url_private"].as_str().map(str::to_string),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let text = text_or_attachment_placeholder(event["text"].as_str(), &attachments);
    let thread_target = encode_thread_target(&channel_id, event["thread_ts"].as_str());
    let channel_type = event["channel_type"].as_str().unwrap_or("channel");
    let is_group = channel_type != "im";
    let is_mention = bot_user_id
        .map(|user_id| {
            text.as_deref()
                .map(|text| text.contains(&format!("<@{user_id}>")))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let timestamp = event["ts"]
        .as_str()
        .and_then(parse_slack_timestamp)
        .unwrap_or_else(chrono::Utc::now);

    Some(InboundMessage {
        channel: ChannelId::new("slack"),
        account_id: "default".to_string(),
        sender_id,
        sender_name: None,
        thread_id: Some(thread_target),
        is_group,
        is_mention,
        text,
        attachments,
        platform_message_id: event["ts"].as_str().map(str::to_string),
        timestamp,
    })
}

fn build_send_body(msg: &OutboundMessage) -> serde_json::Value {
    let (channel, thread_ts) = parse_thread_target(msg.thread_id.as_deref(), &msg.to);
    let thread_ts = thread_ts.or_else(|| msg.reply_to.clone());
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
    let mut body = serde_json::json!({
        "channel": channel,
        "text": text,
        "reply_broadcast": false,
    });
    if let Some(thread_ts) = thread_ts {
        body["thread_ts"] = serde_json::json!(thread_ts);
    }
    body
}

fn build_edit_body(target: &EditMessageTarget, new_text: &str) -> serde_json::Value {
    let (channel, _) = parse_thread_target(target.thread_id.as_deref(), &target.to);
    let text = normalize_outbound_text(new_text, OutboundTextFlavor::Plain);
    serde_json::json!({
        "channel": channel,
        "ts": target.platform_message_id,
        "text": text,
    })
}

fn build_delete_body(target: &DeleteMessageTarget) -> serde_json::Value {
    let (channel, _) = parse_thread_target(target.thread_id.as_deref(), &target.to);
    serde_json::json!({
        "channel": channel,
        "ts": target.platform_message_id,
    })
}

fn encode_thread_target(channel_id: &str, thread_ts: Option<&str>) -> String {
    match thread_ts {
        Some(thread_ts) => format!("{channel_id}:thread:{thread_ts}"),
        None => channel_id.to_string(),
    }
}

fn parse_thread_target(thread_id: Option<&str>, fallback_to: &str) -> (String, Option<String>) {
    let raw = thread_id.unwrap_or(fallback_to);
    if let Some((channel_id, thread_ts)) = raw.split_once(":thread:") {
        return (channel_id.to_string(), Some(thread_ts.to_string()));
    }
    (raw.to_string(), None)
}

fn parse_slack_timestamp(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let seconds = value.split('.').next()?.parse::<i64>().ok()?;
    chrono::DateTime::from_timestamp(seconds, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        match name {
            "event_message_with_file" => serde_json::from_str(include_str!(
                "fixture_slack_event_message_with_file.json"
            ))
            .expect("fixture should parse"),
            _ => panic!("unknown fixture: {name}"),
        }
    }

    #[test]
    fn parse_event_message_detects_group_mentions_and_threads() {
        let inbound = parse_event_message(
            &serde_json::json!({
                "type": "message",
                "channel": "C123",
                "channel_type": "channel",
                "user": "U123",
                "text": "<@UBOT> hello",
                "ts": "1710000000.123456",
                "thread_ts": "1710000000.000001"
            }),
            Some("UBOT"),
        )
        .expect("message should parse");

        assert!(inbound.is_group);
        assert!(inbound.is_mention);
        assert_eq!(
            inbound.thread_id.as_deref(),
            Some("C123:thread:1710000000.000001")
        );
    }

    #[test]
    fn parse_event_message_skips_bot_messages() {
        let inbound = parse_event_message(
            &serde_json::json!({
                "type": "message",
                "channel": "C123",
                "user": "U123",
                "bot_id": "B123",
                "text": "hello",
                "ts": "1710000000.123456"
            }),
            Some("UBOT"),
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn parse_event_message_collects_files_and_uses_media_placeholder() {
        let inbound = parse_event_message(
            &serde_json::json!({
                "type": "message",
                "channel": "C123",
                "channel_type": "channel",
                "user": "U123",
                "ts": "1710000000.123456",
                "files": [{
                    "name": "report.pdf",
                    "mimetype": "application/pdf",
                    "size": 2048,
                    "url_private": "https://files.example/report.pdf"
                }]
            }),
            None,
        )
        .expect("message should parse");

        assert_eq!(inbound.text.as_deref(), Some("<media:attachment>"));
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].filename.as_deref(), Some("report.pdf"));
        assert_eq!(
            inbound.attachments[0].url.as_deref(),
            Some("https://files.example/report.pdf")
        );
    }

    #[test]
    fn parse_event_message_matches_contract_fixture_shape() {
        let inbound = parse_event_message(&fixture("event_message_with_file"), Some("UBOT"))
            .expect("fixture should parse");

        assert_eq!(inbound.channel.as_str(), "slack");
        assert_eq!(inbound.thread_id.as_deref(), Some("C123"));
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].mime_type, "application/pdf");
        assert_eq!(
            inbound.attachments[0].url.as_deref(),
            Some("https://files.example/report.pdf")
        );
    }

    #[test]
    fn build_send_body_includes_thread_ts_when_target_is_thread() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("slack"),
            account_id: "default".into(),
            to: "C123".into(),
            thread_id: Some("C123:thread:1710000000.000001".into()),
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["channel"], serde_json::json!("C123"));
        assert_eq!(body["thread_ts"], serde_json::json!("1710000000.000001"));
    }

    #[test]
    fn build_send_body_uses_reply_to_as_thread_anchor_when_not_already_threaded() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("slack"),
            account_id: "default".into(),
            to: "C123".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: Some("1710000000.123456".into()),
        });

        assert_eq!(body["channel"], serde_json::json!("C123"));
        assert_eq!(body["thread_ts"], serde_json::json!("1710000000.123456"));
        assert_eq!(body["reply_broadcast"], serde_json::json!(false));
    }

    #[test]
    fn build_send_body_trims_plain_outbound_text() {
        let body = build_send_body(&OutboundMessage {
            channel: ChannelId::new("slack"),
            account_id: "default".into(),
            to: "C123".into(),
            thread_id: None,
            text: "\n hello \r\n".into(),
            attachments: Vec::new(),
            reply_to: None,
        });

        assert_eq!(body["text"], serde_json::json!("hello"));
    }

    #[test]
    fn build_edit_body_uses_channel_from_thread_target() {
        let body = build_edit_body(
            &EditMessageTarget {
                account_id: "default".into(),
                to: "C123".into(),
                thread_id: Some("C123:thread:1710000000.000001".into()),
                platform_message_id: "1710000000.123456".into(),
            },
            "updated",
        );

        assert_eq!(body["channel"], serde_json::json!("C123"));
        assert_eq!(body["ts"], serde_json::json!("1710000000.123456"));
        assert_eq!(body["text"], serde_json::json!("updated"));
    }

    #[test]
    fn build_delete_body_uses_channel_from_thread_target() {
        let body = build_delete_body(&DeleteMessageTarget {
            account_id: "default".into(),
            to: "C123".into(),
            thread_id: Some("C123:thread:1710000000.000001".into()),
            platform_message_id: "1710000000.123456".into(),
        });

        assert_eq!(body["channel"], serde_json::json!("C123"));
        assert_eq!(body["ts"], serde_json::json!("1710000000.123456"));
    }
}
