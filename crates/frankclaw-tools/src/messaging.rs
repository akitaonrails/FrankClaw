//! Channel messaging tool: send messages via configured channels.

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};

use crate::{Tool, ToolContext};

/// Maximum text length for outbound messages.
const MAX_TEXT_LEN: usize = 4000;

pub struct MessageSendTool;

#[async_trait]
impl Tool for MessageSendTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "message.send".into(),
            description: "Send a text message via a configured channel \
                (Telegram, Discord, Slack, etc.)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["channel", "to", "text"],
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel name (e.g., 'telegram', 'discord', 'slack')."
                    },
                    "to": {
                        "type": "string",
                        "description": "Recipient identifier (chat ID, channel ID, etc.)."
                    },
                    "text": {
                        "type": "string",
                        "description": "Message text (max 4000 chars)."
                    },
                    "account_id": {
                        "type": "string",
                        "description": "Account ID to send from. Default: 'default'."
                    },
                    "thread_id": {
                        "type": "string",
                        "description": "Optional thread/topic ID for threaded channels."
                    },
                    "reply_to": {
                        "type": "string",
                        "description": "Optional message ID to reply to."
                    }
                }
            }),
            risk_level: ToolRiskLevel::Mutating,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let channels = ctx.channels.as_ref().ok_or_else(|| FrankClawError::AgentRuntime {
            msg: "message.send is not available: no channel service configured".into(),
        })?;

        let channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "message.send requires a non-empty channel".into(),
            })?;

        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "message.send requires a non-empty 'to' recipient".into(),
            })?;

        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "message.send requires text".into(),
            })?;

        if text.len() > MAX_TEXT_LEN {
            return Err(FrankClawError::InvalidRequest {
                msg: format!("message.send text exceeds {} char limit", MAX_TEXT_LEN),
            });
        }

        let account_id = args
            .get("account_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        let thread_id = args
            .get("thread_id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty());

        let reply_to = args
            .get("reply_to")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty());

        let message_id = channels
            .send_text(channel, account_id, to, text, thread_id, reply_to)
            .await?;

        Ok(serde_json::json!({
            "status": "sent",
            "channel": channel,
            "to": to,
            "message_id": message_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_send_definition_is_valid() {
        let tool = MessageSendTool;
        let def = tool.definition();
        assert_eq!(def.name, "message.send");
        assert_eq!(def.risk_level, ToolRiskLevel::Mutating);
    }
}
