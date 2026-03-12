use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;

use chrono::Utc;
use frankclaw_core::config::{FrankClawConfig, WebhookMapping};
use frankclaw_core::error::{FrankClawError, Result};

use crate::state::GatewayState;

type HmacSha256 = Hmac<Sha256>;

/// Maximum age of a webhook timestamp before it is rejected (5 minutes).
const MAX_WEBHOOK_AGE_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub struct ResolvedWebhookRequest {
    pub mapping: WebhookMapping,
    pub message: String,
}

pub fn resolve_mapping(config: &FrankClawConfig, mapping_id: &str) -> Result<WebhookMapping> {
    config
        .hooks
        .parsed_mappings()?
        .into_iter()
        .find(|mapping| mapping.id == mapping_id)
        .ok_or_else(|| FrankClawError::InvalidRequest {
            msg: format!("unknown webhook mapping '{}'", mapping_id),
        })
}

pub fn verify_signature(config: &FrankClawConfig, body: &[u8], signature_header: Option<&str>) -> Result<()> {
    if !config.hooks.enabled {
        return Err(FrankClawError::Forbidden {
            method: "webhooks".into(),
        });
    }

    let secret = config
        .hooks
        .token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| FrankClawError::ConfigValidation {
            msg: "hooks.token is required when hooks are enabled".into(),
        })?;
    let signature = signature_header.ok_or_else(|| FrankClawError::AuthRequired)?;
    let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
    let provided = decode_hex(signature).ok_or_else(|| FrankClawError::AuthFailed)?;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| FrankClawError::Internal {
        msg: "failed to initialize webhook signature verifier".into(),
    })?;
    mac.update(body);
    mac.verify_slice(&provided).map_err(|_| FrankClawError::AuthFailed)
}

pub fn extract_message(mapping: &WebhookMapping, payload: &serde_json::Value) -> Result<String> {
    payload
        .get(&mapping.text_field)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| FrankClawError::InvalidRequest {
            msg: format!(
                "webhook payload must include a non-empty '{}' field",
                mapping.text_field
            ),
        })
}

pub fn resolve_request(
    config: &FrankClawConfig,
    mapping_id: &str,
    payload: &serde_json::Value,
) -> Result<ResolvedWebhookRequest> {
    let mapping = resolve_mapping(config, mapping_id)?;
    let message = extract_message(&mapping, payload)?;
    Ok(ResolvedWebhookRequest { mapping, message })
}

pub async fn execute_request(
    state: &Arc<GatewayState>,
    request: ResolvedWebhookRequest,
) -> Result<frankclaw_runtime::ChatResponse> {
    state
        .runtime
        .chat(frankclaw_runtime::ChatRequest {
            agent_id: request.mapping.agent_id,
            session_key: request.mapping.session_key,
            message: request.message,
            attachments: Vec::new(),
            model_id: None,
            max_tokens: None,
            temperature: None,
            stream_tx: None,
            thinking_budget: None,
            channel_id: None,
            channel_capabilities: None,
            canvas: None,
        })
        .await
}

/// Verify that the webhook timestamp is within the allowed window.
/// Returns an error if the timestamp is missing, unparseable, or too old.
pub fn verify_timestamp(timestamp_header: Option<&str>) -> Result<()> {
    let Some(ts) = timestamp_header.map(str::trim).filter(|v| !v.is_empty()) else {
        // Timestamp header is optional for backward compatibility.
        return Ok(());
    };
    let ts_secs: i64 = ts.parse().map_err(|_| FrankClawError::InvalidRequest {
        msg: format!("invalid webhook timestamp: '{ts}'"),
    })?;
    let now = Utc::now().timestamp();
    let age = now - ts_secs;
    if age > MAX_WEBHOOK_AGE_SECS {
        return Err(FrankClawError::AuthFailed);
    }
    if age < -MAX_WEBHOOK_AGE_SECS {
        // Timestamp is too far in the future — also suspicious.
        return Err(FrankClawError::AuthFailed);
    }
    Ok(())
}

pub fn encode_signature(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac should initialize");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut output = String::with_capacity(bytes.len() * 2 + 7);
    output.push_str("sha256=");
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let value = value.trim();
    if value.len() % 2 != 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars: Vec<_> = value.as_bytes().to_vec();
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

    #[test]
    fn verify_signature_accepts_matching_hmac() {
        let mut config = FrankClawConfig::default();
        config.hooks.enabled = true;
        config.hooks.token = Some("secret".into());
        config.hooks.mappings = vec![serde_json::json!({ "id": "incoming" })];
        let body = br#"{"message":"hello"}"#;
        let signature = encode_signature("secret", body);

        verify_signature(&config, body, Some(&signature)).expect("signature should verify");
    }

    #[test]
    fn extract_message_reads_mapping_text_field() {
        let mapping = WebhookMapping {
            id: "incoming".into(),
            agent_id: None,
            session_key: None,
            text_field: "prompt".into(),
        };

        let message =
            extract_message(&mapping, &serde_json::json!({ "prompt": "hello" })).expect("message should extract");
        assert_eq!(message, "hello");
    }

    #[test]
    fn verify_timestamp_accepts_recent() {
        let now = Utc::now().timestamp().to_string();
        verify_timestamp(Some(&now)).expect("current timestamp should be accepted");
    }

    #[test]
    fn verify_timestamp_rejects_old() {
        let old = (Utc::now().timestamp() - MAX_WEBHOOK_AGE_SECS - 10).to_string();
        assert!(verify_timestamp(Some(&old)).is_err());
    }

    #[test]
    fn verify_timestamp_rejects_future() {
        let future = (Utc::now().timestamp() + MAX_WEBHOOK_AGE_SECS + 10).to_string();
        assert!(verify_timestamp(Some(&future)).is_err());
    }

    #[test]
    fn verify_timestamp_allows_missing() {
        // Missing timestamp is allowed for backward compatibility.
        verify_timestamp(None).expect("missing timestamp should be allowed");
    }

    #[test]
    fn resolve_request_reuses_mapping_and_payload_validation() {
        let mut config = FrankClawConfig::default();
        config.hooks.enabled = true;
        config.hooks.token = Some("secret".into());
        config.hooks.mappings = vec![serde_json::json!({
            "id": "incoming",
            "text_field": "prompt",
        })];

        let request = resolve_request(
            &config,
            "incoming",
            &serde_json::json!({ "prompt": "hello" }),
        )
        .expect("request should resolve");
        assert_eq!(request.mapping.id, "incoming");
        assert_eq!(request.message, "hello");
    }
}
