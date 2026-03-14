//! MCP (Model Context Protocol) JSON-RPC 2.0 types.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

use serde::{Deserialize, Serialize};

/// Protocol version advertised during initialization.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Deserialize)]
pub struct McpResponse {
    pub jsonrpc: String,
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<McpError>,
}

/// JSON-RPC error.
#[derive(Debug, Clone, Deserialize)]
pub struct McpError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP error {}: {}", self.code, self.message)
    }
}

/// Tool definition from MCP server.
#[derive(Debug, Clone, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_schema", rename = "inputSchema")]
    pub input_schema: serde_json::Value,
    pub annotations: Option<McpToolAnnotations>,
}

/// Tool behavior annotations.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolAnnotations {
    #[serde(default, rename = "destructiveHint")]
    pub destructive_hint: bool,
    #[serde(default, rename = "readOnlyHint")]
    pub read_only_hint: bool,
}

fn default_schema() -> serde_json::Value {
    serde_json::json!({"type": "object"})
}

/// Content block in a tool result.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource {
        uri: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
}

/// Result of `tools/list`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
}

/// Result of `tools/call`.
#[derive(Debug, Clone, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ContentBlock>,
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

impl McpRequest {
    pub fn initialize(id: u64) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: "initialize".into(),
            params: Some(serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "frankclaw",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            })),
        }
    }

    pub fn initialized_notification() -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: None,
            method: "notifications/initialized".into(),
            params: None,
        }
    }

    pub fn list_tools(id: u64) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: "tools/list".into(),
            params: None,
        }
    }

    #[expect(clippy::needless_pass_by_value, reason = "value is moved directly into JSON serialization")]
    pub fn call_tool(id: u64, name: &str, arguments: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: "tools/call".into(),
            params: Some(serde_json::json!({
                "name": name,
                "arguments": arguments,
            })),
        }
    }
}

/// Deserialize a JSON-RPC id that may be a number, string, or null.
fn deserialize_flexible_id<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: serde_json::Value = Deserialize::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => Ok(n.as_u64()),
        serde_json::Value::String(s) => Ok(s.parse::<u64>().ok()),
        serde_json::Value::Null => Ok(None),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_request_has_correct_fields() {
        let req = McpRequest::initialize(1);
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, Some(1));
        let params = req.params.unwrap();
        assert_eq!(params["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(params["clientInfo"]["name"], "frankclaw");
    }

    #[test]
    fn initialized_notification_has_no_id() {
        let req = McpRequest::initialized_notification();
        assert!(req.id.is_none());
        assert_eq!(req.method, "notifications/initialized");
    }

    #[test]
    fn list_tools_request() {
        let req = McpRequest::list_tools(2);
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(2));
        assert!(req.params.is_none());
    }

    #[test]
    fn call_tool_request() {
        let req = McpRequest::call_tool(3, "my_tool", serde_json::json!({"key": "value"}));
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.id, Some(3));
        let params = req.params.unwrap();
        assert_eq!(params["name"], "my_tool");
        assert_eq!(params["arguments"]["key"], "value");
    }

    #[test]
    fn deserialize_response_with_numeric_id() {
        let json = r#"{"jsonrpc":"2.0","id":42,"result":{"ok":true}}"#;
        let resp: McpResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(42));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn deserialize_response_with_string_id() {
        let json = r#"{"jsonrpc":"2.0","id":"42","result":null}"#;
        let resp: McpResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(42));
    }

    #[test]
    fn deserialize_response_with_null_id() {
        let json = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-1,"message":"fail"}}"#;
        let resp: McpResponse = serde_json::from_str(json).unwrap();
        assert!(resp.id.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -1);
        assert_eq!(err.message, "fail");
    }

    #[test]
    fn deserialize_mcp_tool() {
        let json = r#"{
            "name": "read_file",
            "description": "Read a file",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description, "Read a file");
        assert!(tool.input_schema["properties"]["path"].is_object());
    }

    #[test]
    fn deserialize_mcp_tool_with_annotations() {
        let json = r#"{
            "name": "delete_file",
            "description": "Delete",
            "inputSchema": {"type": "object"},
            "annotations": {"destructiveHint": true, "readOnlyHint": false}
        }"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        let annotations = tool.annotations.unwrap();
        assert!(annotations.destructive_hint);
        assert!(!annotations.read_only_hint);
    }

    #[test]
    fn deserialize_tool_with_defaults() {
        let json = r#"{"name": "minimal"}"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "minimal");
        assert_eq!(tool.description, "");
        assert_eq!(tool.input_schema["type"], "object");
        assert!(tool.annotations.is_none());
    }

    #[test]
    fn deserialize_content_blocks() {
        let text_json = r#"{"type": "text", "text": "hello"}"#;
        let block: ContentBlock = serde_json::from_str(text_json).unwrap();
        match block {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text block"),
        }
    }

    #[test]
    fn deserialize_call_tool_result() {
        let json = r#"{
            "content": [{"type": "text", "text": "result"}],
            "isError": false
        }"#;
        let result: CallToolResult = serde_json::from_str(json).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn deserialize_call_tool_error_result() {
        let json = r#"{
            "content": [{"type": "text", "text": "something failed"}],
            "isError": true
        }"#;
        let result: CallToolResult = serde_json::from_str(json).unwrap();
        assert!(result.is_error);
    }
}
