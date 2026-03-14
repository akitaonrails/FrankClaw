#![forbid(unsafe_code)]

pub mod bash;
pub mod mcp;

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

use tokio::net::lookup_host;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::media::is_safe_ip;
use frankclaw_core::model::{ToolDef, ToolRiskLevel};
use frankclaw_core::session::SessionStore;
use frankclaw_core::types::{AgentId, SessionKey};

/// Maximum time to wait for a single CDP command response.
const CDP_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Maximum number of concurrent browser sessions.
const MAX_BROWSER_SESSIONS: usize = 10;

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
    policy: ToolPolicy,
}

/// What risk level the operator has approved for automatic tool execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApprovalLevel {
    /// Only read-only tools are auto-approved (default, most restrictive).
    #[default]
    ReadOnly,
    /// Read-only and mutating tools are auto-approved.
    Mutating,
    /// All tools are auto-approved (least restrictive).
    Destructive,
}

impl std::fmt::Display for ApprovalLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "readonly"),
            Self::Mutating => write!(f, "mutating"),
            Self::Destructive => write!(f, "destructive"),
        }
    }
}

impl ApprovalLevel {
    pub fn approves(&self, risk: ToolRiskLevel) -> bool {
        match risk {
            ToolRiskLevel::ReadOnly => true,
            ToolRiskLevel::Mutating => matches!(self, Self::Mutating | Self::Destructive),
            ToolRiskLevel::Destructive => matches!(self, Self::Destructive),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolPolicy {
    pub approval_level: ApprovalLevel,
    pub approved_tools: std::collections::HashSet<String>,
}

impl ToolRegistry {
    pub fn with_builtins() -> Self {
        Self::with_policy(ToolPolicy::from_env())
    }

    pub fn with_policy(policy: ToolPolicy) -> Self {
        let browser = Arc::new(
            BrowserClient::from_env()
                .unwrap_or_else(|_| BrowserClient::new("http://127.0.0.1:9222/").expect("default browser client should build")),
        );
        let mut registry = Self {
            tools: HashMap::new(),
            policy,
        };
        registry.register(Arc::new(SessionInspectTool));
        registry.register(Arc::new(BrowserOpenTool::new(browser.clone())));
        registry.register(Arc::new(BrowserExtractTool::new(browser.clone())));
        registry.register(Arc::new(BrowserSnapshotTool::new(browser.clone())));
        registry.register(Arc::new(BrowserClickTool::new(browser.clone())));
        registry.register(Arc::new(BrowserTypeTool::new(browser.clone())));
        registry.register(Arc::new(BrowserWaitTool::new(browser.clone())));
        registry.register(Arc::new(BrowserPressTool::new(browser.clone())));
        registry.register(Arc::new(BrowserSessionsTool::new(browser.clone())));
        registry.register(Arc::new(BrowserCloseTool::new(browser)));
        registry.register(Arc::new(bash::BashTool::from_env()));
        registry
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.definition().name, tool);
    }

    pub fn validate_names(&self, names: &[String]) -> Result<()> {
        for name in names {
            if !self.tools.contains_key(name) {
                return Err(FrankClawError::ConfigValidation {
                    msg: format!("unknown tool '{name}'"),
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
                msg: format!("unknown tool '{name}'"),
            })?;

        let risk_level = tool.definition().risk_level;
        if !self.policy.is_approved(name, risk_level) {
            return Err(FrankClawError::AgentRuntime {
                msg: format!(
                    "tool '{name}' requires {risk_level} approval. Set FRANKCLAW_TOOL_APPROVAL={risk_level} to enable.",
                ),
            });
        }

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

impl ToolPolicy {
    pub fn from_env() -> Self {
        let approval_level = if let Ok(value) = std::env::var("FRANKCLAW_TOOL_APPROVAL") {
            match value.trim().to_ascii_lowercase().as_str() {
                "mutating" => ApprovalLevel::Mutating,
                "destructive" => ApprovalLevel::Destructive,
                _ => ApprovalLevel::ReadOnly,
            }
        } else if truthy_env("FRANKCLAW_ALLOW_BROWSER_MUTATIONS") {
            // Backward compat: legacy env var maps to Mutating.
            ApprovalLevel::Mutating
        } else {
            ApprovalLevel::default()
        };

        Self {
            approval_level,
            approved_tools: std::collections::HashSet::new(),
        }
    }

    pub fn is_approved(&self, tool_name: &str, risk_level: ToolRiskLevel) -> bool {
        if self.approved_tools.contains(tool_name) {
            return true;
        }
        self.approval_level.approves(risk_level)
    }
}

fn truthy_env(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Returns the risk level assigned to a tool by name.
pub fn tool_risk_level(tool_name: &str) -> ToolRiskLevel {
    match tool_name {
        "browser.click" | "browser.type" | "browser.press" | "browser.select_option" | "bash" => {
            ToolRiskLevel::Mutating
        }
        _ => ToolRiskLevel::ReadOnly,
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

    #[expect(clippy::unused_self, reason = "method on BrowserClient for consistency with other session management methods")]
    fn session_count(&self, sessions: &HashMap<String, BrowserSession>) -> usize {
        sessions.len()
    }

    /// Remove a session from the registry when its CDP target is dead.
    async fn remove_dead_session(&self, session_id: &str) {
        self.sessions.lock().await.remove(session_id);
    }

    #[expect(clippy::unused_self, reason = "method on BrowserClient for consistency with other session management methods")]
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
        validate_navigation_url(url).await?;

        let existing = { self.sessions.lock().await.get(&session_id).cloned() };
        let session = if let Some(mut session) = existing {
            if matches!(self.navigate_target(&session, url).await, Ok(())) {
                session.current_url = url.to_string();
                session.last_updated_at = Utc::now();
                self.sessions
                    .lock()
                    .await
                    .insert(session_id.clone(), session.clone());
                session
            } else {
                // Existing session's target is dead — clean up and create fresh.
                self.remove_dead_session(&session_id).await;
                let target = self.create_target(url).await?;
                let now = Utc::now();
                let fresh = BrowserSession {
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
                    .insert(session_id.clone(), fresh.clone());
                fresh
            }
        } else {
            // Enforce concurrent session limit.
            {
                let sessions = self.sessions.lock().await;
                if self.session_count(&sessions) >= MAX_BROWSER_SESSIONS {
                    return Err(FrankClawError::AgentRuntime {
                        msg: format!(
                            "browser session limit reached ({MAX_BROWSER_SESSIONS}). Close existing sessions first."
                        ),
                    });
                }
            }
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
        };

        let snapshot = self.snapshot_session(&session).await?;
        let mut sessions = self.sessions.lock().await;
        if let Some(entry) = sessions.get_mut(&session_id) {
            entry.title.clone_from(&snapshot.title);
            entry.current_url.clone_from(&snapshot.url);
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
                msg: format!("browser session '{session_id}' was not opened yet"),
            })?;
        self.snapshot_session(&session).await
    }

    async fn list_sessions(&self) -> Vec<BrowserSession> {
        self.sessions.lock().await.values().cloned().collect()
    }

    async fn close(&self, session_id: &str) -> Result<()> {
        let session = self
            .sessions
            .lock()
            .await
            .remove(session_id)
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("browser session '{session_id}' was not opened yet"),
            })?;
        let endpoint = self
            .base_url
            .join(&format!("json/close/{}", session.target_id))
            .map_err(|err| FrankClawError::Internal {
                msg: format!("invalid browser close endpoint: {err}"),
            })?;
        let response = self
            .http
            .get(endpoint)
            .send()
            .await
            .map_err(|err| FrankClawError::AgentRuntime {
                msg: format!("failed to close browser target: {err}"),
            })?;
        if !response.status().is_success() {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("browser close failed with HTTP {}", response.status()),
            });
        }
        Ok(())
    }

    async fn click(&self, session_id: &str, selector: &str) -> Result<BrowserSnapshot> {
        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("browser session '{session_id}' was not opened yet"),
            })?;
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        self.wait_for_ready(&mut socket).await?;
        let clicked = self
            .evaluate_bool(&mut socket, &click_expression(selector))
            .await?;
        if !clicked {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("browser.click could not find selector '{selector}'"),
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
                msg: format!("browser session '{session_id}' was not opened yet"),
            })?;
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        self.wait_for_ready(&mut socket).await?;
        let typed = self
            .evaluate_bool(&mut socket, &type_expression(selector, text))
            .await?;
        if !typed {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("browser.type could not find selector '{selector}'"),
            });
        }
        self.snapshot_session(&session).await
    }

    async fn wait_for(
        &self,
        session_id: &str,
        selector: Option<&str>,
        text: Option<&str>,
        timeout_ms: u64,
    ) -> Result<BrowserSnapshot> {
        if selector.is_none() && text.is_none() {
            return Err(FrankClawError::InvalidRequest {
                msg: "browser.wait requires selector or text".into(),
            });
        }

        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("browser session '{session_id}' was not opened yet"),
            })?;
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        self.wait_for_ready(&mut socket).await?;

        let expression = wait_expression(selector, text);
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(timeout_ms.clamp(50, 30_000));
        loop {
            if self.evaluate_bool(&mut socket, &expression).await? {
                return self.snapshot_session(&session).await;
            }
            if std::time::Instant::now() >= deadline {
                let target = selector
                    .map(|value| format!("selector '{value}'"))
                    .or_else(|| text.map(|value| format!("text '{value}'")))
                    .unwrap_or_else(|| "condition".into());
                return Err(FrankClawError::AgentRuntime {
                    msg: format!("browser.wait timed out waiting for {target}"),
                });
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn press_key(&self, session_id: &str, selector: &str, key: &str) -> Result<BrowserSnapshot> {
        validate_press_key(key)?;
        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: format!("browser session '{session_id}' was not opened yet"),
            })?;
        let mut socket = self.connect_page_socket(&session.page_ws_url).await?;
        self.wait_for_ready(&mut socket).await?;
        let pressed = self
            .evaluate_bool(&mut socket, &press_expression(selector, key))
            .await?;
        if !pressed {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("browser.press could not find selector '{selector}'"),
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

        let read_response = async {
            while let Some(message) = socket.next().await {
                let message = message.map_err(|err| FrankClawError::AgentRuntime {
                    msg: format!("browser socket read failed: {err}"),
                })?;
                let Message::Text(text) = message else {
                    continue;
                };
                let frame: serde_json::Value =
                    serde_json::from_str(text.as_ref()).map_err(|err| {
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
        };

        match tokio::time::timeout(CDP_COMMAND_TIMEOUT, read_response).await {
            Ok(result) => result,
            Err(_) => Err(FrankClawError::AgentRuntime {
                msg: format!("browser command '{method}' timed out after {}s", CDP_COMMAND_TIMEOUT.as_secs()),
            }),
        }
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
struct BrowserWaitTool {
    client: Arc<BrowserClient>,
}
struct BrowserPressTool {
    client: Arc<BrowserClient>,
}
struct BrowserSessionsTool {
    client: Arc<BrowserClient>,
}
struct BrowserCloseTool {
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

impl BrowserWaitTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

impl BrowserPressTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

impl BrowserSessionsTool {
    fn new(client: Arc<BrowserClient>) -> Self {
        Self { client }
    }
}

impl BrowserCloseTool {
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
            risk_level: ToolRiskLevel::ReadOnly,
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
            .and_then(serde_json::Value::as_u64)
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
            risk_level: ToolRiskLevel::ReadOnly,
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
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        let max_chars = args
            .get("max_chars")
            .and_then(serde_json::Value::as_u64)
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
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        let max_chars = args
            .get("max_chars")
            .and_then(serde_json::Value::as_u64)
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
            risk_level: ToolRiskLevel::Mutating,
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
            risk_level: ToolRiskLevel::Mutating,
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

#[async_trait]
impl Tool for BrowserWaitTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.wait".into(),
            description: "Wait for a CSS selector or visible text to appear in an existing Chromium-backed browser session.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." },
                    "selector": { "type": "string", "description": "CSS selector that must resolve before continuing." },
                    "text": { "type": "string", "description": "Visible text snippet that must appear before continuing." },
                    "timeout_ms": { "type": "integer", "minimum": 50, "maximum": 30000, "description": "Maximum time to wait before failing." }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
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
            .filter(|value| !value.is_empty());
        let text = args
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(5_000);
        let snapshot = self
            .client
            .wait_for(&session_id, selector, text, timeout_ms)
            .await?;
        Ok(snapshot_result(snapshot, false, 1000))
    }
}

#[async_trait]
impl Tool for BrowserPressTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.press".into(),
            description: "Send one allowed keyboard key to a focused DOM element by CSS selector in an existing Chromium-backed browser session.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["selector", "key"],
                "properties": {
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." },
                    "selector": { "type": "string", "description": "CSS selector for the target element." },
                    "key": { "type": "string", "description": "Allowed key: Enter, Tab, Escape, ArrowDown, ArrowUp." }
                }
            }),
            risk_level: ToolRiskLevel::Mutating,
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
                msg: "browser.press requires a non-empty selector".into(),
            })?;
        let key = args
            .get("key")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "browser.press requires an allowed key".into(),
            })?;
        let snapshot = self.client.press_key(&session_id, selector, key).await?;
        Ok(snapshot_result(snapshot, false, 1000))
    }
}

#[async_trait]
impl Tool for BrowserSessionsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.sessions".into(),
            description: "List active Chromium-backed browser sessions tracked by FrankClaw.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, _args: serde_json::Value, _ctx: ToolContext) -> Result<serde_json::Value> {
        let sessions = self
            .client
            .list_sessions()
            .await
            .into_iter()
            .map(|session| serde_json::json!({
                "session_id": session.session_id,
                "target_id": session.target_id,
                "url": session.current_url,
                "title": session.title,
                "last_updated_at": session.last_updated_at,
            }))
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "sessions": sessions }))
    }
}

#[async_trait]
impl Tool for BrowserCloseTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "browser.close".into(),
            description: "Close a Chromium-backed browser session and its DevTools target.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional browser session identifier. Defaults to the current chat session." }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let session_id = self
            .client
            .resolve_session_id(args.get("session_id").and_then(|value| value.as_str()), &ctx)?;
        self.client.close(&session_id).await?;
        Ok(serde_json::json!({
            "session_id": session_id,
            "closed": true,
        }))
    }
}

#[expect(clippy::needless_pass_by_value, reason = "snapshot is consumed to build the JSON result")]
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

fn wait_expression(selector: Option<&str>, text: Option<&str>) -> String {
    let selector = selector.map_or_else(|| "null".into(), |value| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()));
    let text = text.map_or_else(|| "null".into(), |value| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()));
    format!(
        "(function() {{ const selector = {selector}; const text = {text}; const hasSelector = !selector || !!document.querySelector(selector); const bodyText = document.body ? document.body.innerText : ''; const hasText = !text || bodyText.includes(text); return hasSelector && hasText; }})()"
    )
}

fn press_expression(selector: &str, key: &str) -> String {
    let selector = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into());
    let key = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(function() {{ const el = document.querySelector({selector}); if (!el) return false; el.focus(); for (const type of ['keydown', 'keypress', 'keyup']) {{ el.dispatchEvent(new KeyboardEvent(type, {{ key: {key}, bubbles: true }})); }} return true; }})()"
    )
}

fn validate_press_key(key: &str) -> Result<()> {
    match key {
        "Enter" | "Tab" | "Escape" | "ArrowDown" | "ArrowUp" => Ok(()),
        _ => Err(FrankClawError::InvalidRequest {
            msg: format!(
                "browser.press only allows Enter, Tab, Escape, ArrowDown, and ArrowUp; got '{key}'"
            ),
        }),
    }
}

/// Validate that a browser navigation URL is not targeting private/internal IPs.
/// Uses the same SSRF blocklist as media fetches.
async fn validate_navigation_url(raw_url: &str) -> Result<()> {
    let parsed = Url::parse(raw_url).map_err(|err| FrankClawError::InvalidRequest {
        msg: format!("invalid browser navigation URL: {err}"),
    })?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(FrankClawError::InvalidRequest {
            msg: format!("browser navigation only allows http/https URLs, got '{scheme}'"),
        });
    }
    let host = parsed.host_str().ok_or_else(|| FrankClawError::InvalidRequest {
        msg: "browser navigation URL has no host".into(),
    })?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let lookup = format!("{host}:{port}");
    let addrs: Vec<_> = lookup_host(&lookup)
        .await
        .map_err(|err| FrankClawError::AgentRuntime {
            msg: format!("DNS lookup failed for browser navigation URL '{host}': {err}"),
        })?
        .collect();
    if addrs.is_empty() {
        return Err(FrankClawError::AgentRuntime {
            msg: format!("DNS lookup returned no addresses for '{host}'"),
        });
    }
    for addr in &addrs {
        if !is_safe_ip(&addr.ip()) {
            return Err(FrankClawError::InvalidRequest {
                msg: format!(
                    "browser navigation blocked: '{host}' resolves to private/internal IP {}",
                    addr.ip()
                ),
            });
        }
    }
    Ok(())
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
    async fn browser_mutation_tools_require_explicit_policy() {
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
            .route("/json/close/{target_id}", get(mock_close_target))
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
            policy: ToolPolicy::default(),
        };
        registry.register(Arc::new(BrowserOpenTool::new(client.clone())));
        registry.register(Arc::new(BrowserClickTool::new(client)));

        let ctx = ToolContext {
            agent_id: AgentId::default_agent(),
            session_key: Some(SessionKey::from_raw("default:web:browser-policy")),
            sessions: Arc::new(MockSessionStore::default()),
        };
        let allowed = vec!["browser.open".into(), "browser.click".into()];

        registry
            .invoke_allowed(
                &allowed,
                "browser.open",
                serde_json::json!({ "url": "https://example.com/" }),
                ctx.clone(),
            )
            .await
            .expect("browser.open should stay allowed");

        let err = registry
            .invoke_allowed(
                &allowed,
                "browser.click",
                serde_json::json!({ "selector": "#submit" }),
                ctx,
            )
            .await
            .expect_err("browser.click should require explicit approval");
        assert!(err
            .to_string()
            .contains("requires mutating approval"));
    }

    #[test]
    fn approval_level_readonly_approves_only_readonly() {
        let level = ApprovalLevel::ReadOnly;
        assert!(level.approves(ToolRiskLevel::ReadOnly));
        assert!(!level.approves(ToolRiskLevel::Mutating));
        assert!(!level.approves(ToolRiskLevel::Destructive));
    }

    #[test]
    fn approval_level_mutating_approves_readonly_and_mutating() {
        let level = ApprovalLevel::Mutating;
        assert!(level.approves(ToolRiskLevel::ReadOnly));
        assert!(level.approves(ToolRiskLevel::Mutating));
        assert!(!level.approves(ToolRiskLevel::Destructive));
    }

    #[test]
    fn approval_level_destructive_approves_all() {
        let level = ApprovalLevel::Destructive;
        assert!(level.approves(ToolRiskLevel::ReadOnly));
        assert!(level.approves(ToolRiskLevel::Mutating));
        assert!(level.approves(ToolRiskLevel::Destructive));
    }

    #[test]
    fn policy_approved_tools_override_level() {
        let policy = ToolPolicy {
            approval_level: ApprovalLevel::ReadOnly,
            approved_tools: std::collections::HashSet::from(["browser.click".into()]),
        };
        assert!(policy.is_approved("browser.click", ToolRiskLevel::Mutating));
        assert!(!policy.is_approved("browser.type", ToolRiskLevel::Mutating));
        assert!(policy.is_approved("browser.extract", ToolRiskLevel::ReadOnly));
    }

    #[test]
    fn tool_risk_level_classification() {
        use frankclaw_core::model::ToolRiskLevel;
        assert_eq!(tool_risk_level("browser.click"), ToolRiskLevel::Mutating);
        assert_eq!(tool_risk_level("browser.type"), ToolRiskLevel::Mutating);
        assert_eq!(tool_risk_level("browser.press"), ToolRiskLevel::Mutating);
        assert_eq!(tool_risk_level("bash"), ToolRiskLevel::Mutating);
        assert_eq!(tool_risk_level("browser.open"), ToolRiskLevel::ReadOnly);
        assert_eq!(tool_risk_level("browser.extract"), ToolRiskLevel::ReadOnly);
        assert_eq!(tool_risk_level("session.inspect"), ToolRiskLevel::ReadOnly);
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
            .route("/json/close/{target_id}", get(mock_close_target))
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
            policy: ToolPolicy {
                approval_level: ApprovalLevel::Mutating,
                approved_tools: std::collections::HashSet::new(),
            },
        };
        registry.register(Arc::new(BrowserOpenTool::new(client.clone())));
        registry.register(Arc::new(BrowserExtractTool::new(client.clone())));
        registry.register(Arc::new(BrowserSnapshotTool::new(client.clone())));
        registry.register(Arc::new(BrowserClickTool::new(client.clone())));
        registry.register(Arc::new(BrowserTypeTool::new(client.clone())));
        registry.register(Arc::new(BrowserWaitTool::new(client.clone())));
        registry.register(Arc::new(BrowserPressTool::new(client.clone())));
        registry.register(Arc::new(BrowserSessionsTool::new(client.clone())));
        registry.register(Arc::new(BrowserCloseTool::new(client)));

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
            "browser.wait".into(),
            "browser.press".into(),
            "browser.sessions".into(),
            "browser.close".into(),
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

        let waited = registry
            .invoke_allowed(
                &allowed,
                "browser.wait",
                serde_json::json!({ "text": "Typed frankclaw", "timeout_ms": 250 }),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(SessionKey::from_raw("default:web:browser")),
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect("browser.wait should succeed");
        assert_eq!(waited.output["title"], serde_json::json!("Typed"));

        let pressed = registry
            .invoke_allowed(
                &allowed,
                "browser.press",
                serde_json::json!({ "selector": "#query", "key": "Enter" }),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(SessionKey::from_raw("default:web:browser")),
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect("browser.press should succeed");
        assert!(
            pressed.output["text"]
                .as_str()
                .expect("text should exist")
                .contains("Pressed Enter")
        );

        let sessions = registry
            .invoke_allowed(
                &allowed,
                "browser.sessions",
                serde_json::json!({}),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(SessionKey::from_raw("default:web:browser")),
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect("browser.sessions should succeed");
        assert_eq!(sessions.output["sessions"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            sessions.output["sessions"][0]["session_id"],
            serde_json::json!("session:default:web:browser")
        );

        let closed = registry
            .invoke_allowed(
                &allowed,
                "browser.close",
                serde_json::json!({}),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(SessionKey::from_raw("default:web:browser")),
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect("browser.close should succeed");
        assert_eq!(closed.output["closed"], serde_json::json!(true));

        let sessions_after_close = registry
            .invoke_allowed(
                &allowed,
                "browser.sessions",
                serde_json::json!({}),
                ToolContext {
                    agent_id: AgentId::default_agent(),
                    session_key: Some(SessionKey::from_raw("default:web:browser")),
                    sessions: Arc::new(MockSessionStore::default()),
                },
            )
            .await
            .expect("browser.sessions should still succeed");
        assert_eq!(
            sessions_after_close.output["sessions"].as_array().map(Vec::len),
            Some(0)
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
                          query.addEventListener("keydown", (event) => {
                            if (event.key === "Enter") {
                              document.title = "Pressed";
                              status.textContent = "Pressed " + event.key + " " + query.value;
                            }
                          });
                          document.getElementById("submit").addEventListener("click", () => {
                            document.title = "Clicked";
                            status.textContent = "Clicked " + query.value;
                          });
                          setTimeout(() => {
                            document.body.dataset.ready = "1";
                            status.textContent = "Loaded";
                          }, 150);
                        </script>
                      </body>
                    </html>"#,
                )
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("real browser test server should run");
        });

        let registry = ToolRegistry::with_policy(ToolPolicy {
            approval_level: ApprovalLevel::Mutating,
            approved_tools: std::collections::HashSet::new(),
        });
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
            "browser.wait".into(),
            "browser.press".into(),
            "browser.sessions".into(),
            "browser.close".into(),
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

        let waited = registry
            .invoke_allowed(
                &allowed,
                "browser.wait",
                serde_json::json!({ "selector": "body[data-ready='1']", "text": "Loaded", "timeout_ms": 2_000 }),
                ctx.clone(),
            )
            .await
            .expect("browser.wait should succeed against real chromium");
        assert!(
            waited.output["text"]
                .as_str()
                .expect("text should exist")
                .contains("Loaded")
        );

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

        let pressed = registry
            .invoke_allowed(
                &allowed,
                "browser.press",
                serde_json::json!({ "selector": "#query", "key": "Enter" }),
                ctx.clone(),
            )
            .await
            .expect("browser.press should succeed against real chromium");
        assert_eq!(pressed.output["title"], serde_json::json!("Pressed"));
        assert!(
            pressed.output["text"]
                .as_str()
                .expect("text should exist")
                .contains("Pressed Enter frankclaw")
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
                ctx.clone(),
            )
            .await
            .expect("browser.snapshot should succeed against real chromium");
        assert!(
            snapshot.output["html"]
                .as_str()
                .expect("html should exist")
                .contains("id=\"status\"")
        );

        let sessions = registry
            .invoke_allowed(
                &allowed,
                "browser.sessions",
                serde_json::json!({}),
                ctx.clone(),
            )
            .await
            .expect("browser.sessions should succeed against real chromium");
        assert_eq!(sessions.output["sessions"].as_array().map(Vec::len), Some(1));

        let closed = registry
            .invoke_allowed(
                &allowed,
                "browser.close",
                serde_json::json!({}),
                ctx.clone(),
            )
            .await
            .expect("browser.close should succeed against real chromium");
        assert_eq!(closed.output["closed"], serde_json::json!(true));

        let sessions_after_close = registry
            .invoke_allowed(
                &allowed,
                "browser.sessions",
                serde_json::json!({}),
                ctx,
            )
            .await
            .expect("browser.sessions should still succeed against real chromium");
        assert_eq!(
            sessions_after_close.output["sessions"].as_array().map(Vec::len),
            Some(0)
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

    async fn mock_close_target(axum::extract::Path(target_id): axum::extract::Path<String>) -> impl IntoResponse {
        (
            axum::http::StatusCode::OK,
            format!("Target is closing: {target_id}"),
        )
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
                        expression if expression.contains("new KeyboardEvent") => {
                            *state.title.lock().await = "Pressed".into();
                            *state.text.lock().await = "Pressed Enter".into();
                            serde_json::json!(true)
                        }
                        expression if expression.contains("const selector = ") => {
                            let text = state.text.lock().await.clone();
                            serde_json::json!(text.contains("Typed frankclaw"))
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

    #[tokio::test]
    async fn navigation_ssrf_blocks_private_ips() {
        // Loopback
        let err = validate_navigation_url("http://127.0.0.1/secret").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("private/internal"));

        // Private network
        let err = validate_navigation_url("http://192.168.1.1/admin").await;
        assert!(err.is_err());

        let err = validate_navigation_url("http://10.0.0.1/").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn navigation_ssrf_blocks_non_http_schemes() {
        let err = validate_navigation_url("file:///etc/passwd").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("http/https"));

        let err = validate_navigation_url("javascript:alert(1)").await;
        assert!(err.is_err());

        let err = validate_navigation_url("ftp://evil.com/file").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn navigation_ssrf_blocks_urls_without_host() {
        let err = validate_navigation_url("http://").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn session_limit_enforcement() {
        let client = BrowserClient::new("http://127.0.0.1:19999/")
            .expect("browser client should build");
        {
            let mut sessions = client.sessions.lock().await;
            for i in 0..MAX_BROWSER_SESSIONS {
                sessions.insert(
                    format!("session-{i}"),
                    BrowserSession {
                        session_id: format!("session-{i}"),
                        target_id: format!("target-{i}"),
                        page_ws_url: "ws://127.0.0.1:19999/devtools/page/x".into(),
                        current_url: "https://example.com/".into(),
                        title: None,
                        last_updated_at: Utc::now(),
                    },
                );
            }
            assert_eq!(client.session_count(&sessions), MAX_BROWSER_SESSIONS);
        }
        // Opening a new session should fail with limit error.
        let err = client
            .open("new-session".into(), "https://example.com/")
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("session limit reached"));
    }

    #[tokio::test]
    async fn cdp_command_timeout_fires() {
        // Start a WS server that accepts connections but never responds.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.expect("accept");
                tokio::spawn(async move {
                    let ws = tokio_tungstenite::accept_async(stream)
                        .await
                        .expect("ws handshake");
                    // Hold the connection open but never send anything.
                    let (_sink, mut stream) = ws.split();
                    while stream.next().await.is_some() {}
                });
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = BrowserClient::new(&format!("http://{addr}/"))
            .expect("browser client should build");
        let ws_url = format!("ws://{addr}/");
        let mut socket = client
            .connect_page_socket(&ws_url)
            .await
            .expect("should connect");

        // Override the timeout constant behavior by using a short timeout directly.
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            client.send_command(&mut socket, "Runtime.evaluate", serde_json::json!({"expression": "1+1"})),
        )
        .await;
        // Either our internal CDP_COMMAND_TIMEOUT or our test timeout should fire.
        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[tokio::test]
    async fn dead_session_recovery_on_open() {
        // If a session's target is dead, opening it again should replace it.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let mock_state = MockBrowserState {
            page_url: Arc::new(Mutex::new("about:blank".into())),
            title: Arc::new(Mutex::new("Fresh".into())),
            text: Arc::new(Mutex::new("Fresh page".into())),
            html: Arc::new(Mutex::new("<html><body>Fresh</body></html>".into())),
            websocket_url: format!("ws://127.0.0.1:{}/devtools/page/mock-page", addr.port()),
        };
        let app = Router::new()
            .route("/json/new", put(mock_create_target))
            .route("/json/close/{target_id}", get(mock_close_target))
            .route("/devtools/page/mock-page", get(mock_page_ws))
            .with_state(mock_state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        let client = BrowserClient::new(&format!("http://{addr}/"))
            .expect("browser client should build");

        // Pre-populate a dead session with a bogus WS URL.
        {
            let mut sessions = client.sessions.lock().await;
            sessions.insert(
                "session:test".into(),
                BrowserSession {
                    session_id: "session:test".into(),
                    target_id: "dead-target".into(),
                    page_ws_url: "ws://127.0.0.1:1/dead".into(),
                    current_url: "https://old.example.com/".into(),
                    title: None,
                    last_updated_at: Utc::now(),
                },
            );
        }

        // Opening should detect the dead session, clean it up, and create fresh.
        let snapshot = client
            .open("session:test".into(), "https://example.com/")
            .await
            .expect("open should recover from dead session");
        assert_eq!(snapshot.title.as_deref(), Some("Fresh"));
    }

    #[test]
    fn press_key_validation_rejects_unknown_keys() {
        assert!(validate_press_key("Enter").is_ok());
        assert!(validate_press_key("Tab").is_ok());
        assert!(validate_press_key("Escape").is_ok());
        assert!(validate_press_key("ArrowDown").is_ok());
        assert!(validate_press_key("ArrowUp").is_ok());
        assert!(validate_press_key("Delete").is_err());
        assert!(validate_press_key("F1").is_err());
        assert!(validate_press_key("a").is_err());
    }

    #[test]
    fn truncate_chars_works() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello world", 5), "hello...");
        assert_eq!(truncate_chars("", 5), "");
    }
}
