//! Config and agent inspection tools.

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};

use crate::{Tool, ToolContext};

/// Patterns that indicate a secret value that should be redacted.
const SECRET_PATTERNS: &[&str] = &[
    "key", "token", "secret", "password", "credential", "auth",
];

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SECRET_PATTERNS.iter().any(|pat| lower.contains(pat))
}

fn redact_secrets(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_secret_key(key) {
                    *val = serde_json::Value::String("[REDACTED]".into());
                } else {
                    redact_secrets(val);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr.iter_mut() {
                redact_secrets(val);
            }
        }
        _ => {}
    }
}

fn navigate_json_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

// --------------------------------------------------------------------------
// config.get
// --------------------------------------------------------------------------

pub struct ConfigGetTool;

#[async_trait]
impl Tool for ConfigGetTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "config.get".into(),
            description: "Inspect the FrankClaw gateway configuration. \
                Secrets are automatically redacted."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional dot-separated path to a config section \
                            (e.g., 'gateway.port', 'agents'). Returns the full config if omitted."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let config = ctx.config.as_ref().ok_or_else(|| FrankClawError::AgentRuntime {
            msg: "config.get is not available: no config loaded".into(),
        })?;

        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Serialize config to JSON, then redact secrets.
        let mut json = serde_json::to_value(config.as_ref()).map_err(|e| {
            FrankClawError::Internal {
                msg: format!("failed to serialize config: {e}"),
            }
        })?;
        redact_secrets(&mut json);

        // Navigate to the requested path.
        let result = if path.is_empty() {
            json
        } else {
            navigate_json_path(&json, path)
                .cloned()
                .ok_or_else(|| FrankClawError::InvalidRequest {
                    msg: format!("config path '{}' not found", path),
                })?
        };

        Ok(serde_json::json!({
            "path": if path.is_empty() { "(root)" } else { path },
            "value": result,
        }))
    }
}

// --------------------------------------------------------------------------
// agents.list
// --------------------------------------------------------------------------

pub struct AgentsListTool;

#[async_trait]
impl Tool for AgentsListTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "agents.list".into(),
            description: "List all configured agents with their IDs, names, models, and tools."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, _args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let config = ctx.config.as_ref().ok_or_else(|| FrankClawError::AgentRuntime {
            msg: "agents.list is not available: no config loaded".into(),
        })?;

        let agents: Vec<serde_json::Value> = config
            .agents
            .agents
            .iter()
            .map(|(id, agent)| {
                serde_json::json!({
                    "id": id.as_str(),
                    "name": agent.name,
                    "model": agent.model,
                    "tools": agent.tools,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "agents": agents,
            "default_agent": config.agents.default_agent.as_str(),
            "count": agents.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secret_keys() {
        let mut value = serde_json::json!({
            "gateway": {
                "port": 18789,
                "api_key": "sk-12345",
                "nested": {
                    "token": "abc",
                    "name": "test"
                }
            },
            "password": "secret123"
        });
        redact_secrets(&mut value);
        assert_eq!(value["gateway"]["api_key"], "[REDACTED]");
        assert_eq!(value["gateway"]["nested"]["token"], "[REDACTED]");
        assert_eq!(value["gateway"]["nested"]["name"], "test");
        assert_eq!(value["gateway"]["port"], 18789);
        assert_eq!(value["password"], "[REDACTED]");
    }

    #[test]
    fn navigate_json_path_works() {
        let value = serde_json::json!({
            "gateway": {
                "port": 18789,
                "bind": "127.0.0.1"
            }
        });
        assert_eq!(navigate_json_path(&value, "gateway.port"), Some(&serde_json::json!(18789)));
        assert_eq!(navigate_json_path(&value, "gateway.bind"), Some(&serde_json::json!("127.0.0.1")));
        assert!(navigate_json_path(&value, "nonexistent").is_none());
        assert!(navigate_json_path(&value, "gateway.nonexistent").is_none());
    }

    #[test]
    fn navigate_empty_path_returns_root() {
        let value = serde_json::json!({"a": 1});
        assert_eq!(navigate_json_path(&value, ""), Some(&value));
    }

    #[test]
    fn is_secret_key_detects_patterns() {
        assert!(is_secret_key("api_key"));
        assert!(is_secret_key("bot_token"));
        assert!(is_secret_key("secret_value"));
        assert!(is_secret_key("PASSWORD"));
        assert!(is_secret_key("auth_header"));
        assert!(!is_secret_key("port"));
        assert!(!is_secret_key("name"));
        assert!(!is_secret_key("enabled"));
    }
}
