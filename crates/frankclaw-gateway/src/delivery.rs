use std::sync::Arc;

use frankclaw_core::channel::{ChannelPlugin, OutboundMessage, SendResult};
use frankclaw_core::error::Result;
use serde::{Deserialize, Serialize};

use crate::audit::{log_event, log_failure};

const MAX_OUTBOUND_ATTEMPTS: usize = 3;
const MAX_RETRY_DELAY_SECS: u64 = 30;
const PSEUDO_STREAM_MIN_CHARS: usize = 240;
#[cfg(test)]
const PSEUDO_STREAM_STEP_DELAY_MS: u64 = 0;
#[cfg(not(test))]
const PSEUDO_STREAM_STEP_DELAY_MS: u64 = 75;

#[derive(Clone, Debug)]
pub(crate) struct DeliveryChunkRecord {
    pub(crate) text: String,
    pub(crate) status: &'static str,
    pub(crate) platform_message_id: Option<String>,
    pub(crate) attempts: usize,
    pub(crate) retry_after_secs: Option<u64>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeliveryRecord {
    pub(crate) status: &'static str,
    pub(crate) platform_message_id: Option<String>,
    pub(crate) attempts: usize,
    pub(crate) retry_after_secs: Option<u64>,
    pub(crate) error: Option<String>,
    pub(crate) chunks: Vec<DeliveryChunkRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredReplyChunk {
    pub content: String,
    pub platform_message_id: Option<String>,
    pub status: String,
    pub attempts: usize,
    pub retry_after_secs: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredReplyMetadata {
    pub channel: String,
    pub account_id: String,
    pub recipient_id: String,
    pub thread_id: Option<String>,
    pub reply_to: Option<String>,
    pub content: String,
    pub platform_message_id: Option<String>,
    pub status: String,
    pub attempts: usize,
    pub retry_after_secs: Option<u64>,
    pub error: Option<String>,
    #[serde(default)]
    pub chunks: Vec<StoredReplyChunk>,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

pub(crate) async fn deliver_outbound_message(
    channel: Arc<dyn ChannelPlugin>,
    outbound: OutboundMessage,
) -> Result<DeliveryRecord> {
    let capabilities = channel.capabilities();
    let outbound_chunks = split_outbound_message(&outbound);
    let chunk_count = outbound_chunks.len();
    let mut chunks = Vec::with_capacity(outbound_chunks.len());
    let mut total_attempts = 0usize;
    let mut last_platform_message_id = None;
    let mut last_retry_after = None;

    for (index, chunk) in outbound_chunks.into_iter().enumerate() {
        let chunk_record = if should_pseudo_stream(&capabilities, &chunk) {
            send_streamed_chunk(channel.clone(), chunk.clone(), index + 1, chunk_count).await?
        } else {
            send_outbound_chunk(channel.clone(), chunk.clone(), index + 1, chunk_count).await?
        };
        total_attempts += chunk_record.attempts;
        if let Some(platform_message_id) = chunk_record.platform_message_id.clone() {
            last_platform_message_id = Some(platform_message_id);
        }
        last_retry_after = chunk_record.retry_after_secs;
        let status = chunk_record.status;
        let error = chunk_record.error.clone();
        chunks.push(chunk_record);

        if status != "sent" {
            return Ok(DeliveryRecord {
                status,
                platform_message_id: last_platform_message_id,
                attempts: total_attempts,
                retry_after_secs: last_retry_after,
                error,
                chunks,
            });
        }
    }

    Ok(DeliveryRecord {
        status: "sent",
        platform_message_id: last_platform_message_id,
        attempts: total_attempts,
        retry_after_secs: last_retry_after,
        error: None,
        chunks,
    })
}

async fn send_outbound_chunk(
    channel: Arc<dyn ChannelPlugin>,
    outbound: OutboundMessage,
    chunk_index: usize,
    chunk_count: usize,
) -> Result<DeliveryChunkRecord> {
    let mut attempts = 0usize;
    let mut last_retry_after = None;
    let max_attempts = max_attempts_for_channel(outbound.channel.as_str());

    loop {
        attempts += 1;
        match channel.send(outbound.clone()).await {
            Ok(SendResult::Sent { platform_message_id }) => {
                log_event(
                    "channel.send",
                    "success",
                    serde_json::json!({
                        "channel": outbound.channel.as_str(),
                        "account_id": outbound.account_id,
                        "recipient": outbound.to,
                        "chunk_index": chunk_index,
                        "chunk_count": chunk_count,
                        "attempts": attempts,
                        "platform_message_id": platform_message_id,
                    }),
                );
                return Ok(DeliveryChunkRecord {
                    text: outbound.text,
                    status: "sent",
                    platform_message_id: Some(platform_message_id),
                    attempts,
                    retry_after_secs: last_retry_after,
                    error: None,
                });
            }
            Ok(SendResult::RateLimited { retry_after_secs }) => {
                last_retry_after = retry_after_secs;
                if attempts >= max_attempts {
                    log_failure(
                        "channel.send",
                        serde_json::json!({
                            "channel": outbound.channel.as_str(),
                            "account_id": outbound.account_id,
                            "recipient": outbound.to,
                            "chunk_index": chunk_index,
                            "chunk_count": chunk_count,
                            "attempts": attempts,
                            "reason": "rate_limited",
                            "retry_after_secs": retry_after_secs,
                        }),
                    );
                    return Ok(DeliveryChunkRecord {
                        text: outbound.text,
                        status: "rate_limited",
                        platform_message_id: None,
                        attempts,
                        retry_after_secs,
                        error: Some("rate limited".to_string()),
                    });
                }

                sleep_retry(retry_delay_secs(
                    outbound.channel.as_str(),
                    attempts,
                    retry_after_secs,
                ))
                .await;
            }
            Ok(SendResult::Failed { reason }) => {
                if attempts >= max_attempts
                    || !should_retry_send_failure(outbound.channel.as_str(), &reason)
                {
                    log_failure(
                        "channel.send",
                        serde_json::json!({
                            "channel": outbound.channel.as_str(),
                            "account_id": outbound.account_id,
                            "recipient": outbound.to,
                            "chunk_index": chunk_index,
                            "chunk_count": chunk_count,
                            "attempts": attempts,
                            "reason": reason,
                        }),
                    );
                    return Ok(DeliveryChunkRecord {
                        text: outbound.text,
                        status: "failed",
                        platform_message_id: None,
                        attempts,
                        retry_after_secs: None,
                        error: Some(reason),
                    });
                }

                sleep_retry(retry_delay_secs(outbound.channel.as_str(), attempts, None)).await;
            }
            Err(err) => {
                if attempts >= max_attempts
                    || !should_retry_send_failure(outbound.channel.as_str(), &err.to_string())
                {
                    log_failure(
                        "channel.send",
                        serde_json::json!({
                            "channel": outbound.channel.as_str(),
                            "account_id": outbound.account_id,
                            "recipient": outbound.to,
                            "chunk_index": chunk_index,
                            "chunk_count": chunk_count,
                            "attempts": attempts,
                            "reason": err.to_string(),
                        }),
                    );
                    return Err(err);
                }

                sleep_retry(retry_delay_secs(outbound.channel.as_str(), attempts, None)).await;
            }
        }
    }
}

async fn send_streamed_chunk(
    channel: Arc<dyn ChannelPlugin>,
    outbound: OutboundMessage,
    chunk_index: usize,
    chunk_count: usize,
) -> Result<DeliveryChunkRecord> {
    let steps = pseudo_stream_steps(&outbound.text);
    if steps.len() < 2 {
        return send_outbound_chunk(channel, outbound, chunk_index, chunk_count).await;
    }

    let initial = OutboundMessage {
        text: steps[0].clone(),
        ..outbound.clone()
    };
    let handle = match channel.stream_start(&initial).await {
        Ok(handle) => handle,
        Err(err) => {
            log_failure(
                "channel.stream_start",
                serde_json::json!({
                    "channel": outbound.channel.as_str(),
                    "account_id": outbound.account_id,
                    "recipient": outbound.to,
                    "chunk_index": chunk_index,
                    "chunk_count": chunk_count,
                    "reason": err.to_string(),
                }),
            );
            return send_outbound_chunk(channel, outbound, chunk_index, chunk_count).await;
        }
    };

    for step in steps.iter().take(steps.len() - 1).skip(1) {
        if let Err(err) = channel.stream_update(&handle, step).await {
            log_failure(
                "channel.stream_update",
                serde_json::json!({
                    "channel": outbound.channel.as_str(),
                    "account_id": outbound.account_id,
                    "recipient": outbound.to,
                    "chunk_index": chunk_index,
                    "chunk_count": chunk_count,
                    "reason": err.to_string(),
                }),
            );
            channel.stream_end(&handle, &outbound.text).await?;
            return Ok(DeliveryChunkRecord {
                text: outbound.text,
                status: "sent",
                platform_message_id: Some(handle.draft_message_id),
                attempts: 1,
                retry_after_secs: None,
                error: None,
            });
        }
        if PSEUDO_STREAM_STEP_DELAY_MS > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(PSEUDO_STREAM_STEP_DELAY_MS)).await;
        }
    }

    channel.stream_end(&handle, &outbound.text).await?;
    log_event(
        "channel.stream",
        "success",
        serde_json::json!({
            "channel": outbound.channel.as_str(),
            "account_id": outbound.account_id,
            "recipient": outbound.to,
            "chunk_index": chunk_index,
            "chunk_count": chunk_count,
            "updates": steps.len().saturating_sub(1),
            "platform_message_id": handle.draft_message_id,
        }),
    );
    Ok(DeliveryChunkRecord {
        text: outbound.text,
        status: "sent",
        platform_message_id: Some(handle.draft_message_id),
        attempts: 1,
        retry_after_secs: None,
        error: None,
    })
}

pub fn last_reply_from_metadata(metadata: &serde_json::Value) -> Option<StoredReplyMetadata> {
    serde_json::from_value(metadata.get("delivery")?.get("last_reply")?.clone()).ok()
}

pub fn set_last_reply_in_metadata(
    metadata: &mut serde_json::Value,
    reply: &StoredReplyMetadata,
) -> serde_json::Result<()> {
    let reply_value = serde_json::to_value(reply)?;
    match metadata {
        serde_json::Value::Object(object) => {
            let delivery = object
                .entry("delivery".to_string())
                .or_insert_with(|| serde_json::json!({}));
            match delivery {
                serde_json::Value::Object(delivery_object) => {
                    delivery_object.insert("last_reply".to_string(), reply_value);
                }
                _ => {
                    *delivery = serde_json::json!({ "last_reply": reply_value });
                }
            }
        }
        _ => {
            *metadata = serde_json::json!({
                "delivery": {
                    "last_reply": reply_value,
                }
            });
        }
    }
    Ok(())
}

fn split_outbound_message(outbound: &OutboundMessage) -> Vec<OutboundMessage> {
    let Some(max_chars) = max_chars_for_channel(outbound.channel.as_str()) else {
        return vec![outbound.clone()];
    };
    if !outbound.attachments.is_empty() || outbound.text.chars().count() <= max_chars {
        return vec![outbound.clone()];
    }

    split_text(&outbound.text, max_chars)
        .into_iter()
        .enumerate()
        .map(|(index, text)| OutboundMessage {
            text,
            reply_to: if index == 0 {
                outbound.reply_to.clone()
            } else {
                None
            },
            ..outbound.clone()
        })
        .collect()
}

fn max_chars_for_channel(channel_id: &str) -> Option<usize> {
    match channel_id {
        "discord" => Some(1900),
        "telegram" => Some(3500),
        "slack" => Some(3500),
        "signal" => Some(3000),
        _ => None,
    }
}

fn split_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut remaining = text.trim();
    let mut chunks = Vec::new();

    while !remaining.is_empty() {
        if remaining.chars().count() <= max_chars {
            chunks.push(remaining.to_string());
            break;
        }

        let mut split_at = 0usize;
        let mut last_whitespace = None;
        let mut count = 0usize;

        for (idx, ch) in remaining.char_indices() {
            if count == max_chars {
                break;
            }
            let next_idx = idx + ch.len_utf8();
            if ch.is_whitespace() {
                last_whitespace = Some(idx);
            }
            split_at = next_idx;
            count += 1;
        }

        let preferred = last_whitespace.filter(|idx| *idx > 0).unwrap_or(split_at);
        let chunk = remaining[..preferred].trim();
        let chunk = if chunk.is_empty() {
            remaining[..split_at].trim()
        } else {
            chunk
        };

        if chunk.is_empty() {
            break;
        }

        chunks.push(chunk.to_string());
        let consumed = if preferred > 0 { preferred } else { split_at };
        remaining = remaining[consumed..].trim_start();
    }

    if chunks.is_empty() {
        vec![text.to_string()]
    } else {
        chunks
    }
}

fn should_pseudo_stream(
    capabilities: &frankclaw_core::channel::ChannelCapabilities,
    outbound: &OutboundMessage,
) -> bool {
    capabilities.streaming
        && capabilities.edit
        && outbound.attachments.is_empty()
        && outbound.text.chars().count() >= PSEUDO_STREAM_MIN_CHARS
}

fn max_attempts_for_channel(channel_id: &str) -> usize {
    match channel_id {
        "slack" | "discord" | "whatsapp" => 4,
        _ => MAX_OUTBOUND_ATTEMPTS,
    }
}

fn retry_delay_secs(channel_id: &str, attempts: usize, retry_after_secs: Option<u64>) -> u64 {
    if let Some(retry_after_secs) = retry_after_secs {
        return retry_after_secs.clamp(1, MAX_RETRY_DELAY_SECS);
    }

    let base: u64 = match channel_id {
        "slack" => 2,
        "discord" => 2,
        "whatsapp" => 3,
        "signal" => 2,
        _ => 1,
    };
    let exponent = attempts.saturating_sub(1) as u32;
    base.saturating_mul(2u64.saturating_pow(exponent))
        .clamp(1, MAX_RETRY_DELAY_SECS)
}

fn should_retry_send_failure(channel_id: &str, reason: &str) -> bool {
    let normalized = reason.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }

    let permanent_markers = [
        "invalid_auth",
        "unauthorized",
        "forbidden",
        "not_in_channel",
        "channel_not_found",
        "chat not found",
        "message_not_found",
        "authentication failed",
        "auth failed",
        "invalid token",
        "permission denied",
    ];
    if permanent_markers.iter().any(|marker| normalized.contains(marker)) {
        return false;
    }

    let transient_markers = [
        "timeout",
        "temporarily_unavailable",
        "temporarily unavailable",
        "internal",
        "server error",
        "connection reset",
        "connection refused",
        "rate limit",
        "try again",
    ];
    if transient_markers.iter().any(|marker| normalized.contains(marker)) {
        return true;
    }

    !matches!(channel_id, "whatsapp" | "slack" | "discord")
}

async fn sleep_retry(delay_secs: u64) {
    if delay_secs == 0 {
        return;
    }
    #[cfg(test)]
    {
        let _ = delay_secs;
    }
    #[cfg(not(test))]
    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
}

fn pseudo_stream_steps(text: &str) -> Vec<String> {
    let text = text.trim();
    let total = text.chars().count();
    if total < PSEUDO_STREAM_MIN_CHARS {
        return vec![text.to_string()];
    }

    let mut steps = Vec::new();
    for ratio in [1usize, 2usize] {
        let target = (total * ratio) / 3;
        if let Some(candidate) = preview_slice(text, target) {
            if steps.last() != Some(&candidate) {
                steps.push(candidate);
            }
        }
    }
    if steps.last().map(|value| value.as_str()) != Some(text) {
        steps.push(text.to_string());
    }
    steps
}

fn preview_slice(text: &str, target_chars: usize) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    let mut fallback_end = text.len();
    let mut boundary_end = None;
    let mut count = 0usize;
    for (idx, ch) in text.char_indices() {
        count += 1;
        fallback_end = idx + ch.len_utf8();
        if count >= target_chars && (ch.is_whitespace() || matches!(ch, '.' | '!' | '?' | ',' | ';' | ':')) {
            boundary_end = Some(fallback_end);
            break;
        }
    }

    let end = boundary_end.unwrap_or(fallback_end);
    let candidate = text[..end].trim();
    if candidate.is_empty() || candidate == text {
        None
    } else {
        Some(candidate.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use frankclaw_core::types::ChannelId;
    use frankclaw_core::channel::{ChannelCapabilities, EditMessageTarget, StreamHandle};
    use std::collections::VecDeque;

    struct CaptureChannel {
        sent: tokio::sync::Mutex<Vec<OutboundMessage>>,
    }

    impl CaptureChannel {
        fn new() -> Self {
            Self {
                sent: tokio::sync::Mutex::new(Vec::new()),
            }
        }

        async fn drain(&self) -> Vec<OutboundMessage> {
            let mut sent = self.sent.lock().await;
            std::mem::take(&mut *sent)
        }
    }

    #[async_trait]
    impl ChannelPlugin for CaptureChannel {
        fn id(&self) -> ChannelId {
            ChannelId::new("discord")
        }

        fn capabilities(&self) -> frankclaw_core::channel::ChannelCapabilities {
            frankclaw_core::channel::ChannelCapabilities::default()
        }

        fn label(&self) -> &str {
            "Capture"
        }

        async fn start(
            &self,
            _inbound_tx: tokio::sync::mpsc::Sender<frankclaw_core::channel::InboundMessage>,
        ) -> Result<()> {
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            Ok(())
        }

        async fn health(&self) -> frankclaw_core::channel::HealthStatus {
            frankclaw_core::channel::HealthStatus::Connected
        }

        async fn send(&self, msg: OutboundMessage) -> Result<SendResult> {
            self.sent.lock().await.push(msg);
            Ok(SendResult::Sent {
                platform_message_id: format!("msg-{}", uuid::Uuid::new_v4()),
            })
        }
    }

    struct CaptureStreamingChannel {
        sent: tokio::sync::Mutex<Vec<OutboundMessage>>,
        updates: tokio::sync::Mutex<Vec<String>>,
    }

    struct SequenceChannel {
        outcomes: tokio::sync::Mutex<VecDeque<std::result::Result<SendResult, frankclaw_core::error::FrankClawError>>>,
        sent: tokio::sync::Mutex<Vec<OutboundMessage>>,
    }

    impl SequenceChannel {
        fn new(
            outcomes: Vec<std::result::Result<SendResult, frankclaw_core::error::FrankClawError>>,
        ) -> Self {
            Self {
                outcomes: tokio::sync::Mutex::new(outcomes.into()),
                sent: tokio::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ChannelPlugin for SequenceChannel {
        fn id(&self) -> ChannelId {
            ChannelId::new("slack")
        }

        fn capabilities(&self) -> ChannelCapabilities {
            ChannelCapabilities::default()
        }

        fn label(&self) -> &str {
            "Sequence"
        }

        async fn start(
            &self,
            _inbound_tx: tokio::sync::mpsc::Sender<frankclaw_core::channel::InboundMessage>,
        ) -> Result<()> {
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            Ok(())
        }

        async fn health(&self) -> frankclaw_core::channel::HealthStatus {
            frankclaw_core::channel::HealthStatus::Connected
        }

        async fn send(&self, msg: OutboundMessage) -> Result<SendResult> {
            self.sent.lock().await.push(msg);
            self.outcomes
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| Ok(SendResult::Sent {
                    platform_message_id: "done".into(),
                }))
        }
    }

    impl CaptureStreamingChannel {
        fn new() -> Self {
            Self {
                sent: tokio::sync::Mutex::new(Vec::new()),
                updates: tokio::sync::Mutex::new(Vec::new()),
            }
        }

        async fn sent(&self) -> Vec<OutboundMessage> {
            self.sent.lock().await.clone()
        }

        async fn updates(&self) -> Vec<String> {
            self.updates.lock().await.clone()
        }
    }

    #[async_trait]
    impl ChannelPlugin for CaptureStreamingChannel {
        fn id(&self) -> ChannelId {
            ChannelId::new("telegram")
        }

        fn capabilities(&self) -> ChannelCapabilities {
            ChannelCapabilities {
                edit: true,
                streaming: true,
                ..Default::default()
            }
        }

        fn label(&self) -> &str {
            "Streaming Capture"
        }

        async fn start(
            &self,
            _inbound_tx: tokio::sync::mpsc::Sender<frankclaw_core::channel::InboundMessage>,
        ) -> Result<()> {
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            Ok(())
        }

        async fn health(&self) -> frankclaw_core::channel::HealthStatus {
            frankclaw_core::channel::HealthStatus::Connected
        }

        async fn send(&self, msg: OutboundMessage) -> Result<SendResult> {
            self.sent.lock().await.push(msg);
            Ok(SendResult::Sent {
                platform_message_id: "stream-msg-1".into(),
            })
        }

        async fn edit_message(&self, _target: &EditMessageTarget, new_text: &str) -> Result<()> {
            self.updates.lock().await.push(new_text.to_string());
            Ok(())
        }

        async fn stream_start(&self, msg: &OutboundMessage) -> Result<StreamHandle> {
            let _ = self.send(msg.clone()).await?;
            Ok(StreamHandle {
                channel: self.id(),
                account_id: msg.account_id.clone(),
                to: msg.to.clone(),
                thread_id: msg.thread_id.clone(),
                draft_message_id: "stream-msg-1".into(),
            })
        }

        async fn stream_update(&self, _handle: &StreamHandle, text: &str) -> Result<()> {
            self.updates.lock().await.push(text.to_string());
            Ok(())
        }

        async fn stream_end(&self, _handle: &StreamHandle, final_text: &str) -> Result<()> {
            self.updates.lock().await.push(final_text.to_string());
            Ok(())
        }
    }

    #[test]
    fn split_text_prefers_word_boundaries() {
        let chunks = split_text("one two three four five", 9);
        assert_eq!(chunks, vec!["one two", "three", "four five"]);
    }

    #[tokio::test]
    async fn deliver_outbound_message_splits_long_discord_messages() {
        let channel = Arc::new(CaptureChannel::new());
        let outbound = OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "channel-1".into(),
            thread_id: None,
            text: "word ".repeat(600),
            attachments: Vec::new(),
            reply_to: None,
        };

        let delivery = deliver_outbound_message(channel.clone(), outbound)
            .await
            .expect("delivery should succeed");
        let sent = channel.drain().await;

        assert!(sent.len() > 1);
        assert_eq!(delivery.status, "sent");
        assert_eq!(delivery.chunks.len(), sent.len());
        assert!(sent.iter().all(|message| message.text.chars().count() <= 1900));
    }

    #[tokio::test]
    async fn deliver_outbound_message_only_replies_on_first_chunk() {
        let channel = Arc::new(CaptureChannel::new());
        let outbound = OutboundMessage {
            channel: ChannelId::new("discord"),
            account_id: "default".into(),
            to: "channel-1".into(),
            thread_id: None,
            text: "word ".repeat(600),
            attachments: Vec::new(),
            reply_to: Some("incoming-42".into()),
        };

        deliver_outbound_message(channel.clone(), outbound)
            .await
            .expect("delivery should succeed");
        let sent = channel.drain().await;

        assert!(sent.len() > 1);
        assert_eq!(sent[0].reply_to.as_deref(), Some("incoming-42"));
        assert!(sent.iter().skip(1).all(|message| message.reply_to.is_none()));
    }

    #[tokio::test]
    async fn deliver_outbound_message_uses_pseudo_streaming_when_supported() {
        let channel = Arc::new(CaptureStreamingChannel::new());
        let outbound = OutboundMessage {
            channel: ChannelId::new("telegram"),
            account_id: "default".into(),
            to: "chat-1".into(),
            thread_id: None,
            text: "This is a long reply that should be progressively updated across multiple preview boundaries so the user sees movement before the final text arrives. ".repeat(4),
            attachments: Vec::new(),
            reply_to: Some("incoming-1".into()),
        };

        let delivery = deliver_outbound_message(channel.clone(), outbound.clone())
            .await
            .expect("delivery should succeed");
        let sent = channel.sent().await;
        let updates = channel.updates().await;

        assert_eq!(delivery.status, "sent");
        assert_eq!(sent.len(), 1);
        assert!(sent[0].text.chars().count() < outbound.text.chars().count());
        assert!(updates.len() >= 2);
        assert_eq!(updates.last().map(String::as_str), Some(outbound.text.as_str()));
    }

    #[tokio::test]
    async fn deliver_outbound_message_retries_transient_failures_with_channel_policy() {
        let channel = Arc::new(SequenceChannel::new(vec![
            Ok(SendResult::Failed {
                reason: "temporarily_unavailable".into(),
            }),
            Ok(SendResult::Failed {
                reason: "internal error".into(),
            }),
            Ok(SendResult::Sent {
                platform_message_id: "final-msg".into(),
            }),
        ]));
        let outbound = OutboundMessage {
            channel: ChannelId::new("slack"),
            account_id: "default".into(),
            to: "channel-1".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
        };

        let delivery = deliver_outbound_message(channel.clone(), outbound)
            .await
            .expect("delivery should succeed");

        assert_eq!(delivery.status, "sent");
        assert_eq!(delivery.attempts, 3);
        assert_eq!(channel.sent.lock().await.len(), 3);
    }

    #[tokio::test]
    async fn deliver_outbound_message_does_not_retry_permanent_failures() {
        let channel = Arc::new(SequenceChannel::new(vec![Ok(SendResult::Failed {
            reason: "invalid_auth".into(),
        })]));
        let outbound = OutboundMessage {
            channel: ChannelId::new("slack"),
            account_id: "default".into(),
            to: "channel-1".into(),
            thread_id: None,
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
        };

        let delivery = deliver_outbound_message(channel.clone(), outbound)
            .await
            .expect("delivery should return a terminal failed record");

        assert_eq!(delivery.status, "failed");
        assert_eq!(delivery.attempts, 1);
        assert_eq!(channel.sent.lock().await.len(), 1);
    }
}
