//! Session tools: list sessions and get transcript history.

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};
use frankclaw_core::types::SessionKey;

use crate::{Tool, ToolContext};

/// Maximum transcript output size (80 KB).
const MAX_TRANSCRIPT_BYTES: usize = 80 * 1024;

// --------------------------------------------------------------------------
// sessions.list
// --------------------------------------------------------------------------

pub struct SessionsListTool;

#[async_trait]
impl Tool for SessionsListTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "sessions.list".into(),
            description: "List sessions for the current agent. Returns session keys, \
                channels, and last activity timestamps."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Max sessions to return. Default: 20."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Offset for pagination. Default: 0."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .clamp(1, 100) as usize;
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let sessions = ctx.sessions.list(&ctx.agent_id, limit, offset).await?;
        Ok(serde_json::json!({
            "sessions": sessions,
            "count": sessions.len(),
            "offset": offset,
        }))
    }
}

// --------------------------------------------------------------------------
// sessions.history
// --------------------------------------------------------------------------

pub struct SessionsHistoryTool;

#[async_trait]
impl Tool for SessionsHistoryTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "sessions.history".into(),
            description: "Get transcript history for a session. \
                Returns recent messages with roles, content, and timestamps."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_key": {
                        "type": "string",
                        "description": "Session key. Defaults to the current session."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Max transcript entries to return. Default: 50."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_key = args
            .get("session_key")
            .and_then(|v| v.as_str())
            .map(SessionKey::from_raw)
            .or(ctx.session_key)
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "sessions.history requires a session_key (none provided and no current session)".into(),
            })?;

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .clamp(1, 200) as usize;

        let entries = ctx.sessions.get_transcript(&session_key, limit, None).await?;

        // Serialize and cap output size.
        let serialized = serde_json::to_string(&entries).unwrap_or_default();
        let truncated = serialized.len() > MAX_TRANSCRIPT_BYTES;
        let output = if truncated {
            // Re-fetch with a smaller limit.
            let reduced = (limit / 2).max(1);
            let reduced_entries = ctx
                .sessions
                .get_transcript(&session_key, reduced, None)
                .await?;
            serde_json::json!({
                "session_key": session_key.as_str(),
                "entries": reduced_entries,
                "count": reduced_entries.len(),
                "truncated": true,
                "note": format!("Output was too large; returned {} of {} entries", reduced_entries.len(), entries.len()),
            })
        } else {
            serde_json::json!({
                "session_key": session_key.as_str(),
                "entries": entries,
                "count": entries.len(),
                "truncated": false,
            })
        };

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_list_definition_is_valid() {
        let tool = SessionsListTool;
        let def = tool.definition();
        assert_eq!(def.name, "sessions.list");
        assert_eq!(def.risk_level, ToolRiskLevel::ReadOnly);
    }

    #[test]
    fn sessions_history_definition_is_valid() {
        let tool = SessionsHistoryTool;
        let def = tool.definition();
        assert_eq!(def.name, "sessions.history");
        assert_eq!(def.risk_level, ToolRiskLevel::ReadOnly);
    }
}
