use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::auth::{AuthMode, RateLimitConfig};
use crate::session::{PruningConfig, SessionResetPolicy, SessionScoping};
use crate::types::{AgentId, ChannelId};

/// Top-level FrankClaw configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FrankClawConfig {
    pub gateway: GatewayConfig,
    pub agents: AgentsConfig,
    pub channels: HashMap<ChannelId, ChannelConfig>,
    pub models: ModelsConfig,
    pub session: SessionConfig,
    pub cron: CronConfig,
    pub hooks: HooksConfig,
    pub logging: LoggingConfig,
    pub media: MediaConfig,
    pub security: SecurityConfig,
}

impl Default for FrankClawConfig {
    fn default() -> Self {
        Self {
            gateway: GatewayConfig::default(),
            agents: AgentsConfig::default(),
            channels: HashMap::new(),
            models: ModelsConfig::default(),
            session: SessionConfig::default(),
            cron: CronConfig::default(),
            hooks: HooksConfig::default(),
            logging: LoggingConfig::default(),
            media: MediaConfig::default(),
            security: SecurityConfig::default(),
        }
    }
}

/// Gateway network configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    /// TCP port to listen on.
    pub port: u16,
    /// Bind address. "loopback" (default), "lan", or a specific IP.
    pub bind: BindMode,
    /// Authentication mode.
    pub auth: AuthMode,
    /// Rate limiting for auth failures.
    pub rate_limit: RateLimitConfig,
    /// Enable TLS. Auto-generates self-signed cert if no cert path given.
    pub tls: Option<TlsConfig>,
    /// Maximum WebSocket message size in bytes.
    pub max_ws_message_bytes: usize,
    /// Maximum concurrent WebSocket connections.
    pub max_connections: usize,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            port: 18789,
            bind: BindMode::Loopback,
            auth: AuthMode::None,
            rate_limit: RateLimitConfig::default(),
            tls: None,
            max_ws_message_bytes: 4 * 1024 * 1024, // 4 MB
            max_connections: 64,
        }
    }
}

/// How to bind the listening socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindMode {
    /// 127.0.0.1 only (safest default).
    Loopback,
    /// 0.0.0.0 (LAN accessible). Requires auth.
    Lan,
    /// Specific address.
    Address(String),
}

impl Default for BindMode {
    fn default() -> Self {
        Self::Loopback
    }
}

/// TLS configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

/// Agent configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub default_agent: AgentId,
    pub agents: HashMap<AgentId, AgentDef>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        let mut agents = HashMap::new();
        agents.insert(
            AgentId::default_agent(),
            AgentDef::default(),
        );
        Self {
            default_agent: AgentId::default_agent(),
            agents,
        }
    }
}

/// Definition of a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentDef {
    pub name: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub workspace: Option<PathBuf>,
    pub sandbox: SandboxConfig,
    pub tools: Vec<String>,
    pub skills: Vec<String>,
}

impl Default for AgentDef {
    fn default() -> Self {
        Self {
            name: "Default Agent".to_string(),
            model: None,
            system_prompt: None,
            workspace: None,
            sandbox: SandboxConfig::default(),
            tools: vec![],
            skills: vec![],
        }
    }
}

/// Sandbox configuration for agent code execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SandboxConfig {
    None,
    Docker {
        image: String,
        #[serde(default = "default_sandbox_memory")]
        memory_limit_mb: u64,
        #[serde(default = "default_sandbox_timeout")]
        timeout_secs: u64,
    },
    Podman {
        image: String,
        #[serde(default = "default_sandbox_memory")]
        memory_limit_mb: u64,
        #[serde(default = "default_sandbox_timeout")]
        timeout_secs: u64,
    },
    Bubblewrap {
        #[serde(default)]
        network: bool,
        #[serde(default = "default_sandbox_timeout")]
        timeout_secs: u64,
    },
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self::None
    }
}

fn default_sandbox_memory() -> u64 {
    512
}
fn default_sandbox_timeout() -> u64 {
    300
}

/// Per-channel configuration (opaque — channel plugins parse their own section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub enabled: bool,
    #[serde(default)]
    pub accounts: Vec<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Model provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    pub providers: Vec<ProviderConfig>,
    pub default_model: Option<String>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            providers: vec![],
            default_model: None,
        }
    }
}

/// Configuration for a model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub api: String,
    pub base_url: Option<String>,
    /// Reference to API key (env var name or secret ref).
    pub api_key_ref: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub cooldown_secs: u64,
}

/// Session defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub scoping: SessionScoping,
    pub reset: SessionResetPolicy,
    pub pruning: PruningConfig,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            scoping: SessionScoping::default(),
            reset: SessionResetPolicy::default(),
            pruning: PruningConfig::default(),
        }
    }
}

/// Cron defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CronConfig {
    pub enabled: bool,
    pub jobs: Vec<serde_json::Value>,
}

/// Hooks configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    pub enabled: bool,
    pub token: Option<String>,
    pub max_body_bytes: Option<usize>,
    pub mappings: Vec<serde_json::Value>,
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub format: LogFormat,
    /// Redact sensitive values in logs.
    pub redact_secrets: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: LogFormat::Pretty,
            redact_secrets: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Pretty,
    Json,
    Compact,
}

/// Media pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    pub max_file_size_bytes: u64,
    pub ttl_hours: u64,
    pub storage_path: Option<PathBuf>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_file_size_bytes: 5 * 1024 * 1024, // 5 MB
            ttl_hours: 2,
            storage_path: None,
        }
    }
}

/// Security hardening options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Encrypt sessions at rest.
    pub encrypt_sessions: bool,
    /// Encrypt media files at rest.
    pub encrypt_media: bool,
    /// Require authentication for LAN/public bind modes.
    /// This is ALWAYS true and cannot be disabled.
    #[serde(skip_deserializing)]
    pub require_auth_for_network: bool,
    /// Block SSRF to private IP ranges.
    pub ssrf_protection: bool,
    /// Maximum request body size for webhooks (bytes).
    pub max_webhook_body_bytes: usize,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            encrypt_sessions: true,
            encrypt_media: false,
            require_auth_for_network: true, // Cannot be disabled
            ssrf_protection: true,
            max_webhook_body_bytes: 1024 * 1024, // 1 MB
        }
    }
}
