//! MCP (Model Context Protocol) client for tool ecosystem integration.
//!
//! Connects to MCP servers via stdio (spawned process) or HTTP, discovers tools
//! via `tools/list`, and invokes them via `tools/call`. MCP tools are wrapped as
//! FrankClaw `Tool` implementations so they integrate seamlessly with the tool
//! registry.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

#![forbid(unsafe_code)]

pub mod protocol;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, trace, warn};

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};

use crate::{Tool, ToolContext};
use protocol::*;

/// Configuration for an MCP server.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Unique server name (used as tool name prefix).
    pub name: String,
    /// Transport configuration.
    pub transport: McpTransport,
    /// Whether this server is enabled.
    pub enabled: bool,
}

/// Transport type for connecting to an MCP server.
#[derive(Debug, Clone)]
pub enum McpTransport {
    /// HTTP transport — POST JSON-RPC to a URL.
    Http {
        url: String,
        headers: HashMap<String, String>,
    },
    /// Stdio transport — spawn a process and communicate via stdin/stdout.
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
}

/// MCP client connected to a single server.
pub struct McpClient {
    name: String,
    transport: McpTransportInner,
    next_id: AtomicU64,
    initialized: RwLock<bool>,
    tools_cache: RwLock<Option<Vec<McpTool>>>,
}

enum McpTransportInner {
    Http {
        url: String,
        headers: HashMap<String, String>,
        client: reqwest::Client,
    },
    Stdio {
        child: Mutex<Option<Child>>,
        stdin: Mutex<tokio::process::ChildStdin>,
        stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    },
}

impl McpClient {
    /// Create a new HTTP-based MCP client.
    pub fn new_http(name: String, url: String, headers: HashMap<String, String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| FrankClawError::Internal {
                msg: format!("failed to build HTTP client: {e}"),
            })?;
        Ok(Self {
            name,
            transport: McpTransportInner::Http {
                url,
                headers,
                client,
            },
            next_id: AtomicU64::new(1),
            initialized: RwLock::new(false),
            tools_cache: RwLock::new(None),
        })
    }

    /// Create a new stdio-based MCP client by spawning a process.
    pub async fn new_stdio(
        name: String,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| FrankClawError::Internal {
                msg: format!("failed to spawn MCP server '{command}': {e}"),
            })?;

        let stdin = child.stdin.take().ok_or_else(|| FrankClawError::Internal {
            msg: "failed to capture MCP server stdin".into(),
        })?;

        let stdout = child.stdout.take().ok_or_else(|| FrankClawError::Internal {
            msg: "failed to capture MCP server stdout".into(),
        })?;

        // Drain stderr in background to prevent blocking.
        if let Some(stderr) = child.stderr.take() {
            let server_name = name.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!(mcp_server = %server_name, stderr = %line);
                }
            });
        }

        Ok(Self {
            name,
            transport: McpTransportInner::Stdio {
                child: Mutex::new(Some(child)),
                stdin: Mutex::new(stdin),
                stdout: Mutex::new(BufReader::new(stdout)),
            },
            next_id: AtomicU64::new(1),
            initialized: RwLock::new(false),
            tools_cache: RwLock::new(None),
        })
    }

    /// Create a client from config.
    pub async fn from_config(config: &McpServerConfig) -> Result<Self> {
        match &config.transport {
            McpTransport::Http { url, headers } => {
                Self::new_http(config.name.clone(), url.clone(), headers.clone())
            }
            McpTransport::Stdio { command, args, env } => {
                Self::new_stdio(config.name.clone(), command, args, env).await
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a JSON-RPC request and receive the response.
    async fn send(&self, request: &McpRequest) -> Result<McpResponse> {
        match &self.transport {
            McpTransportInner::Http {
                url,
                headers,
                client,
            } => {
                let mut builder = client
                    .post(url)
                    .header("Content-Type", "application/json");
                for (k, v) in headers {
                    builder = builder.header(k, v);
                }
                let body = serde_json::to_string(request).map_err(|e| {
                    FrankClawError::Internal {
                        msg: format!("failed to serialize MCP request: {e}"),
                    }
                })?;

                trace!(method = %request.method, "MCP HTTP request");

                let resp = builder.body(body).send().await.map_err(|e| {
                    FrankClawError::Internal {
                        msg: format!("MCP HTTP request failed: {e}"),
                    }
                })?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    let body = if body.len() > 200 { &body[..200] } else { &body };
                    return Err(FrankClawError::Internal {
                        msg: format!("MCP server returned {status}: {body}"),
                    });
                }

                let text = resp.text().await.map_err(|e| FrankClawError::Internal {
                    msg: format!("failed to read MCP response: {e}"),
                })?;

                serde_json::from_str(&text).map_err(|e| FrankClawError::Internal {
                    msg: format!("failed to parse MCP response: {e}"),
                })
            }
            McpTransportInner::Stdio { stdin, stdout, .. } => {
                let body = serde_json::to_string(request).map_err(|e| {
                    FrankClawError::Internal {
                        msg: format!("failed to serialize MCP request: {e}"),
                    }
                })?;

                trace!(method = %request.method, "MCP stdio request");

                // Notifications (no id) are fire-and-forget.
                if request.id.is_none() {
                    let mut stdin_guard = stdin.lock().await;
                    stdin_guard
                        .write_all(body.as_bytes())
                        .await
                        .map_err(|e| FrankClawError::Internal {
                            msg: format!("MCP stdin write failed: {e}"),
                        })?;
                    stdin_guard
                        .write_all(b"\n")
                        .await
                        .map_err(|e| FrankClawError::Internal {
                            msg: format!("MCP stdin write failed: {e}"),
                        })?;
                    stdin_guard.flush().await.ok();
                    // Return a dummy response for notifications.
                    return Ok(McpResponse {
                        jsonrpc: "2.0".into(),
                        id: None,
                        result: None,
                        error: None,
                    });
                }

                let mut stdin_guard = stdin.lock().await;
                stdin_guard
                    .write_all(body.as_bytes())
                    .await
                    .map_err(|e| FrankClawError::Internal {
                        msg: format!("MCP stdin write failed: {e}"),
                    })?;
                stdin_guard
                    .write_all(b"\n")
                    .await
                    .map_err(|e| FrankClawError::Internal {
                        msg: format!("MCP stdin write failed: {e}"),
                    })?;
                stdin_guard.flush().await.ok();
                drop(stdin_guard);

                // Read response line with timeout.
                let mut stdout_guard = stdout.lock().await;
                let mut line = String::new();
                let read_result = tokio::time::timeout(
                    Duration::from_secs(30),
                    stdout_guard.read_line(&mut line),
                )
                .await;

                match read_result {
                    Ok(Ok(0)) => Err(FrankClawError::Internal {
                        msg: "MCP server closed stdout".into(),
                    }),
                    Ok(Ok(_)) => {
                        serde_json::from_str(line.trim()).map_err(|e| FrankClawError::Internal {
                            msg: format!("failed to parse MCP response: {e}"),
                        })
                    }
                    Ok(Err(e)) => Err(FrankClawError::Internal {
                        msg: format!("MCP stdout read failed: {e}"),
                    }),
                    Err(_) => Err(FrankClawError::Internal {
                        msg: "MCP server response timed out (30s)".into(),
                    }),
                }
            }
        }
    }

    /// Initialize the MCP connection if not already done.
    async fn ensure_initialized(&self) -> Result<()> {
        {
            let guard = self.initialized.read().await;
            if *guard {
                return Ok(());
            }
        }

        let mut guard = self.initialized.write().await;
        // Double-check after acquiring write lock.
        if *guard {
            return Ok(());
        }

        let id = self.next_id();
        let resp = self.send(&McpRequest::initialize(id)).await?;

        if let Some(err) = resp.error {
            return Err(FrankClawError::Internal {
                msg: format!("MCP initialization failed: {err}"),
            });
        }

        debug!(server = %self.name, "MCP server initialized");

        // Send the initialized notification (fire-and-forget).
        self.send(&McpRequest::initialized_notification()).await.ok();

        *guard = true;
        Ok(())
    }

    /// Discover tools from the MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        // Return cached tools if available.
        {
            let cache = self.tools_cache.read().await;
            if let Some(ref tools) = *cache {
                return Ok(tools.clone());
            }
        }

        self.ensure_initialized().await?;

        let id = self.next_id();
        let resp = self.send(&McpRequest::list_tools(id)).await?;

        if let Some(err) = resp.error {
            return Err(FrankClawError::Internal {
                msg: format!("MCP tools/list failed: {err}"),
            });
        }

        let result: ListToolsResult =
            serde_json::from_value(resp.result.unwrap_or_else(|| serde_json::json!({"tools": []}))).map_err(
                |e| FrankClawError::Internal {
                    msg: format!("failed to parse tools/list result: {e}"),
                },
            )?;

        debug!(server = %self.name, count = result.tools.len(), "MCP tools discovered");

        let mut cache = self.tools_cache.write().await;
        *cache = Some(result.tools.clone());

        Ok(result.tools)
    }

    /// Invoke a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String> {
        self.ensure_initialized().await?;

        // Strip top-level null values from arguments (LLMs emit these for
        // optional params, but many MCP servers reject explicit nulls).
        let arguments = strip_top_level_nulls(arguments);

        let id = self.next_id();
        let resp = self.send(&McpRequest::call_tool(id, tool_name, arguments)).await?;

        if let Some(err) = resp.error {
            return Err(FrankClawError::Internal {
                msg: format!("MCP tools/call '{tool_name}' failed: {err}"),
            });
        }

        let result: CallToolResult =
            serde_json::from_value(resp.result.unwrap_or_else(|| serde_json::json!({"content": []}))).map_err(
                |e| FrankClawError::Internal {
                    msg: format!("failed to parse tools/call result: {e}"),
                },
            )?;

        // Extract text content from blocks.
        let text: String = result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Resource { text, .. } => text.as_deref(),
                ContentBlock::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("MCP tool '{tool_name}' returned error: {text}"),
            });
        }

        Ok(text)
    }

    /// Shut down the client (kills stdio process if applicable).
    pub async fn shutdown(&self) {
        if let McpTransportInner::Stdio { child, .. } = &self.transport
            && let Some(mut child) = child.lock().await.take() {
                let _ = child.kill().await;
            }
    }

    /// Server name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let McpTransportInner::Stdio { child, .. } = &self.transport {
            // Best-effort kill on drop (non-async).
            if let Ok(mut guard) = child.try_lock()
                && let Some(ref mut child) = *guard {
                    let _ = child.start_kill();
                }
        }
    }
}

/// Wrapper that exposes an MCP tool as a FrankClaw `Tool`.
pub struct McpToolWrapper {
    server_name: String,
    client: Arc<McpClient>,
    tool: McpTool,
}

impl McpToolWrapper {
    pub fn new(server_name: String, client: Arc<McpClient>, tool: McpTool) -> Self {
        Self {
            server_name,
            client,
            tool,
        }
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn definition(&self) -> ToolDef {
        let risk_level = if let Some(ref annotations) = self.tool.annotations {
            if annotations.destructive_hint {
                ToolRiskLevel::Destructive
            } else if annotations.read_only_hint {
                ToolRiskLevel::ReadOnly
            } else {
                ToolRiskLevel::Mutating
            }
        } else {
            // Default: assume mutating for safety.
            ToolRiskLevel::Mutating
        };

        ToolDef {
            name: format!("{}_{}", self.server_name, self.tool.name),
            description: self.tool.description.clone(),
            parameters: self.tool.input_schema.clone(),
            risk_level,
        }
    }

    async fn invoke(&self, args: serde_json::Value, _ctx: ToolContext) -> Result<serde_json::Value> {
        let result = self.client.call_tool(&self.tool.name, args).await?;
        Ok(serde_json::json!({ "output": result }))
    }
}

/// Remove top-level keys with null values from a JSON object.
///
/// LLMs often emit `null` for optional parameters, but many MCP servers
/// reject explicit nulls in their input schemas.
fn strip_top_level_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let filtered: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .collect();
            serde_json::Value::Object(filtered)
        }
        other => other,
    }
}

/// Load MCP tool wrappers from a list of server configs.
///
/// Connects to each enabled server, discovers tools, and returns wrapped `Tool`
/// implementations ready for registration in the tool registry.
pub async fn load_mcp_tools(configs: &[McpServerConfig]) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

    for config in configs {
        if !config.enabled {
            continue;
        }

        match McpClient::from_config(config).await {
            Ok(client) => {
                let client = Arc::new(client);
                match client.list_tools().await {
                    Ok(mcp_tools) => {
                        debug!(
                            server = %config.name,
                            count = mcp_tools.len(),
                            "loaded MCP tools"
                        );
                        for mcp_tool in mcp_tools {
                            tools.push(Arc::new(McpToolWrapper::new(
                                config.name.clone(),
                                client.clone(),
                                mcp_tool,
                            )));
                        }
                    }
                    Err(e) => {
                        warn!(server = %config.name, error = %e, "failed to discover MCP tools");
                    }
                }
            }
            Err(e) => {
                warn!(server = %config.name, error = %e, "failed to connect to MCP server");
            }
        }
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_nulls_removes_top_level() {
        let input = serde_json::json!({"a": 1, "b": null, "c": "hello"});
        let result = strip_top_level_nulls(input);
        assert_eq!(result, serde_json::json!({"a": 1, "c": "hello"}));
    }

    #[test]
    fn strip_nulls_preserves_nested_nulls() {
        let input = serde_json::json!({"a": {"b": null}, "c": 1});
        let result = strip_top_level_nulls(input);
        assert_eq!(result, serde_json::json!({"a": {"b": null}, "c": 1}));
    }

    #[test]
    fn strip_nulls_non_object_passthrough() {
        let input = serde_json::json!("hello");
        let result = strip_top_level_nulls(input.clone());
        assert_eq!(result, input);
    }

    #[test]
    fn strip_nulls_all_null() {
        let input = serde_json::json!({"a": null, "b": null});
        let result = strip_top_level_nulls(input);
        assert_eq!(result, serde_json::json!({}));
    }

    #[test]
    fn mcp_tool_wrapper_definition_prefixes_name() {
        let tool = McpTool {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({"type": "object"}),
            annotations: None,
        };
        let client = Arc::new(McpClient::new_http(
            "test".into(),
            "http://localhost:1234".into(),
            HashMap::new(),
        ).expect("client should build"));
        let wrapper = McpToolWrapper::new("myserver".into(), client, tool);
        let def = wrapper.definition();
        assert_eq!(def.name, "myserver_read_file");
        assert_eq!(def.description, "Read a file");
        // Default risk level for unannotated tools is Mutating.
        assert_eq!(def.risk_level, ToolRiskLevel::Mutating);
    }

    #[test]
    fn mcp_tool_wrapper_read_only_annotation() {
        let tool = McpTool {
            name: "list".into(),
            description: "List items".into(),
            input_schema: serde_json::json!({"type": "object"}),
            annotations: Some(McpToolAnnotations {
                destructive_hint: false,
                read_only_hint: true,
            }),
        };
        let client = Arc::new(McpClient::new_http(
            "test".into(),
            "http://localhost:1234".into(),
            HashMap::new(),
        ).expect("client should build"));
        let wrapper = McpToolWrapper::new("srv".into(), client, tool);
        assert_eq!(wrapper.definition().risk_level, ToolRiskLevel::ReadOnly);
    }

    #[test]
    fn mcp_tool_wrapper_destructive_annotation() {
        let tool = McpTool {
            name: "delete".into(),
            description: "Delete".into(),
            input_schema: serde_json::json!({"type": "object"}),
            annotations: Some(McpToolAnnotations {
                destructive_hint: true,
                read_only_hint: false,
            }),
        };
        let client = Arc::new(McpClient::new_http(
            "test".into(),
            "http://localhost:1234".into(),
            HashMap::new(),
        ).expect("client should build"));
        let wrapper = McpToolWrapper::new("srv".into(), client, tool);
        assert_eq!(wrapper.definition().risk_level, ToolRiskLevel::Destructive);
    }

    #[test]
    fn mcp_client_id_increments() {
        let client = McpClient::new_http(
            "test".into(),
            "http://localhost:1234".into(),
            HashMap::new(),
        ).expect("client should build");
        assert_eq!(client.next_id(), 1);
        assert_eq!(client.next_id(), 2);
        assert_eq!(client.next_id(), 3);
    }

    #[test]
    fn mcp_server_config_from_transport() {
        let config = McpServerConfig {
            name: "test".into(),
            transport: McpTransport::Http {
                url: "http://localhost:8080".into(),
                headers: HashMap::new(),
            },
            enabled: true,
        };
        assert!(config.enabled);
        assert_eq!(config.name, "test");
    }
}
