use async_trait::async_trait;
use base64::Engine as _;
use futures_util::StreamExt;
use reqwest::Client;
use tracing::{info, warn};

use frankclaw_core::channel::*;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

use crate::inbound_media::infer_inbound_mime_type;
use crate::media_text::text_quote_or_attachment_placeholder;
use crate::outbound_media::attachment_bytes;
use crate::outbound_text::{normalize_outbound_text, OutboundTextFlavor};

const SIGNAL_API_PATH_CHECK: &str = "/api/v1/check";
const SIGNAL_API_PATH_EVENTS: &str = "/api/v1/events";
const SIGNAL_API_PATH_RPC: &str = "/api/v1/rpc";
const SIGNAL_API_PATH_SEND: &str = "/v2/send";

pub struct SignalChannel {
    base_url: String,
    account: Option<String>,
    client: Client,
}

impl SignalChannel {
    pub fn new(base_url: String, account: Option<String>) -> Result<Self> {
        let client = crate::build_channel_http_client()?;

        Ok(Self {
            base_url: normalize_base_url(&base_url),
            account: account.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
            client,
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn events_url(&self) -> Result<url::Url> {
        let mut url = url::Url::parse(&self.endpoint(SIGNAL_API_PATH_EVENTS)).map_err(|e| self.channel_err(format!("invalid signal events url: {e}")))?;
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
            .map_err(|e| self.channel_err(format!("signal event stream connect failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(self.channel_err(format!("signal event stream returned HTTP {}", resp.status())));
        }

        let mut parser = SignalSseParser::default();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| self.channel_err(format!("signal event stream read failed: {e}")))?;
            let text = std::str::from_utf8(&chunk).map_err(|e| self.channel_err(format!("signal event stream sent invalid UTF-8: {e}")))?;
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

        Err(self.channel_err("signal event stream closed".into()))
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
        let (endpoint, body) = if msg.attachments.is_empty() {
            (self.endpoint(SIGNAL_API_PATH_RPC), build_send_request(&msg, self.account.as_deref())?)
        } else {
            (self.endpoint(SIGNAL_API_PATH_SEND), build_send_attachment_request(&msg, self.account.as_deref())?)
        };
        let resp = self
            .client
            .post(endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| self.channel_err(format!("signal send failed: {e}")))?;

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

        let rpc: SignalRpcResponse = resp.json().await.map_err(|e| self.channel_err(format!("invalid signal send response: {e}")))?;

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

    // Self-message echo prevention: skip messages from our own account
    if is_self_message(
        envelope.source_number.as_deref(),
        envelope.source_uuid.as_deref(),
        configured_account,
    ) {
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
    // Replace Object Replacement Characters (U+FFFC) with @mention names
    let message_text = data_message
        .message
        .as_deref()
        .map(|text| replace_mention_orc(text, data_message.mentions.as_deref()));
    let text = text_quote_or_attachment_placeholder(
        message_text.as_deref(),
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
            mime_type: infer_inbound_mime_type(
                attachment.content_type.as_deref(),
                attachment.filename.as_deref(),
                None,
            ),
            filename: attachment.filename.clone(),
            size_bytes: attachment.size,
            url: None,
        })
        .collect()
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

fn build_send_request(msg: &OutboundMessage, account: Option<&str>) -> Result<serde_json::Value> {
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
    let mut params = serde_json::json!({
        "message": text,
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

    Ok(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "send",
        "params": params,
        "id": uuid::Uuid::new_v4().to_string(),
    }))
}

fn build_send_attachment_request(
    msg: &OutboundMessage,
    account: Option<&str>,
) -> Result<serde_json::Value> {
    let number = account.ok_or_else(|| FrankClawError::Channel {
        channel: ChannelId::new("signal"),
        msg: "signal attachment sends require a configured account number".into(),
    })?;
    let text = normalize_outbound_text(&msg.text, OutboundTextFlavor::Plain);
    let attachments = msg
        .attachments
        .iter()
        .map(|attachment| {
            attachment_bytes(&ChannelId::new("signal"), attachment).map(|bytes| {
                base64::engine::general_purpose::STANDARD.encode(bytes)
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut body = serde_json::json!({
        "message": text,
        "number": number,
        "base64_attachments": attachments,
    });

    match resolve_signal_target(msg.thread_id.as_deref(), &msg.to) {
        SignalTarget::Recipient(recipient) => {
            body["recipients"] = serde_json::json!([recipient]);
        }
        SignalTarget::Group(group_id) => {
            body["recipients"] = serde_json::json!([format!("group.{group_id}")]);
        }
        SignalTarget::Username(username) => {
            return Err(FrankClawError::Channel {
                channel: ChannelId::new("signal"),
                msg: format!("signal attachment sends do not support username target '{username}'"),
            });
        }
    }

    Ok(body)
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
    let stripped = value.trim().trim_start_matches("signal:").trim();
    // If it looks like a phone number (starts with + or digits), normalize to E.164
    if stripped.starts_with('+') || stripped.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        normalize_e164(stripped)
    } else {
        // UUID or other identifier — lowercase for case-insensitive comparison
        stripped.to_lowercase()
    }
}

/// Normalize a phone number to E.164 format: strip non-digits, re-add leading `+`.
fn normalize_e164(value: &str) -> String {
    let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return String::new();
    }
    format!("+{digits}")
}

/// Check if a sender identity matches the configured account (self-message detection).
/// Compares both phone number and UUID against the configured account value.
fn is_self_message(
    source_number: Option<&str>,
    source_uuid: Option<&str>,
    configured_account: Option<&str>,
) -> bool {
    let Some(account) = configured_account else {
        return false;
    };
    let normalized_account = normalize_signal_identity(account);
    if normalized_account.is_empty() {
        return false;
    }

    if let Some(number) = source_number {
        let normalized = normalize_signal_identity(number);
        if !normalized.is_empty() && normalized == normalized_account {
            return true;
        }
    }
    if let Some(uuid) = source_uuid {
        let normalized = normalize_signal_identity(uuid);
        if !normalized.is_empty() && normalized == normalized_account {
            return true;
        }
    }
    false
}

/// Replace U+FFFC (Object Replacement Character) placeholders with `@name` strings.
/// Signal encodes mentions as ORC characters with a parallel metadata array.
fn replace_mention_orc(text: &str, mentions: Option<&[SignalMention]>) -> String {
    let mentions = mentions.unwrap_or(&[]);
    if mentions.is_empty() || !text.contains('\u{FFFC}') {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut mention_idx = 0;
    for ch in text.chars() {
        if ch == '\u{FFFC}' {
            if let Some(mention) = mentions.get(mention_idx) {
                let name = mention
                    .name
                    .as_deref()
                    .or(mention.number.as_deref())
                    .unwrap_or("someone");
                result.push('@');
                result.push_str(name);
            } else {
                result.push(ch);
            }
            mention_idx += 1;
        } else {
            result.push(ch);
        }
    }
    result
}

fn timestamp_millis(value: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp_millis(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        match name {
            "receive_attachment" => serde_json::from_str(include_str!(
                "fixture_signal_receive_attachment.json"
            ))
            .expect("fixture should parse"),
            _ => panic!("unknown fixture: {name}"),
        }
    }

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
    fn build_inbound_attachments_infers_mime_type_from_filename() {
        let attachments = build_inbound_attachments(Some(&[SignalAttachmentPayload {
            content_type: None,
            filename: Some("voice-note.ogg".into()),
            size: Some(321),
        }]));

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].mime_type, "audio/ogg");
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
        )
        .expect("send request should build");

        assert_eq!(body["method"], serde_json::json!("send"));
        assert_eq!(body["params"]["groupId"], serde_json::json!("group-42"));
        assert_eq!(body["params"]["account"], serde_json::json!("+15551234567"));
        assert_eq!(body["params"]["message"], serde_json::json!("hello"));
    }

    #[test]
    fn build_send_request_trims_plain_outbound_text() {
        let body = build_send_request(
            &OutboundMessage {
                channel: ChannelId::new("signal"),
                account_id: "default".into(),
                to: "+15550001111".into(),
                thread_id: None,
                text: "\n hello \r\n".into(),
                attachments: Vec::new(),
                reply_to: None,
            },
            Some("+15551234567"),
        )
        .expect("send request should build");

        assert_eq!(body["params"]["message"], serde_json::json!("hello"));
    }

    #[test]
    fn build_send_attachment_request_uses_v2_shape_for_recipients() {
        let body = build_send_attachment_request(
            &OutboundMessage {
                channel: ChannelId::new("signal"),
                account_id: "default".into(),
                to: "+15550001111".into(),
                thread_id: None,
                text: "hello".into(),
                attachments: vec![OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    url: None,
                    bytes: b"png".to_vec(),
                }],
                reply_to: None,
            },
            Some("+15551234567"),
        )
        .expect("attachment request should build");

        assert_eq!(body["number"], serde_json::json!("+15551234567"));
        assert_eq!(body["recipients"], serde_json::json!(["+15550001111"]));
        assert_eq!(body["message"], serde_json::json!("hello"));
        assert_eq!(body["base64_attachments"][0], serde_json::json!("cG5n"));
    }

    #[test]
    fn build_send_attachment_request_uses_group_recipient_marker() {
        let body = build_send_attachment_request(
            &OutboundMessage {
                channel: ChannelId::new("signal"),
                account_id: "default".into(),
                to: "+15550001111".into(),
                thread_id: Some("group:group-42".into()),
                text: "hello".into(),
                attachments: vec![OutboundAttachment {
                    media_id: frankclaw_core::types::MediaId::new(),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    url: None,
                    bytes: b"png".to_vec(),
                }],
                reply_to: None,
            },
            Some("+15551234567"),
        )
        .expect("attachment request should build");

        assert_eq!(body["recipients"], serde_json::json!(["group.group-42"]));
    }

    #[test]
    fn build_send_attachment_request_supports_multiple_attachments() {
        let body = build_send_attachment_request(
            &OutboundMessage {
                channel: ChannelId::new("signal"),
                account_id: "default".into(),
                to: "+15550001111".into(),
                thread_id: None,
                text: "hello".into(),
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
                reply_to: None,
            },
            Some("+15551234567"),
        )
        .expect("attachment request should build");

        assert_eq!(
            body["base64_attachments"],
            serde_json::json!(["cG5n", "JVBERg=="])
        );
    }

    // --- Self-message echo prevention ---

    #[test]
    fn parse_receive_event_skips_self_messages_by_phone() {
        let inbound = parse_receive_event(
            &SignalSseEvent {
                event: Some("message".into()),
                data: Some(
                    serde_json::json!({
                        "envelope": {
                            "sourceNumber": "+15551234567",
                            "sourceName": "Me",
                            "timestamp": 1710000000123_i64,
                            "dataMessage": {
                                "message": "echo"
                            }
                        }
                    })
                    .to_string(),
                ),
                id: None,
            },
            Some("+15551234567"),
        );

        assert!(inbound.is_none(), "should skip self-message by phone match");
    }

    #[test]
    fn parse_receive_event_skips_self_messages_by_uuid() {
        let inbound = parse_receive_event(
            &SignalSseEvent {
                event: Some("message".into()),
                data: Some(
                    serde_json::json!({
                        "envelope": {
                            "sourceUuid": "abc-def-123",
                            "sourceName": "Me",
                            "timestamp": 1710000000123_i64,
                            "dataMessage": {
                                "message": "echo"
                            }
                        }
                    })
                    .to_string(),
                ),
                id: None,
            },
            Some("abc-def-123"),
        );

        assert!(inbound.is_none(), "should skip self-message by UUID match");
    }

    // --- E.164 normalization ---

    #[test]
    fn normalize_e164_strips_formatting() {
        assert_eq!(normalize_e164("+1 (555) 123-4567"), "+15551234567");
        assert_eq!(normalize_e164("1.555.123.4567"), "+15551234567");
        assert_eq!(normalize_e164("+15551234567"), "+15551234567");
    }

    #[test]
    fn normalize_signal_identity_handles_phone_and_uuid() {
        // Phone numbers get E.164 normalization
        assert_eq!(normalize_signal_identity("+1 555 123 4567"), "+15551234567");
        assert_eq!(normalize_signal_identity("signal:+15551234567"), "+15551234567");

        // UUIDs get lowercased
        assert_eq!(
            normalize_signal_identity("ABC-DEF-123"),
            "abc-def-123"
        );
    }

    #[test]
    fn self_message_matches_with_formatting_differences() {
        // Phone with spaces vs clean
        assert!(is_self_message(
            Some("+1 555 123 4567"),
            None,
            Some("+15551234567"),
        ));
        // UUID case mismatch
        assert!(is_self_message(
            None,
            Some("ABC-DEF-123"),
            Some("abc-def-123"),
        ));
        // Different sender
        assert!(!is_self_message(
            Some("+15559999999"),
            None,
            Some("+15551234567"),
        ));
        // No configured account — can't detect self
        assert!(!is_self_message(
            Some("+15551234567"),
            None,
            None,
        ));
    }

    // --- Mention ORC replacement ---

    #[test]
    fn replace_mention_orc_substitutes_names() {
        let text = "Hello \u{FFFC} and \u{FFFC}!";
        let mentions = vec![
            SignalMention {
                name: Some("Alice".into()),
                number: Some("+15550001111".into()),
                uuid: None,
            },
            SignalMention {
                name: Some("Bob".into()),
                number: None,
                uuid: None,
            },
        ];
        let result = replace_mention_orc(text, Some(&mentions));
        assert_eq!(result, "Hello @Alice and @Bob!");
    }

    #[test]
    fn replace_mention_orc_falls_back_to_number() {
        let text = "\u{FFFC} said hi";
        let mentions = vec![SignalMention {
            name: None,
            number: Some("+15550001111".into()),
            uuid: None,
        }];
        let result = replace_mention_orc(text, Some(&mentions));
        assert_eq!(result, "@+15550001111 said hi");
    }

    #[test]
    fn replace_mention_orc_no_mentions_returns_unchanged() {
        let text = "no mentions here";
        assert_eq!(replace_mention_orc(text, None), "no mentions here");
        assert_eq!(replace_mention_orc(text, Some(&[])), "no mentions here");
    }

    #[test]
    fn parse_receive_event_replaces_orc_in_message_text() {
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
                                "message": "Hey \u{FFFC} check this",
                                "mentions": [
                                    { "name": "Bot", "number": "+15551234567" }
                                ]
                            }
                        }
                    })
                    .to_string(),
                ),
                id: None,
            },
            Some("+15559999999"),
        )
        .expect("should parse with ORC replacement");

        assert_eq!(inbound.text.as_deref(), Some("Hey @Bot check this"));
    }

    #[test]
    fn parse_receive_event_matches_contract_fixture_shape() {
        let inbound = parse_receive_event(
            &SignalSseEvent {
                event: Some("message".into()),
                data: Some(fixture("receive_attachment").to_string()),
                id: None,
            },
            Some("+15551234567"),
        )
        .expect("fixture should parse");

        assert_eq!(inbound.channel.as_str(), "signal");
        assert_eq!(inbound.sender_id, "+15550001111");
        assert_eq!(inbound.thread_id.as_deref(), Some("group:group-42"));
        assert_eq!(inbound.text.as_deref(), Some("photo"));
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].mime_type, "image/png");
    }
}
