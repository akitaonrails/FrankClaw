use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use tracing::{info, warn};

use frankclaw_core::channel::*;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

const SIGNAL_API_PATH_CHECK: &str = "/api/v1/check";
const SIGNAL_API_PATH_EVENTS: &str = "/api/v1/events";
const SIGNAL_API_PATH_RPC: &str = "/api/v1/rpc";

pub struct SignalChannel {
    base_url: String,
    account: Option<String>,
    client: Client,
}

impl SignalChannel {
    pub fn new(base_url: String, account: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");

        Self {
            base_url: normalize_base_url(&base_url),
            account: account.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
            client,
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn events_url(&self) -> Result<url::Url> {
        let mut url = url::Url::parse(&self.endpoint(SIGNAL_API_PATH_EVENTS)).map_err(|e| {
            FrankClawError::Channel {
                channel: self.id(),
                msg: format!("invalid signal events url: {e}"),
            }
        })?;
        if let Some(account) = self.account.as_deref() {
            url.query_pairs_mut().append_pair("account", account);
        }
        Ok(url)
    }

    async fn run_event_stream(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        let resp = self
            .client
            .get(self.events_url()?)
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("signal event stream connect failed: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(FrankClawError::Channel {
                channel: self.id(),
                msg: format!("signal event stream returned HTTP {}", resp.status()),
            });
        }

        let mut parser = SignalSseParser::default();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("signal event stream read failed: {e}"),
            })?;
            let text = std::str::from_utf8(&chunk).map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("signal event stream sent invalid UTF-8: {e}"),
            })?;
            for event in parser.push(text) {
                if let Some(inbound) = parse_receive_event(&event, self.account.as_deref()) {
                    if inbound_tx.send(inbound).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }

        if let Some(event) = parser.finish() {
            if let Some(inbound) = parse_receive_event(&event, self.account.as_deref()) {
                if inbound_tx.send(inbound).await.is_err() {
                    return Ok(());
                }
            }
        }

        Err(FrankClawError::Channel {
            channel: self.id(),
            msg: "signal event stream closed".into(),
        })
    }
}

#[async_trait]
impl ChannelPlugin for SignalChannel {
    fn id(&self) -> ChannelId {
        ChannelId::new("signal")
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
        "Signal"
    }

    async fn start(
        &self,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        info!("signal channel starting (SSE mode)");
        loop {
            match self.run_event_stream(inbound_tx.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    warn!(error = %err, "signal event stream error, retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn stop(&self) -> Result<()> {
        info!("signal channel stopped");
        Ok(())
    }

    async fn health(&self) -> HealthStatus {
        match self.client.get(self.endpoint(SIGNAL_API_PATH_CHECK)).send().await {
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
        let body = build_send_request(&msg, self.account.as_deref());
        let resp = self
            .client
            .post(self.endpoint(SIGNAL_API_PATH_RPC))
            .json(&body)
            .send()
            .await
            .map_err(|e| FrankClawError::Channel {
                channel: self.id(),
                msg: format!("signal send failed: {e}"),
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
        if status == reqwest::StatusCode::CREATED {
            return Ok(SendResult::Sent {
                platform_message_id: "unknown".into(),
            });
        }

        let rpc: SignalRpcResponse = resp.json().await.map_err(|e| FrankClawError::Channel {
            channel: self.id(),
            msg: format!("invalid signal send response: {e}"),
        })?;

        if let Some(error) = rpc.error {
            return Ok(SendResult::Failed {
                reason: error.message.unwrap_or_else(|| format!("Signal RPC error {}", error.code.unwrap_or_default())),
            });
        }

        if !status.is_success() {
            return Ok(SendResult::Failed {
                reason: format!("HTTP {status}"),
            });
        }

        Ok(SendResult::Sent {
            platform_message_id: rpc
                .result
                .and_then(|result| result.timestamp.map(|timestamp| timestamp.to_string()))
                .unwrap_or_else(|| "unknown".into()),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SignalSseEvent {
    event: Option<String>,
    data: Option<String>,
    id: Option<String>,
}

#[derive(Default)]
struct SignalSseParser {
    buffer: String,
    current: SignalSseEvent,
}

impl SignalSseParser {
    fn push(&mut self, chunk: &str) -> Vec<SignalSseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(line_end) = self.buffer.find('\n') {
            let mut line = self.buffer.drain(..=line_end).collect::<String>();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                if let Some(event) = self.take_current() {
                    events.push(event);
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }

            let (field, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.current.event = Some(value.to_string()),
                "data" => match &mut self.current.data {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(value);
                    }
                    None => self.current.data = Some(value.to_string()),
                },
                "id" => self.current.id = Some(value.to_string()),
                _ => {}
            }
        }

        events
    }

    fn finish(&mut self) -> Option<SignalSseEvent> {
        if !self.buffer.trim().is_empty() {
            self.push("\n\n");
        }
        self.take_current()
    }

    fn take_current(&mut self) -> Option<SignalSseEvent> {
        let event = std::mem::take(&mut self.current);
        if event.event.is_none() && event.data.is_none() && event.id.is_none() {
            None
        } else {
            Some(event)
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct SignalReceivePayload {
    envelope: Option<SignalEnvelope>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalEnvelope {
    #[serde(rename = "sourceNumber")]
    source_number: Option<String>,
    #[serde(rename = "sourceUuid")]
    source_uuid: Option<String>,
    #[serde(rename = "sourceName")]
    source_name: Option<String>,
    timestamp: Option<i64>,
    #[serde(rename = "dataMessage")]
    data_message: Option<SignalDataMessage>,
    #[serde(rename = "editMessage")]
    edit_message: Option<SignalEditMessage>,
    #[serde(rename = "syncMessage")]
    sync_message: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalEditMessage {
    #[serde(rename = "dataMessage")]
    data_message: Option<SignalDataMessage>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalDataMessage {
    timestamp: Option<i64>,
    message: Option<String>,
    attachments: Option<Vec<SignalAttachmentPayload>>,
    mentions: Option<Vec<SignalMention>>,
    #[serde(rename = "groupInfo")]
    group_info: Option<SignalGroupInfo>,
    quote: Option<SignalQuote>,
    reaction: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalAttachmentPayload {
    #[serde(rename = "contentType")]
    content_type: Option<String>,
    filename: Option<String>,
    size: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalMention {
    name: Option<String>,
    number: Option<String>,
    uuid: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalGroupInfo {
    #[serde(rename = "groupId")]
    group_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalQuote {
    text: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalRpcResponse {
    result: Option<SignalSendResult>,
    error: Option<SignalRpcError>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalSendResult {
    timestamp: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalRpcError {
    code: Option<i64>,
    message: Option<String>,
}

fn parse_receive_event(
    event: &SignalSseEvent,
    configured_account: Option<&str>,
) -> Option<InboundMessage> {
    let data = event.data.as_deref()?;
    let payload: SignalReceivePayload = serde_json::from_str(data).ok()?;
    let envelope = payload.envelope?;
    if envelope.sync_message.is_some() {
        return None;
    }

    let data_message = envelope
        .data_message
        .or_else(|| envelope.edit_message.and_then(|edit| edit.data_message))?;
    if data_message.reaction.is_some()
        && data_message
            .message
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        && data_message
            .attachments
            .as_ref()
            .map(|attachments| attachments.is_empty())
            .unwrap_or(true)
    {
        return None;
    }

    let sender_id = envelope
        .source_number
        .clone()
        .or(envelope.source_uuid.clone())?;
    let group_id = data_message
        .group_info
        .as_ref()
        .and_then(|group| group.group_id.as_deref())
        .map(str::to_string);
    let is_group = group_id.is_some();
    let attachments = build_inbound_attachments(data_message.attachments.as_deref());
    let text = message_or_placeholder(
        data_message.message.as_deref(),
        data_message.quote.as_ref().and_then(|quote| quote.text.as_deref()),
        &attachments,
    )?;
    let timestamp = envelope
        .timestamp
        .or(data_message.timestamp)
        .and_then(timestamp_millis)
        .unwrap_or_else(chrono::Utc::now);

    Some(InboundMessage {
        channel: ChannelId::new("signal"),
        account_id: "default".into(),
        sender_id,
        sender_name: envelope.source_name,
        thread_id: group_id.map(|group_id| format!("group:{group_id}")),
        is_group,
        is_mention: detect_group_mention(data_message.mentions.as_deref(), configured_account),
        text: Some(text),
        attachments,
        platform_message_id: envelope.timestamp.map(|timestamp| timestamp.to_string()),
        timestamp,
    })
}

fn build_inbound_attachments(
    attachments: Option<&[SignalAttachmentPayload]>,
) -> Vec<InboundAttachment> {
    attachments
        .unwrap_or(&[])
        .iter()
        .map(|attachment| InboundAttachment {
            media_id: None,
            mime_type: attachment
                .content_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".into()),
            filename: attachment.filename.clone(),
            size_bytes: attachment.size,
            url: None,
        })
        .collect()
}

fn message_or_placeholder(
    message: Option<&str>,
    quote: Option<&str>,
    attachments: &[InboundAttachment],
) -> Option<String> {
    let text = message
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if text.is_some() {
        return text;
    }

    if !attachments.is_empty() {
        return Some(attachment_placeholder(attachments));
    }

    quote
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn attachment_placeholder(attachments: &[InboundAttachment]) -> String {
    if attachments.len() > 1 {
        return "<media:attachments>".into();
    }

    let mime = attachments
        .first()
        .map(|attachment| attachment.mime_type.as_str())
        .unwrap_or("application/octet-stream");
    if mime.starts_with("image/") {
        "<media:image>".into()
    } else if mime.starts_with("audio/") {
        "<media:audio>".into()
    } else if mime.starts_with("video/") {
        "<media:video>".into()
    } else {
        "<media:attachment>".into()
    }
}

fn detect_group_mention(mentions: Option<&[SignalMention]>, configured_account: Option<&str>) -> bool {
    let Some(mentions) = mentions else {
        return false;
    };
    if mentions.is_empty() {
        return false;
    }

    let Some(configured_account) = configured_account.map(normalize_signal_identity) else {
        return true;
    };

    mentions.iter().any(|mention| {
        mention
            .number
            .as_deref()
            .map(normalize_signal_identity)
            .filter(|value| !value.is_empty())
            .as_deref()
            == Some(configured_account.as_str())
            || mention
                .uuid
                .as_deref()
                .map(normalize_signal_identity)
                .filter(|value| !value.is_empty())
                .as_deref()
                == Some(configured_account.as_str())
            || mention
                .name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                == Some(configured_account.as_str())
    })
}

fn build_send_request(msg: &OutboundMessage, account: Option<&str>) -> serde_json::Value {
    let mut params = serde_json::json!({
        "message": msg.text,
    });

    if let Some(account) = account {
        params["account"] = serde_json::json!(account);
    }

    match resolve_signal_target(msg.thread_id.as_deref(), &msg.to) {
        SignalTarget::Recipient(recipient) => {
            params["recipient"] = serde_json::json!([recipient]);
        }
        SignalTarget::Group(group_id) => {
            params["groupId"] = serde_json::json!(group_id);
        }
        SignalTarget::Username(username) => {
            params["username"] = serde_json::json!([username]);
        }
    }

    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "send",
        "params": params,
        "id": uuid::Uuid::new_v4().to_string(),
    })
}

enum SignalTarget {
    Recipient(String),
    Group(String),
    Username(String),
}

fn resolve_signal_target(thread_id: Option<&str>, to: &str) -> SignalTarget {
    let raw = thread_id.unwrap_or(to).trim();
    let raw = raw.strip_prefix("signal:").unwrap_or(raw);

    if let Some(group_id) = raw.strip_prefix("group:") {
        return SignalTarget::Group(group_id.trim().to_string());
    }
    if let Some(username) = raw.strip_prefix("username:") {
        return SignalTarget::Username(username.trim().to_string());
    }
    if let Some(username) = raw.strip_prefix("u:") {
        return SignalTarget::Username(username.trim().to_string());
    }

    SignalTarget::Recipient(raw.to_string())
}

fn normalize_base_url(value: &str) -> String {
    let trimmed = value.trim();
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

fn normalize_signal_identity(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("signal:")
        .trim()
        .to_string()
}

fn timestamp_millis(value: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp_millis(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_merges_multiline_events() {
        let mut parser = SignalSseParser::default();
        let events = parser.push("event: message\ndata: {\"a\":1}\n");
        assert!(events.is_empty());

        let events = parser.push("data: {\"b\":2}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data.as_deref(), Some("{\"a\":1}\n{\"b\":2}"));
    }

    #[test]
    fn parse_receive_event_builds_group_message_and_attachment_placeholder() {
        let inbound = parse_receive_event(
            &SignalSseEvent {
                event: Some("message".into()),
                data: Some(
                    serde_json::json!({
                        "envelope": {
                            "sourceNumber": "+15550001111",
                            "sourceName": "Alice",
                            "timestamp": 1710000000123_i64,
                            "dataMessage": {
                                "attachments": [
                                    {
                                        "contentType": "image/png",
                                        "filename": "photo.png",
                                        "size": 42
                                    }
                                ],
                                "mentions": [
                                    { "number": "+15551234567" }
                                ],
                                "groupInfo": {
                                    "groupId": "group-42"
                                }
                            }
                        }
                    })
                    .to_string(),
                ),
                id: None,
            },
            Some("+15551234567"),
        )
        .expect("signal inbound should parse");

        assert!(inbound.is_group);
        assert!(inbound.is_mention);
        assert_eq!(inbound.thread_id.as_deref(), Some("group:group-42"));
        assert_eq!(inbound.text.as_deref(), Some("<media:image>"));
        assert_eq!(inbound.attachments.len(), 1);
    }

    #[test]
    fn parse_receive_event_skips_reaction_only_payloads() {
        let inbound = parse_receive_event(
            &SignalSseEvent {
                event: Some("message".into()),
                data: Some(
                    serde_json::json!({
                        "envelope": {
                            "sourceNumber": "+15550001111",
                            "timestamp": 1710000000123_i64,
                            "dataMessage": {
                                "reaction": {
                                    "emoji": "👍"
                                }
                            }
                        }
                    })
                    .to_string(),
                ),
                id: None,
            },
            Some("+15551234567"),
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn build_send_request_uses_group_target_from_thread() {
        let body = build_send_request(
            &OutboundMessage {
                channel: ChannelId::new("signal"),
                account_id: "default".into(),
                to: "+15550001111".into(),
                thread_id: Some("group:group-42".into()),
                text: "hello".into(),
                attachments: Vec::new(),
                reply_to: None,
            },
            Some("+15551234567"),
        );

        assert_eq!(body["method"], serde_json::json!("send"));
        assert_eq!(body["params"]["groupId"], serde_json::json!("group-42"));
        assert_eq!(body["params"]["account"], serde_json::json!("+15551234567"));
        assert_eq!(body["params"]["message"], serde_json::json!("hello"));
    }
}
