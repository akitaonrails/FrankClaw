#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use tokio::sync::Mutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::ToolDef;
use frankclaw_core::session::SessionStore;
use frankclaw_core::types::{AgentId, SessionKey};

#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: AgentId,
    pub session_key: Option<SessionKey>,
    pub sessions: Arc<dyn SessionStore>,
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub name: String,
    pub output: serde_json::Value,
}

#[async_trait]
pub trait Tool: Send + Sync + 'static {
    fn definition(&self) -> ToolDef;

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn with_builtins() -> Self {
        let browser = Arc::new(
            BrowserClient::from_env()
                .unwrap_or_else(|_| BrowserClient::new("http://127.0.0.1:9222/").expect("default browser client should build")),
        );
        let mut registry = Self {
            tools: HashMap::new(),
        };
        registry.register(Arc::new(SessionInspectTool));
        registry.register(Arc::new(BrowserOpenTool::new(browser.clone())));
        registry.register(Arc::new(BrowserExtractTool::new(browser.clone())));
        registry.register(Arc::new(BrowserSnapshotTool::new(browser.clone())));
        registry.register(Arc::new(BrowserClickTool::new(browser.clone())));
        registry.register(Arc::new(BrowserTypeTool::new(browser)));
        registry
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.definition().name.clone(), tool);
    }

    pub fn validate_names(&self, names: &[String]) -> Result<()> {
        for name in names {
            if !self.tools.contains_key(name) {
                return Err(FrankClawError::ConfigValidation {
                    msg: format!("unknown tool '{}'", name),
                });
            }
        }
        Ok(())
    }

    pub fn definitions(&self, names: &[String]) -> Result<Vec<ToolDef>> {
        self.validate_names(names)?;
        Ok(names
            .iter()
            .filter_map(|name| self.tools.get(name))
            .map(|tool| tool.definition())
            .collect())
    }

    pub async fn invoke_allowed(
        &self,
        allowed_tools: &[String],
        name: &str,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        if !allowed_tools.iter().any(|allowed| allowed == name) {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("tool '{}' is not allowed for agent '{}'", name, ctx.agent_id),
            });
        }

        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("unknown tool '{}'", name),
            })?;
        let output = tool.invoke(args, ctx).await?;
        Ok(ToolOutput {
            name: name.to_string(),
            output,
        })
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[derive(Debug, Clone)]
struct BrowserSession {
    session_id: String,
    target_id: String,
    page_ws_url: String,
    current_url: String,
    title: Option<String>,
    last_updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
struct BrowserSnapshot {
    session_id: String,
    target_id: String,
    url: String,
    title: Option<String>,
    text: String,
    html: String,
    captured_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
struct DevtoolsTarget {
    id: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: String,
}

struct BrowserClient {
    base_url: Url,
    http: Client,
    sessions: Mutex<HashMap<String, BrowserSession>>,
    next_command_id: AtomicU64,
}

impl BrowserClient {
    fn from_env() -> Result<Self> {
        let raw = std::env::var("FRANKCLAW_BROWSER_DEVTOOLS_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:9222/".to_string());
        Self::new(&raw)
    }

    fn new(raw_base_url: &str) -> Result<Self> {
        let mut base_url = Url::parse(raw_base_url).map_err(|err| FrankClawError::ConfigValidation {
            msg: format!("invalid FRANKCLAW_BROWSER_DEVTOOLS_URL: {err}"),
        })?;
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }

        Ok(Self {
            base_url,
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|err| FrankClawError::Internal {
                    msg: format!("failed to build browser client: {err}"),
                })?,
            sessions: Mutex::new(HashMap::new()),
            next_command_id: AtomicU64::new(1),
        })
    }

    fn resolve_session_id(&self, requested: Option<&str>, ctx: &ToolContext) -> Result<String> {
        if let Some(session_id) = requested.map(str::trim).filter(|value| !value.is_empty()) {
            return Ok(session_id.to_string());
        }
        if let Some(session_key) = &ctx.session_key {
            return Ok(format!("session:{}", session_key.as_str()));
        }
        Err(FrankClawError::InvalidRequest {
            msg: "browser tool requires session_id or session context".into(),
        })
    }

    async fn open(&self, session_id: String, url: &str) -> Result<BrowserSnapshot> {
        let existing = { self.sessions.lock().await.get(&session_id).cloned() };
        let session = match existing {
            Some(mut session) => {
                self.navigate_target(&session, url).await?;
                session.current_url = url.to_string();
                session.last_updated_at = Utc::now();
                self.sessions
                    .lock()
                    .await
                    .insert(session_id.clone(), session.clone());
                session
            }
            None => {
                let target = self.create_target(url).await?;
                let now = Utc::now();
                let session = BrowserSession {
                    session_id: session_id.clone(),
                    target_id: target.id,
                    page_ws_url: target.web_socket_debugger_url,
                    current_url: target.url,
                    title: None,
                    last_updated_at: now,
                };
                self.sessions
                    .lock()
                    .await
                    .insert(session_id.clone(), session.clone());
                session
            }
        };

        let snapshot = self.snapshot_session(&session).await?;
        let mut sessions = self.sessions.lock().await;
        if let Some(entry) = sessions.get_mut(&session_id) {
            entry.title = snapshot.title.clone();
            entry.current_url = snapshot.url.clone();
            entry.last_updated_at = snapshot.captured_at;
        }
        Ok(snapshot)
    }

    async fn extract(&self, session_id: &str) -> Result<BrowserSnapshot> {
        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("browser session '{}' was not opened yet", session_id),
            })?;
        self.snapshot_session(&session).await
    }

    async fn click(&self, session_id: &str, selector: &str) -> Result<BrowserSnapshot> {
        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("browser session '{}' was not opened yet", session_id),
            })?;
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        self.wait_for_ready(&mut socket).await?;
        let clicked = self
            .evaluate_bool(&mut socket, &click_expression(selector))
            .await?;
        if !clicked {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("browser.click could not find selector '{}'", selector),
            });
        }
        self.snapshot_session(&session).await
    }

    async fn type_text(&self, session_id: &str, selector: &str, text: &str) -> Result<BrowserSnapshot> {
        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("browser session '{}' was not opened yet", session_id),
            })?;
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        self.wait_for_ready(&mut socket).await?;
        let typed = self
            .evaluate_bool(&mut socket, &type_expression(selector, text))
            .await?;
        if !typed {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("browser.type could not find selector '{}'", selector),
            });
        }
        self.snapshot_session(&session).await
    }

    async fn create_target(&self, url: &str) -> Result<DevtoolsTarget> {
        let mut endpoint = self.base_url.join("json/new").map_err(|err| FrankClawError::Internal {
            msg: format!("invalid browser endpoint: {err}"),
        })?;
        endpoint.set_query(Some(url));
        let response = self
            .http
            .put(endpoint)
            .send()
            .await
            .map_err(|err| FrankClawError::AgentRuntime {
                msg: format!("failed to create browser target: {err}"),
            })?;
        if !response.status().is_success() {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("browser target creation failed with HTTP {}", response.status()),
            });
        }
        response.json::<DevtoolsTarget>().await.map_err(|err| FrankClawError::AgentRuntime {
            msg: format!("invalid browser target response: {err}"),
        })
    }

    async fn navigate_target(&self, session: &BrowserSession, url: &str) -> Result<()> {
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        let _ = self
            .send_command(
                &mut socket,
                "Page.navigate",
                serde_json::json!({ "url": url }),
            )
            .await?;
        self.wait_for_ready(&mut socket).await?;
        Ok(())
    }

    async fn snapshot_session(&self, session: &BrowserSession) -> Result<BrowserSnapshot> {
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        self.wait_for_ready(&mut socket).await?;
        let title = self
            .evaluate_string(&mut socket, "document.title || ''")
            .await?;
        let text = self
            .evaluate_string(&mut socket, "document.body ? document.body.innerText : ''")
            .await?;
        let html = self
            .evaluate_string(
                &mut socket,
                "document.documentElement ? document.documentElement.outerHTML : ''",
            )
            .await?;
        let url = self
            .evaluate_string(&mut socket, "window.location.href")
            .await?;

        Ok(BrowserSnapshot {
            session_id: session.session_id.clone(),
            target_id: session.target_id.clone(),
            url,
            title: (!title.trim().is_empty()).then_some(title),
            text,
            html,
            captured_at: Utc::now(),
        })
    }

    async fn connect_page_socket(
        &self,
        ws_url: &str,
    ) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>> {
        let (socket, _) = connect_async(ws_url)
            .await
            .map_err(|err| FrankClawError::AgentRuntime {
                msg: format!("failed to connect to browser page socket: {err}"),
            })?;
        Ok(socket)
    }

    async fn wait_for_ready(
        &self,
        socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    ) -> Result<()> {
        for _ in 0..20 {
            let ready_state = self
                .evaluate_string(socket, "document.readyState")
                .await
                .unwrap_or_else(|_| "complete".to_string());
            if ready_state == "interactive" || ready_state == "complete" {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }

    async fn evaluate_string(
        &self,
        socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        expression: &str,
    ) -> Result<String> {
        Ok(self
            .evaluate_value(socket, expression)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    async fn evaluate_bool(
        &self,
        socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        expression: &str,
    ) -> Result<bool> {
        Ok(self
            .evaluate_value(socket, expression)
            .await?
            .as_bool()
            .unwrap_or(false))
    }

    async fn evaluate_value(
        &self,
        socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        expression: &str,
    ) -> Result<serde_json::Value> {
        let response = self
            .send_command(
                socket,
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": expression,
                    "returnByValue": true
                }),
            )
            .await?;
        Ok(response["result"]["result"]["value"].clone())
    }

    async fn send_command(
        &self,
        socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.next_command_id.fetch_add(1, Ordering::Relaxed);
        socket
            .send(Message::Text(
                serde_json::json!({
                    "id": id,
                    "method": method,
                    "params": params,
                })
                .to_string()
                .into(),
            ))
            .await
            .map_err(|err| FrankClawError::AgentRuntime {
                msg: format!("failed to send browser command '{method}': {err}"),
            })?;

        while let Some(message) = socket.next().await {
            let message = message.map_err(|err| FrankClawError::AgentRuntime {
                msg: format!("browser socket read failed: {err}"),
            })?;
            let Message::Text(text) = message else {
                continue;
            };
            let frame: serde_json::Value = serde_json::from_str(text.as_ref()).map_err(|err| {
                FrankClawError::AgentRuntime {
                    msg: format!("browser socket sent invalid JSON: {err}"),
                }
            })?;
            if frame["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(message) = frame["error"]["message"].as_str() {
                return Err(FrankClawError::AgentRuntime {
                    msg: format!("browser command '{method}' failed: {message}"),
                });
            }
            return Ok(frame);
        }

        Err(FrankClawError::AgentRuntime {
            msg: format!("browser socket closed while waiting for '{method}'"),
        })
    }
}

struct SessionInspectTool;
struct BrowserOpenTool {
    client: Arc<BrowserClient>,
}
struct BrowserExtractTool {
    client: Arc<BrowserClient>,
}
struct BrowserSnapshotTool {
    client: Arc<BrowserClient>,
}
struct BrowserClickTool {
    client: Arc<BrowserClient>,
}
struct BrowserTypeTool {
    client: Arc<BrowserClient>,
}

impl BrowserOpenTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

impl BrowserExtractTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

impl BrowserSnapshotTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

impl BrowserClickTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

impl BrowserTypeTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for SessionInspectTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "session.inspect".into(),
            description: "Inspect one session entry and recent transcript messages.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_key": {
                        "type": "string",
                        "description": "Optional explicit session key. Defaults to the current tool context session."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Maximum transcript entries to return."
                    }
                }
            }),
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_key = args
            .get("session_key")
            .and_then(|value| value.as_str())
            .map(SessionKey::from_raw)
            .or(ctx.session_key)
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "session.inspect requires a session_key".into(),
            })?;
        let limit = args
            .get("limit")
            .and_then(|value| value.as_u64())
            .unwrap_or(20)
            .clamp(1, 100) as usize;

        let session = ctx.sessions.get(&session_key).await?;
        let entries = ctx.sessions.get_transcript(&session_key, limit, None).await?;

        Ok(serde_json::json!({
            "session": session,
            "entries": entries,
        }))
    }
}

#[async_trait]
impl Tool for BrowserOpenTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.open".into(),
            description: "Create or reuse a Chromium-backed browser session and navigate it to a URL over the DevTools protocol.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": { "type": "string", "description": "HTTP or HTTPS URL to open." },
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." }
                }
            }),
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let url = args
            .get("url")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "browser.open requires a non-empty url".into(),
            })?;
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        let snapshot = self.client.open(session_id, url).await?;
        Ok(snapshot_result(snapshot, false, 800))
    }
}

#[async_trait]
impl Tool for BrowserExtractTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.extract".into(),
            description: "Extract visible text from an existing Chromium-backed browser session.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." },
                    "max_chars": { "type": "integer", "minimum": 1, "maximum": 8000, "description": "Maximum number of visible text characters to return." }
                }
            }),
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        let max_chars = args
            .get("max_chars")
            .and_then(|value| value.as_u64())
            .unwrap_or(2000)
            .clamp(1, 8000) as usize;
        let snapshot = self.client.extract(&session_id).await?;
        Ok(snapshot_result(snapshot, false, max_chars))
    }
}

#[async_trait]
impl Tool for BrowserSnapshotTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.snapshot".into(),
            description: "Return stored HTML plus visible text from an existing Chromium-backed browser session.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." },
                    "max_chars": { "type": "integer", "minimum": 1, "maximum": 32000, "description": "Maximum number of HTML characters to return." }
                }
            }),
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        let max_chars = args
            .get("max_chars")
            .and_then(|value| value.as_u64())
            .unwrap_or(8000)
            .clamp(1, 32000) as usize;
        let snapshot = self.client.extract(&session_id).await?;
        Ok(snapshot_result(snapshot, true, max_chars))
    }
}

#[async_trait]
impl Tool for BrowserClickTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.click".into(),
            description: "Click a DOM element by CSS selector in an existing Chromium-backed browser session.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["selector"],
                "properties": {
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." },
                    "selector": { "type": "string", "description": "CSS selector for the target element." }
                }
            }),
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        let selector = args
            .get("selector")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "browser.click requires a non-empty selector".into(),
            })?;
        let snapshot = self.client.click(&session_id, selector).await?;
        Ok(snapshot_result(snapshot, false, 1000))
    }
}

#[async_trait]
impl Tool for BrowserTypeTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.type".into(),
            description: "Set an input or textarea value by CSS selector in an existing Chromium-backed browser session.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["selector", "text"],
                "properties": {
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." },
                    "selector": { "type": "string", "description": "CSS selector for the target input or textarea." },
                    "text": { "type": "string", "description": "Replacement text value." }
                }
            }),
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        let selector = args
            .get("selector")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "browser.type requires a non-empty selector".into(),
            })?;
        let text = args
            .get("text")
            .and_then(|value| value.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "browser.type requires text".into(),
            })?;
        let snapshot = self.client.type_text(&session_id, selector, text).await?;
        Ok(snapshot_result(snapshot, false, 1000))
    }
}

fn snapshot_result(snapshot: BrowserSnapshot, include_html: bool, max_chars: usize) -> serde_json::Value {
    let mut value = serde_json::json!({
        "session_id": snapshot.session_id,
        "target_id": snapshot.target_id,
        "url": snapshot.url,
        "title": snapshot.title,
        "text": truncate_chars(&snapshot.text, max_chars),
        "captured_at": snapshot.captured_at,
    });
    if include_html {
        value["html"] = serde_json::json!(truncate_chars(&snapshot.html, max_chars));
    }
    value
}

fn click_expression(selector: &str) -> String {
    let selector = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(function() {{ const el = document.querySelector({selector}); if (!el) return false; el.click(); return true; }})()"
    )
}

fn type_expression(selector: &str, text: &str) -> String {
    let selector = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into());
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(function() {{ const el = document.querySelector({selector}); if (!el) return false; el.focus(); if ('value' in el) {{ el.value = {text}; }} else {{ el.textContent = {text}; }} el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.dispatchEvent(new Event('change', {{ bubbles: true }})); return true; }})()"
    )
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let total_chars = value.chars().count();
    let truncated: String = value.chars().take(max_chars).collect();
    if total_chars > max_chars {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
    use axum::extract::{RawQuery, State};
    use axum::response::IntoResponse;
    use axum::{Json, Router, routing::{get, put}};
    use chrono::Utc;
    use tokio::net::TcpListener;

    use frankclaw_core::session::{
        PruningConfig, SessionEntry, SessionScoping, SessionStore, TranscriptEntry,
    };
    use frankclaw_core::types::{ChannelId, Role};

    use super::*;

    #[derive(Default)]
    struct MockSessionStore {
        sessions: Mutex<BTreeMap<String, SessionEntry>>,
        transcripts: Mutex<BTreeMap<String, Vec<TranscriptEntry>>>,
    }

    #[derive(Clone)]
    struct MockBrowserState {
        page_url: Arc<Mutex<String>>,
        title: Arc<Mutex<String>>,
        text: Arc<Mutex<String>>,
        html: Arc<Mutex<String>>,
        websocket_url: String,
    }

    #[async_trait]
    impl SessionStore for MockSessionStore {
        async fn get(&self, key: &SessionKey) -> Result<Option<SessionEntry>> {
            Ok(self.sessions.lock().await.get(key.as_str()).cloned())
        }

        async fn upsert(&self, entry: &SessionEntry) -> Result<()> {
            self.sessions
                .lock()
                .await
                .insert(entry.key.as_str().to_string(), entry.clone());
            Ok(())
        }

        async fn delete(&self, key: &SessionKey) -> Result<()> {
            self.sessions.lock().await.remove(key.as_str());
            self.transcripts.lock().await.remove(key.as_str());
            Ok(())
        }

        async fn list(
            &self,
            _agent_id: &AgentId,
            _limit: usize,
            _offset: usize,
        ) -> Result<Vec<SessionEntry>> {
            Ok(self.sessions.lock().await.values().cloned().collect())
        }

        async fn append_transcript(&self, key: &SessionKey, entry: &TranscriptEntry) -> Result<()> {
            self.transcripts
                .lock()
                .await
                .entry(key.as_str().to_string())
                .or_default()
                .push(entry.clone());
            Ok(())
        }

        async fn get_transcript(
            &self,
            key: &SessionKey,
            limit: usize,
            _before_seq: Option<u64>,
        ) -> Result<Vec<TranscriptEntry>> {
            Ok(self
                .transcripts
                .lock()
                .await
                .get(key.as_str())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(limit)
                .collect())
        }

        async fn clear_transcript(&self, key: &SessionKey) -> Result<()> {
            self.transcripts.lock().await.remove(key.as_str());
            Ok(())
        }

        async fn maintenance(&self, _config: &PruningConfig) -> Result<u64> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn session_inspect_returns_session_and_entries() {
        let store = Arc::new(MockSessionStore::default());
        let key = SessionKey::from_raw("main:web:default");
        store
            .upsert(&SessionEntry {
                key: key.clone(),
                agent_id: AgentId::default_agent(),
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                scoping: SessionScoping::PerChannelPeer,
                created_at: Utc::now(),
                last_message_at: Some(Utc::now()),
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("session should upsert");
        store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: "hello".into(),
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("transcript should append");

        let registry = ToolRegistry::with_builtins();
        let result = registry
            .invoke_allowed(
                &["session.inspect".into()],
                "session.inspect",
                serde_json::json!({ "limit": 5 }),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(key.clone()),
                    sessions: store as Arc<dyn SessionStore>,
                },
            )
            .await
            .expect("tool should succeed");

        assert_eq!(result.name, "session.inspect");
        assert_eq!(result.output["session"]["key"], serde_json::json!(key.as_str()));
        assert_eq!(result.output["entries"][0]["content"], serde_json::json!("hello"));
    }

    #[tokio::test]
    async fn invoke_allowed_rejects_unlisted_tools() {
        let registry = ToolRegistry::with_builtins();
        let err = registry
            .invoke_allowed(
                &[],
                "session.inspect",
                serde_json::json!({}),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: None,
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect_err("tool should be rejected");

        assert!(err.to_string().contains("not allowed"));
    }

    #[tokio::test]
    async fn browser_tools_drive_mock_devtools_server() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have local addr");
        let mock_state = MockBrowserState {
            page_url: Arc::new(Mutex::new("about:blank".into())),
            title: Arc::new(Mutex::new("Example page".into())),
            text: Arc::new(Mutex::new("Hello from Chromium".into())),
            html: Arc::new(Mutex::new("<html><body><h1>Hello from Chromium</h1></body></html>".into())),
            websocket_url: format!("ws://127.0.0.1:{}/devtools/page/mock-page", addr.port()),
        };
        let app = Router::new()
            .route("/json/new", put(mock_create_target))
            .route("/devtools/page/mock-page", get(mock_page_ws))
            .with_state(mock_state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server should run");
        });

        let client = Arc::new(
            BrowserClient::new(&format!("http://{addr}/"))
                .expect("browser client should build"),
        );
        let mut registry = ToolRegistry {
            tools: HashMap::new(),
        };
        registry.register(Arc::new(BrowserOpenTool::new(client.clone())));
        registry.register(Arc::new(BrowserExtractTool::new(client.clone())));
        registry.register(Arc::new(BrowserSnapshotTool::new(client.clone())));
        registry.register(Arc::new(BrowserClickTool::new(client.clone())));
        registry.register(Arc::new(BrowserTypeTool::new(client)));

        let ctx = ToolContext {
            agent_id: AgentId::default_agent(),
            session_key: Some(SessionKey::from_raw("default:web:browser")),
            sessions: Arc::new(MockSessionStore::default()),
        };
        let allowed = vec![
            "browser.open".into(),
            "browser.extract".into(),
            "browser.snapshot".into(),
            "browser.click".into(),
            "browser.type".into(),
        ];

        let opened = registry
            .invoke_allowed(
                &allowed,
                "browser.open",
                serde_json::json!({ "url": "https://example.com/" }),
                ctx.clone(),
            )
            .await
            .expect("browser.open should succeed");
        assert_eq!(opened.output["title"], serde_json::json!("Example page"));

        let extracted = registry
            .invoke_allowed(
                &allowed,
                "browser.extract",
                serde_json::json!({ "max_chars": 32 }),
                ctx.clone(),
            )
            .await
            .expect("browser.extract should succeed");
        assert_eq!(extracted.output["text"], serde_json::json!("Hello from Chromium"));

        let snapshot = registry
            .invoke_allowed(
                &allowed,
                "browser.snapshot",
                serde_json::json!({ "max_chars": 128 }),
                ctx,
            )
            .await
            .expect("browser.snapshot should succeed");
        assert!(snapshot.output["html"]
            .as_str()
            .expect("html should exist")
            .contains("<h1>Hello from Chromium</h1>"));

        let clicked = registry
            .invoke_allowed(
                &allowed,
                "browser.click",
                serde_json::json!({ "selector": "#submit" }),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(SessionKey::from_raw("default:web:browser")),
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect("browser.click should succeed");
        assert_eq!(clicked.output["title"], serde_json::json!("Clicked"));

        let typed = registry
            .invoke_allowed(
                &allowed,
                "browser.type",
                serde_json::json!({ "selector": "#query", "text": "frankclaw" }),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(SessionKey::from_raw("default:web:browser")),
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect("browser.type should succeed");
        assert!(
            typed.output["text"]
                .as_str()
                .expect("text should exist")
                .contains("Typed frankclaw")
        );
    }

    #[tokio::test]
    #[ignore = "requires a live Chromium DevTools endpoint via FRANKCLAW_BROWSER_DEVTOOLS_URL"]
    async fn browser_tools_drive_real_chromium() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have local addr");
        let app = Router::new().route(
            "/",
            get(|| async {
                axum::response::Html(
                    r#"<!doctype html>
                    <html>
                      <head>
                        <title>Ready</title>
                        <meta charset="utf-8">
                      </head>
                      <body>
                        <input id="query" value="">
                        <button id="submit" type="button">Submit</button>
                        <div id="status">Idle</div>
                        <script>
                          const query = document.getElementById("query");
                          const status = document.getElementById("status");
                          query.addEventListener("input", () => {
                            document.title = "Typed";
                            status.textContent = "Typed " + query.value;
                          });
                          document.getElementById("submit").addEventListener("click", () => {
                            document.title = "Clicked";
                            status.textContent = "Clicked " + query.value;
                          });
                        </script>
                      </body>
                    </html>"#,
                )
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("real browser test server should run");
        });

        let registry = ToolRegistry::with_builtins();
        let ctx = ToolContext {
            agent_id: AgentId::default_agent(),
            session_key: Some(SessionKey::from_raw("default:web:real-browser")),
            sessions: Arc::new(MockSessionStore::default()),
        };
        let allowed = vec![
            "browser.open".into(),
            "browser.extract".into(),
            "browser.snapshot".into(),
            "browser.click".into(),
            "browser.type".into(),
        ];

        let opened = registry
            .invoke_allowed(
                &allowed,
                "browser.open",
                serde_json::json!({ "url": format!("http://{addr}/") }),
                ctx.clone(),
            )
            .await
            .expect("browser.open should succeed against real chromium");
        assert_eq!(opened.output["title"], serde_json::json!("Ready"));

        let typed = registry
            .invoke_allowed(
                &allowed,
                "browser.type",
                serde_json::json!({ "selector": "#query", "text": "frankclaw" }),
                ctx.clone(),
            )
            .await
            .expect("browser.type should succeed against real chromium");
        assert_eq!(typed.output["title"], serde_json::json!("Typed"));
        assert!(
            typed.output["text"]
                .as_str()
                .expect("text should exist")
                .contains("Typed frankclaw")
        );

        let clicked = registry
            .invoke_allowed(
                &allowed,
                "browser.click",
                serde_json::json!({ "selector": "#submit" }),
                ctx.clone(),
            )
            .await
            .expect("browser.click should succeed against real chromium");
        assert_eq!(clicked.output["title"], serde_json::json!("Clicked"));
        assert!(
            clicked.output["text"]
                .as_str()
                .expect("text should exist")
                .contains("Clicked frankclaw")
        );

        let snapshot = registry
            .invoke_allowed(
                &allowed,
                "browser.snapshot",
                serde_json::json!({ "max_chars": 4096 }),
                ctx,
            )
            .await
            .expect("browser.snapshot should succeed against real chromium");
        assert!(
            snapshot.output["html"]
                .as_str()
                .expect("html should exist")
                .contains("id=\"status\"")
        );
    }

    async fn mock_create_target(
        State(state): State<MockBrowserState>,
        raw_query: RawQuery,
    ) -> impl IntoResponse {
        let url = raw_query.0.unwrap_or_else(|| "about:blank".into());
        *state.page_url.lock().await = url.clone();
        Json(serde_json::json!({
            "id": "mock-page",
            "url": url,
            "webSocketDebuggerUrl": state.websocket_url,
        }))
    }

    async fn mock_page_ws(
        ws: WebSocketUpgrade,
        State(state): State<MockBrowserState>,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| handle_mock_page_ws(socket, state))
    }

    async fn handle_mock_page_ws(mut socket: WebSocket, state: MockBrowserState) {
        while let Some(Ok(message)) = socket.next().await {
            let WsMessage::Text(text) = message else {
                continue;
            };
            let frame: serde_json::Value = serde_json::from_str(&text).expect("frame should parse");
            let id = frame["id"].as_u64().expect("id should exist");
            let method = frame["method"].as_str().unwrap_or_default();
            let response = match method {
                "Page.navigate" => {
                    if let Some(url) = frame["params"]["url"].as_str() {
                        *state.page_url.lock().await = url.to_string();
                    }
                    serde_json::json!({ "id": id, "result": { "frameId": "1" } })
                }
                "Runtime.evaluate" => {
                    let expression = frame["params"]["expression"].as_str().unwrap_or_default();
                    let value = match expression {
                        "document.readyState" => serde_json::json!("complete"),
                        "document.title || ''" => serde_json::json!(state.title.lock().await.clone()),
                        "document.body ? document.body.innerText : ''" => serde_json::json!(state.text.lock().await.clone()),
                        "document.documentElement ? document.documentElement.outerHTML : ''" => serde_json::json!(state.html.lock().await.clone()),
                        "window.location.href" => serde_json::json!(state.page_url.lock().await.clone()),
                        expression if expression.contains(".click();") => {
                            *state.title.lock().await = "Clicked".into();
                            *state.text.lock().await = "Clicked submit".into();
                            serde_json::json!(true)
                        }
                        expression if expression.contains("dispatchEvent(new Event('input'") => {
                            let typed = expression
                                .split("el.value = ")
                                .nth(1)
                                .and_then(|value| value.split(';').next())
                                .and_then(|value| serde_json::from_str::<String>(value).ok())
                                .unwrap_or_default();
                            *state.title.lock().await = "Typed".into();
                            *state.text.lock().await = format!("Typed {typed}");
                            serde_json::json!(true)
                        }
                        _ => serde_json::json!(""),
                    };
                    let value_type = if value.is_boolean() { "boolean" } else { "string" };
                    serde_json::json!({
                        "id": id,
                        "result": {
                            "result": {
                                "type": value_type,
                                "value": value,
                            }
                        }
                    })
                }
                _ => serde_json::json!({ "id": id, "result": {} }),
            };
            let _ = socket
                .send(WsMessage::Text(response.to_string().into()))
                .await;
        }
    }
}
