use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::auth::{AuthMode, RateLimitConfig};
use crate::error::{FrankClawError, Result};
use crate::session::{PruningConfig, SessionResetPolicy, SessionScoping};
use crate::types::{AgentId, ChannelId, SessionKey};

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
    pub memory: MemoryConfig,
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
            memory: MemoryConfig::default(),
        }
    }
}

impl FrankClawConfig {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|err| FrankClawError::ConfigIo {
            msg: format!("failed to read config '{}': {err}", path.display()),
        })?;
        serde_json::from_str(&content).map_err(|err| FrankClawError::ConfigIo {
            msg: format!("failed to parse config '{}': {err}", path.display()),
        })
    }

    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::load_from_path(path)
    }

    pub fn validate(&self) -> Result<()> {
        self.gateway.auth.validate()?;

        if !self.agents.agents.contains_key(&self.agents.default_agent) {
            return Err(FrankClawError::ConfigValidation {
                msg: format!(
                    "default agent '{}' is not present in agents map",
                    self.agents.default_agent
                ),
            });
        }

        let mut provider_ids = std::collections::HashSet::new();
        for provider in &self.models.providers {
            if provider.id.trim().is_empty() {
                return Err(FrankClawError::ConfigValidation {
                    msg: "model provider id cannot be empty".into(),
                });
            }
            if !provider_ids.insert(provider.id.clone()) {
                return Err(FrankClawError::ConfigValidation {
                    msg: format!("duplicate model provider id '{}'", provider.id),
                });
            }
            match provider.api.as_str() {
                "openai" | "anthropic" | "ollama" => {}
                other => {
                    return Err(FrankClawError::ConfigValidation {
                        msg: format!(
                            "unsupported model provider api '{}'; expected openai, anthropic, or ollama",
                            other
                        ),
                    });
                }
            }
            if matches!(provider.api.as_str(), "openai" | "anthropic")
                && provider
                    .api_key_ref
                    .as_deref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
            {
                return Err(FrankClawError::ConfigValidation {
                    msg: format!(
                        "provider '{}' requires a non-empty api_key_ref",
                        provider.id
                    ),
                });
            }
        }

        if let Some(default_model) = &self.models.default_model {
            if default_model.trim().is_empty() {
                return Err(FrankClawError::ConfigValidation {
                    msg: "models.default_model cannot be empty".into(),
                });
            }
        }

        if self.gateway.max_connections == 0 {
            return Err(FrankClawError::ConfigValidation {
                msg: "gateway.max_connections must be greater than 0".into(),
            });
        }

        if self.gateway.max_ws_message_bytes == 0 {
            return Err(FrankClawError::ConfigValidation {
                msg: "gateway.max_ws_message_bytes must be greater than 0".into(),
            });
        }

        if let BindMode::Address(address) = &self.gateway.bind {
            if address.parse::<std::net::IpAddr>().is_err() {
                return Err(FrankClawError::ConfigValidation {
                    msg: format!(
                        "gateway.bind address '{}' is not a valid IP address",
                        address
                    ),
                });
            }
        }

        for (channel_id, channel) in &self.channels {
            channel.security_policy()?;
            validate_channel_config(channel_id, channel)?;
        }

        self.hooks.parsed_mappings()?;

        Ok(())
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
    /// Channel health check interval in seconds. 0 disables monitoring.
    /// Default: 300 (5 minutes).
    pub health_check_interval_secs: Option<u64>,
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
            health_check_interval_secs: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelDmPolicy {
    Open,
    Allowlist,
    Pairing,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSecurityPolicy {
    pub dm_policy: ChannelDmPolicy,
    pub allow_from: Vec<String>,
    pub allowed_groups: Option<Vec<String>>,
    pub require_mention_for_groups: bool,
    pub max_message_bytes: Option<usize>,
}

impl Default for ChannelSecurityPolicy {
    fn default() -> Self {
        Self {
            dm_policy: ChannelDmPolicy::Pairing,
            allow_from: Vec::new(),
            allowed_groups: None,
            require_mention_for_groups: true,
            max_message_bytes: None,
        }
    }
}

impl ChannelConfig {
    pub fn security_policy(&self) -> Result<ChannelSecurityPolicy> {
        let mut policy = ChannelSecurityPolicy::default();

        if let Some(raw) = self.extra.get("dm_policy").and_then(|value| value.as_str()) {
            policy.dm_policy = match raw {
                "open" => ChannelDmPolicy::Open,
                "allowlist" => ChannelDmPolicy::Allowlist,
                "pairing" => ChannelDmPolicy::Pairing,
                "disabled" => ChannelDmPolicy::Disabled,
                other => {
                    return Err(FrankClawError::ConfigValidation {
                        msg: format!(
                            "invalid dm_policy '{}'; expected open, allowlist, pairing, or disabled",
                            other
                        ),
                    });
                }
            };
        }

        if let Some(raw) = self.extra.get("allow_from") {
            let entries = raw.as_array().ok_or_else(|| FrankClawError::ConfigValidation {
                msg: "allow_from must be an array of sender ids".into(),
            })?;
            policy.allow_from = entries
                .iter()
                .map(|entry| {
                    entry.as_str().map(str::to_string).ok_or_else(|| {
                        FrankClawError::ConfigValidation {
                            msg: "allow_from entries must be strings".into(),
                        }
                    })
                })
                .collect::<Result<Vec<_>>>()?;
        }

        if let Some(raw) = self.extra.get("groups") {
            let entries = raw.as_array().ok_or_else(|| FrankClawError::ConfigValidation {
                msg: "groups must be an array of group or thread ids".into(),
            })?;
            policy.allowed_groups = Some(
                entries
                    .iter()
                    .map(|entry| {
                        entry.as_str().map(str::to_string).ok_or_else(|| {
                            FrankClawError::ConfigValidation {
                                msg: "groups entries must be strings".into(),
                            }
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
        }

        if let Some(raw) = self
            .extra
            .get("require_mention_for_groups")
            .and_then(|value| value.as_bool())
        {
            policy.require_mention_for_groups = raw;
        }

        if let Some(raw) = self.extra.get("max_message_bytes") {
            let value = raw.as_u64().ok_or_else(|| FrankClawError::ConfigValidation {
                msg: "max_message_bytes must be a positive integer".into(),
            })? as usize;
            if value == 0 {
                return Err(FrankClawError::ConfigValidation {
                    msg: "max_message_bytes must be greater than 0".into(),
                });
            }
            policy.max_message_bytes = Some(value);
        }

        Ok(policy)
    }
}

fn validate_channel_config(channel_id: &ChannelId, channel: &ChannelConfig) -> Result<()> {
    if !channel.enabled {
        return Ok(());
    }

    match channel_id.as_str() {
        "web" => Ok(()),
        "telegram" => validate_channel_account_value_source(
            channel,
            "telegram",
            &["bot_token", "token"],
            &["bot_token_env", "token_env"],
            "bot token",
        ),
        "discord" => validate_channel_account_value_source(
            channel,
            "discord",
            &["bot_token", "token"],
            &["bot_token_env", "token_env"],
            "bot token",
        ),
        "signal" => validate_channel_account_value_source(
            channel,
            "signal",
            &["base_url", "http_url"],
            &["base_url_env", "http_url_env"],
            "base URL",
        ),
        "whatsapp" => {
            validate_channel_account_value_source(
                channel,
                "whatsapp",
                &["access_token", "token"],
                &["access_token_env", "token_env"],
                "access token",
            )?;
            validate_channel_account_value_source(
                channel,
                "whatsapp",
                &["phone_number_id"],
                &["phone_number_id_env"],
                "phone number id",
            )?;
            validate_channel_account_value_source(
                channel,
                "whatsapp",
                &["verify_token"],
                &["verify_token_env"],
                "verify token",
            )
        }
        "slack" => {
            validate_channel_account_value_source(
                channel,
                "slack",
                &["app_token"],
                &["app_token_env"],
                "app token",
            )?;
            validate_channel_account_value_source(
                channel,
                "slack",
                &["bot_token", "token"],
                &["bot_token_env", "token_env"],
                "bot token",
            )
        }
        other => Err(FrankClawError::ConfigValidation {
            msg: format!(
                "unsupported enabled channel '{}'; currently supported: web, telegram, discord, signal, slack, whatsapp",
                other
            ),
        }),
    }
}

fn validate_channel_account_value_source(
    channel: &ChannelConfig,
    channel_name: &str,
    inline_keys: &[&str],
    env_keys: &[&str],
    label: &str,
) -> Result<()> {
    let account = channel.accounts.first().ok_or_else(|| FrankClawError::ConfigValidation {
        msg: format!("{channel_name} channel requires at least one account"),
    })?;

    let has_inline_secret = inline_keys.iter().any(|key| {
        account
            .get(*key)
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    });
    if has_inline_secret {
        return Ok(());
    }

    let has_env_secret = env_keys.iter().any(|key| {
        account
            .get(*key)
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    });
    if has_env_secret {
        return Ok(());
    }

    Err(FrankClawError::ConfigValidation {
        msg: format!(
            "{channel_name} channel requires a non-empty {label} or {label} env reference"
        ),
    })
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

impl HooksConfig {
    pub fn parsed_mappings(&self) -> Result<Vec<WebhookMapping>> {
        if !self.enabled {
            return Ok(Vec::new());
        }
        if self
            .token
            .as_deref()
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            return Err(FrankClawError::ConfigValidation {
                msg: "hooks.enabled requires a non-empty hooks.token".into(),
            });
        }
        if self.mappings.is_empty() {
            return Err(FrankClawError::ConfigValidation {
                msg: "hooks.enabled requires at least one mapping".into(),
            });
        }

        let mut seen = std::collections::HashSet::new();
        let mut mappings = Vec::with_capacity(self.mappings.len());
        for raw in &self.mappings {
            let mapping: WebhookMapping = serde_json::from_value(raw.clone()).map_err(|err| {
                FrankClawError::ConfigValidation {
                    msg: format!("invalid webhook mapping: {err}"),
                }
            })?;
            if mapping.id.trim().is_empty() {
                return Err(FrankClawError::ConfigValidation {
                    msg: "webhook mapping id cannot be empty".into(),
                });
            }
            if !seen.insert(mapping.id.clone()) {
                return Err(FrankClawError::ConfigValidation {
                    msg: format!("duplicate webhook mapping '{}'", mapping.id),
                });
            }
            if mapping.text_field.trim().is_empty() {
                return Err(FrankClawError::ConfigValidation {
                    msg: format!("webhook mapping '{}' text_field cannot be empty", mapping.id),
                });
            }
            if let (Some(agent_id), Some(session_key)) = (&mapping.agent_id, &mapping.session_key) {
                if let Some((session_agent, _, _)) = session_key.parse() {
                    if &session_agent != agent_id {
                        return Err(FrankClawError::ConfigValidation {
                            msg: format!(
                                "webhook mapping '{}' session '{}' does not belong to agent '{}'",
                                mapping.id, session_key, agent_id
                            ),
                        });
                    }
                }
            }
            mappings.push(mapping);
        }
        Ok(mappings)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookMapping {
    pub id: String,
    pub agent_id: Option<AgentId>,
    pub session_key: Option<SessionKey>,
    pub text_field: String,
}

impl Default for WebhookMapping {
    fn default() -> Self {
        Self {
            id: String::new(),
            agent_id: None,
            session_key: None,
            text_field: "message".into(),
        }
    }
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

/// Memory/RAG configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub enabled: bool,
    /// Embedding provider: "openai", "ollama", or "none".
    pub embedding_provider: String,
    /// Embedding model name (e.g., "text-embedding-3-small").
    pub embedding_model: Option<String>,
    /// Base URL for the embedding provider API.
    pub embedding_base_url: Option<String>,
    /// API key env var for embedding provider.
    pub embedding_api_key_ref: Option<String>,
    /// Chunk size target in characters (~384 tokens).
    pub chunk_size: usize,
    /// Directory to sync for memory content.
    pub memory_dir: Option<PathBuf>,
    /// Enable embedding cache.
    pub cache_embeddings: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding_provider: "none".into(),
            embedding_model: None,
            embedding_base_url: None,
            embedding_api_key_ref: None,
            chunk_size: 1500,
            memory_dir: None,
            cache_embeddings: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_provider_ids_fail_validation() {
        let mut config = FrankClawConfig::default();
        config.models.providers = vec![
            ProviderConfig {
                id: "openai".into(),
                api: "openai".into(),
                base_url: None,
                api_key_ref: Some("OPENAI_API_KEY".into()),
                models: vec!["gpt-4o-mini".into()],
                cooldown_secs: 30,
            },
            ProviderConfig {
                id: "openai".into(),
                api: "ollama".into(),
                base_url: None,
                api_key_ref: None,
                models: vec!["llama3".into()],
                cooldown_secs: 30,
            },
        ];

        assert!(config.validate().is_err());
    }

    #[test]
    fn openai_provider_requires_api_key_ref() {
        let mut config = FrankClawConfig::default();
        config.models.providers = vec![ProviderConfig {
            id: "openai".into(),
            api: "openai".into(),
            base_url: None,
            api_key_ref: None,
            models: vec!["gpt-4o-mini".into()],
            cooldown_secs: 30,
        }];

        assert!(config.validate().is_err());
    }

    #[test]
    fn invalid_dm_policy_fails_validation() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({
                    "dm_policy": "wide_open"
                }),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn channel_security_policy_defaults_to_pairing_and_mentions() {
        let policy = ChannelConfig {
            enabled: true,
            accounts: Vec::new(),
            extra: serde_json::json!({}),
        }
        .security_policy()
        .expect("policy should parse");

        assert_eq!(policy.dm_policy, ChannelDmPolicy::Pairing);
        assert!(policy.require_mention_for_groups);
        assert!(policy.allow_from.is_empty());
        assert!(policy.allowed_groups.is_none());
    }

    #[test]
    fn channel_security_policy_parses_group_allowlist() {
        let policy = ChannelConfig {
            enabled: true,
            accounts: Vec::new(),
            extra: serde_json::json!({
                "groups": ["group:family", "*"]
            }),
        }
        .security_policy()
        .expect("policy should parse");

        assert_eq!(
            policy.allowed_groups,
            Some(vec!["group:family".into(), "*".into()])
        );
    }

    #[test]
    fn invalid_group_allowlist_fails_validation() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("signal"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "base_url": "http://127.0.0.1:8080",
                    "account": "+15551234567"
                })],
                extra: serde_json::json!({
                    "groups": [42]
                }),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn telegram_channel_requires_secret_source() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("telegram"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({})],
                extra: serde_json::json!({}),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn discord_channel_requires_secret_source() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("discord"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({})],
                extra: serde_json::json!({}),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn signal_channel_requires_base_url_source() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("signal"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "account": "+15551234567"
                })],
                extra: serde_json::json!({}),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn whatsapp_channel_requires_access_token_phone_number_and_verify_token() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("whatsapp"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "access_token": "test-token",
                    "phone_number_id": "123456789"
                })],
                extra: serde_json::json!({}),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn unsupported_enabled_channel_fails_validation() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("mattermost"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "test-token"
                })],
                extra: serde_json::json!({}),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn slack_channel_requires_app_and_bot_tokens() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("slack"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "xoxb-test"
                })],
                extra: serde_json::json!({}),
            },
        );

        assert!(config.validate().is_err());
    }

    #[test]
    fn hooks_require_token_and_mapping_when_enabled() {
        let mut config = FrankClawConfig::default();
        config.hooks.enabled = true;

        assert!(config.validate().is_err());

        config.hooks.token = Some("secret".into());
        config.hooks.mappings = vec![serde_json::json!({
            "id": "incoming",
            "text_field": "message"
        })];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn webhook_mapping_rejects_mismatched_agent_and_session() {
        let mut config = FrankClawConfig::default();
        config.hooks.enabled = true;
        config.hooks.token = Some("secret".into());
        config.hooks.mappings = vec![serde_json::json!({
            "id": "incoming",
            "agent_id": "main",
            "session_key": "other:web:default",
            "text_field": "message"
        })];

        assert!(config.validate().is_err());
    }

    #[test]
    fn invalid_bind_address_fails_validation() {
        let mut config = FrankClawConfig::default();
        config.gateway.bind = BindMode::Address("not-an-ip".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn load_or_default_returns_default_when_file_is_missing() {
        let path = std::env::temp_dir().join(format!(
            "frankclaw-missing-config-{}.json",
            uuid::Uuid::new_v4()
        ));

        let loaded = FrankClawConfig::load_or_default(&path).expect("missing config should default");

        assert_eq!(loaded.gateway.port, FrankClawConfig::default().gateway.port);
    }

    #[test]
    fn load_from_path_reads_json_config() {
        let path = std::env::temp_dir().join(format!(
            "frankclaw-config-load-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            serde_json::json!({
                "gateway": {
                    "port": 19999
                }
            })
            .to_string(),
        )
        .expect("config should write");

        let loaded = FrankClawConfig::load_from_path(&path).expect("config should load");

        assert_eq!(loaded.gateway.port, 19999);
        let _ = std::fs::remove_file(path);
    }
}
