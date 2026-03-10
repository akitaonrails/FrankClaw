use std::sync::Arc;

use frankclaw_core::channel::{ChannelPlugin, OutboundMessage, SendResult};
use frankclaw_core::error::Result;
use serde::{Deserialize, Serialize};

use crate::audit::{log_event, log_failure};

const MAX_OUTBOUND_ATTEMPTS: usize = 3;
const MAX_RETRY_DELAY_SECS: u64 = 30;

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
    let outbound_chunks = split_outbound_message(&outbound);
    let chunk_count = outbound_chunks.len();
    let mut chunks = Vec::with_capacity(outbound_chunks.len());
    let mut total_attempts = 0usize;
    let mut last_platform_message_id = None;
    let mut last_retry_after = None;

    for (index, chunk) in outbound_chunks.into_iter().enumerate() {
        let chunk_record =
            send_outbound_chunk(channel.clone(), chunk.clone(), index + 1, chunk_count).await?;
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
                if attempts >= MAX_OUTBOUND_ATTEMPTS {
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

                let delay_secs = retry_after_secs
                    .unwrap_or(attempts as u64)
                    .clamp(1, MAX_RETRY_DELAY_SECS);
                tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
            }
            Ok(SendResult::Failed { reason }) => {
                if attempts >= MAX_OUTBOUND_ATTEMPTS {
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

                tokio::time::sleep(std::time::Duration::from_secs(attempts as u64)).await;
            }
            Err(err) => {
                if attempts >= MAX_OUTBOUND_ATTEMPTS {
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

                tokio::time::sleep(std::time::Duration::from_secs(attempts as u64)).await;
            }
        }
    }
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
        .map(|text| OutboundMessage {
            text,
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use frankclaw_core::types::ChannelId;

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
}
