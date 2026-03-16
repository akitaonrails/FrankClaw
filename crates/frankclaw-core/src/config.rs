use figment::Figment;
use figment::providers::Env;
use figment::providers::Format as _;
use figment::providers::Serialized;
use figment::providers::Toml;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::auth::{AuthMode, RateLimitConfig};
use crate::error::{ConfigIo, ConfigValidation, Result};
use crate::session::{PruningConfig, SessionResetPolicy, SessionScoping};
use crate::types::{AgentId, ChannelId, SessionKey};

pub const ENV_PREFIX: &str = "FRANKCLAW__";

/// Top-level FrankClaw configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub understanding: MediaUnderstandingConfig,
    /// Named browser profiles for CDP connections.
    #[serde(default)]
    pub browser_profiles: Vec<BrowserProfileConfig>,
}

impl FrankClawConfig {
    #[must_use]
    pub fn base_figment() -> Figment {
        Figment::new().merge(Serialized::defaults(Self::default()))
    }

    #[must_use]
    pub fn figment_with_path(path: Option<&Path>) -> Figment {
        let base = Self::base_figment();
        let figment = match path {
            Some(config_path) if config_path.exists() => base.merge(Toml::file(config_path)),
            Some(_) | None => base,
        };

        figment.merge(Env::prefixed(ENV_PREFIX).split("__"))
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|err| {
            ConfigIo {
                msg: format!("failed to read config '{}': {err}", path.display()),
            }
            .build()
        })?;
        toml::from_str(&content).map_err(|err| {
            ConfigIo {
                msg: format!("failed to parse config '{}': {err}", path.display()),
            }
            .build()
        })
    }

    /// Serialize this config to a pretty-printed TOML string.
    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|err| {
            ConfigIo {
                msg: format!("failed to serialize config: {err}"),
            }
            .build()
        })
    }

    pub fn load_or_default(path: &Path) -> Result<Self> {
        Self::figment_with_path(Some(path))
            .extract()
            .map_err(|err| {
                ConfigIo {
                    msg: format!("failed to load resolved config '{}': {err}", path.display()),
                }
                .build()
            })
    }

    pub fn validate(&self) -> Result<()> {
        self.gateway.auth.validate()?;

        if !self.agents.agents.contains_key(&self.agents.default_agent) {
            return ConfigValidation {
                msg: format!(
                    "default agent '{}' is not present in agents map",
                    self.agents.default_agent
                ),
            }
            .fail();
        }

        let mut provider_ids = std::collections::HashSet::new();
        for provider in &self.models.providers {
            if provider.id.trim().is_empty() {
                return ConfigValidation {
                    msg: "model provider id cannot be empty",
                }
                .fail();
            }
            if !provider_ids.insert(provider.id.clone()) {
                return ConfigValidation {
                    msg: format!("duplicate model provider id '{}'", provider.id),
                }
                .fail();
            }
            match provider.api.as_str() {
                "openai" | "anthropic" | "ollama" | "google" | "gemini" | "openrouter" | "groq"
                | "together" | "deepseek" | "github-copilot" => {}
                other => {
                    return ConfigValidation {
                        msg: format!(
                            "unsupported model provider api '{other}'; expected openai, anthropic, ollama, google, gemini, openrouter, groq, together, deepseek, or github-copilot"
                        ),
                    }
                    .fail();
                }
            }
            if matches!(
                provider.api.as_str(),
                "openai"
                    | "anthropic"
                    | "google"
                    | "gemini"
                    | "openrouter"
                    | "groq"
                    | "together"
                    | "deepseek"
                    | "github-copilot"
            ) && provider
                .api_key_ref
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return ConfigValidation {
                    msg: format!(
                        "provider '{}' requires a non-empty api_key_ref",
                        provider.id
                    ),
                }
                .fail();
            }
        }

        if let Some(default_model) = &self.models.default_model
            && default_model.trim().is_empty()
        {
            return ConfigValidation {
                msg: "models.default_model cannot be empty",
            }
            .fail();
        }

        if self.gateway.max_connections == 0 {
            return ConfigValidation {
                msg: "gateway.max_connections must be greater than 0",
            }
            .fail();
        }

        if self.gateway.max_ws_message_bytes == 0 {
            return ConfigValidation {
                msg: "gateway.max_ws_message_bytes must be greater than 0",
            }
            .fail();
        }

        if let BindMode::Address(address) = &self.gateway.bind
            && address.parse::<std::net::IpAddr>().is_err()
        {
            return ConfigValidation {
                msg: format!("gateway.bind address '{address}' is not a valid IP address"),
            }
            .fail();
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindMode {
    /// 127.0.0.1 only (safest default).
    #[default]
    Loopback,
    /// 0.0.0.0 (LAN accessible). Requires auth.
    Lan,
    /// Specific address.
    Address(String),
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
        agents.insert(AgentId::default_agent(), AgentDef::default());
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SandboxConfig {
    #[default]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::VariantNames)]
#[strum(serialize_all = "snake_case")]
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
            policy.dm_policy = raw.parse::<ChannelDmPolicy>().map_err(|_| {
                ConfigValidation {
                    msg: format!(
                        "invalid dm_policy '{raw}'; expected {}",
                        <ChannelDmPolicy as strum::VariantNames>::VARIANTS.join(", ")
                    ),
                }
                .build()
            })?;
        }

        if let Some(raw) = self.extra.get("allow_from") {
            let entries = raw.as_array().ok_or_else(|| {
                ConfigValidation {
                    msg: "allow_from must be an array of sender ids",
                }
                .build()
            })?;
            policy.allow_from = entries
                .iter()
                .map(|entry| {
                    entry.as_str().map(str::to_string).ok_or_else(|| {
                        ConfigValidation {
                            msg: "allow_from entries must be strings",
                        }
                        .build()
                    })
                })
                .collect::<Result<Vec<_>>>()?;
        }

        if let Some(raw) = self.extra.get("groups") {
            let entries = raw.as_array().ok_or_else(|| {
                ConfigValidation {
                    msg: "groups must be an array of group or thread ids",
                }
                .build()
            })?;
            policy.allowed_groups = Some(
                entries
                    .iter()
                    .map(|entry| {
                        entry.as_str().map(str::to_string).ok_or_else(|| {
                            ConfigValidation {
                                msg: "groups entries must be strings",
                            }
                            .build()
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
        }

        if let Some(raw) = self
            .extra
            .get("require_mention_for_groups")
            .and_then(serde_json::Value::as_bool)
        {
            policy.require_mention_for_groups = raw;
        }

        if let Some(raw) = self.extra.get("max_message_bytes") {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "config values are small positive integers; truncation is not a concern"
            )]
            let value = raw.as_u64().ok_or_else(|| {
                ConfigValidation {
                    msg: "max_message_bytes must be a positive integer",
                }
                .build()
            })? as usize;
            if value == 0 {
                return ConfigValidation {
                    msg: "max_message_bytes must be greater than 0",
                }
                .fail();
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
        other => ConfigValidation {
            msg: format!(
                "unsupported enabled channel '{other}'; currently supported: web, telegram, discord, signal, slack, whatsapp"
            ),
        }
        .fail(),
    }
}

fn validate_channel_account_value_source(
    channel: &ChannelConfig,
    channel_name: &str,
    inline_keys: &[&str],
    env_keys: &[&str],
    label: &str,
) -> Result<()> {
    let account = channel.accounts.first().ok_or_else(|| {
        ConfigValidation {
            msg: format!("{channel_name} channel requires at least one account"),
        }
        .build()
    })?;

    let has_inline_secret = inline_keys.iter().any(|key| {
        account
            .get(*key)
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.trim().is_empty())
    });
    if has_inline_secret {
        return Ok(());
    }

    let has_env_secret = env_keys.iter().any(|key| {
        account
            .get(*key)
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.trim().is_empty())
    });
    if has_env_secret {
        return Ok(());
    }

    ConfigValidation {
        msg: format!(
            "{channel_name} channel requires a non-empty {label} or {label} env reference"
        ),
    }
    .fail()
}

/// Model provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ModelsConfig {
    pub providers: Vec<ProviderConfig>,
    pub default_model: Option<String>,
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
#[derive(Default)]
pub struct SessionConfig {
    pub scoping: SessionScoping,
    pub reset: SessionResetPolicy,
    pub pruning: PruningConfig,
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
            .is_none_or(|value| value.trim().is_empty())
        {
            return ConfigValidation {
                msg: "hooks.enabled requires a non-empty hooks.token",
            }
            .fail();
        }
        if self.mappings.is_empty() {
            return ConfigValidation {
                msg: "hooks.enabled requires at least one mapping",
            }
            .fail();
        }

        let mut seen = std::collections::HashSet::new();
        let mut mappings = Vec::with_capacity(self.mappings.len());
        for raw in &self.mappings {
            let mapping: WebhookMapping = serde_json::from_value(raw.clone()).map_err(|err| {
                ConfigValidation {
                    msg: format!("invalid webhook mapping: {err}"),
                }
                .build()
            })?;
            if mapping.id.trim().is_empty() {
                return ConfigValidation {
                    msg: "webhook mapping id cannot be empty",
                }
                .fail();
            }
            if !seen.insert(mapping.id.clone()) {
                return ConfigValidation {
                    msg: format!("duplicate webhook mapping '{}'", mapping.id),
                }
                .fail();
            }
            if mapping.text_field.trim().is_empty() {
                return ConfigValidation {
                    msg: format!(
                        "webhook mapping '{}' text_field cannot be empty",
                        mapping.id
                    ),
                }
                .fail();
            }
            if let (Some(agent_id), Some(session_key)) = (&mapping.agent_id, &mapping.session_key)
                && let Some((session_agent, _, _)) = session_key.parse()
                && &session_agent != agent_id
            {
                return ConfigValidation {
                    msg: format!(
                        "webhook mapping '{}' session '{}' does not belong to agent '{}'",
                        mapping.id, session_key, agent_id
                    ),
                }
                .fail();
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
    /// Dot-notation JSON path for text extraction (e.g., "data.message.text").
    /// When set, used instead of `text_field` for extraction.
    #[serde(default)]
    pub json_path: Option<String>,
    /// Prefix template for extracted text. `{text}` is replaced with the extracted value.
    #[serde(default)]
    pub template: Option<String>,
    /// Route replies to a specific channel.
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    /// Max concurrent webhook executions for this mapping.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    /// Fixed-window rate limit (requests per minute).
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
}

fn default_max_concurrent() -> usize {
    8
}

impl Default for WebhookMapping {
    fn default() -> Self {
        Self {
            id: String::new(),
            agent_id: None,
            session_key: None,
            text_field: "message".into(),
            json_path: None,
            template: None,
            channel_id: None,
            max_concurrent: 8,
            rate_limit_per_minute: None,
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

/// Media understanding configuration (vision + transcription).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaUnderstandingConfig {
    /// Enable automatic media understanding for non-vision models.
    pub enabled: bool,
    /// Vision provider: "openai", "anthropic", "ollama", or "none".
    pub vision_provider: String,
    /// Model for vision analysis (e.g., "gpt-4o", "claude-sonnet-4-20250514", "llava").
    pub vision_model: Option<String>,
    /// Base URL for the vision provider API.
    pub vision_base_url: Option<String>,
    /// API key env var for vision provider.
    pub vision_api_key_ref: Option<String>,
    /// Transcription provider: "openai" or "none".
    pub transcription_provider: String,
    /// Model for audio transcription (e.g., "whisper-1").
    pub transcription_model: Option<String>,
    /// Base URL for the transcription provider API.
    pub transcription_base_url: Option<String>,
    /// API key env var for transcription provider.
    pub transcription_api_key_ref: Option<String>,
    /// Automatically transcribe voice messages from channels.
    pub auto_transcribe_voice: bool,
}

impl Default for MediaUnderstandingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            vision_provider: "none".into(),
            vision_model: None,
            vision_base_url: None,
            vision_api_key_ref: None,
            transcription_provider: "none".into(),
            transcription_model: None,
            transcription_base_url: None,
            transcription_api_key_ref: None,
            auto_transcribe_voice: false,
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

/// Named browser profile for CDP connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProfileConfig {
    pub name: String,
    pub cdp_port: Option<u16>,
    pub cdp_url: Option<String>,
    #[serde(default)]
    pub color: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Jail;
    use rstest::rstest;

    type JailResult<T> = std::result::Result<T, Box<figment::Error>>;

    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail requires figment::Error to preserve env isolation failures"
    )]
    fn with_clean_jail<F>(f: F)
    where
        F: FnOnce(&mut Jail) -> JailResult<()>,
    {
        Jail::try_with(|jail| {
            jail.clear_env();
            f(jail).map_err(|error| *error)
        })
        .expect("figment jail should execute successfully");
    }

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

    #[rstest]
    #[case("telegram", serde_json::json!({}))]
    #[case("discord", serde_json::json!({}))]
    #[case("signal", serde_json::json!({"account": "+15551234567"}))]
    fn enabled_channel_without_credentials_fails_validation(
        #[case] channel_name: &str,
        #[case] account: serde_json::Value,
    ) {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new(channel_name),
            ChannelConfig {
                enabled: true,
                accounts: vec![account],
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
        config
            .validate()
            .expect("config should validate once token and mapping are set");
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
            "frankclaw-missing-config-{}.toml",
            uuid::Uuid::new_v4()
        ));

        with_clean_jail(|_jail| {
            let loaded =
                FrankClawConfig::load_or_default(&path).expect("missing config should default");

            assert_eq!(loaded.gateway.port, FrankClawConfig::default().gateway.port);
            Ok(())
        });
    }

    #[test]
    fn load_from_path_reads_toml_config() {
        let path = std::env::temp_dir().join(format!(
            "frankclaw-config-load-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[gateway]\nport = 19999\n").expect("config should write");

        let loaded = FrankClawConfig::load_from_path(&path).expect("config should load");

        assert_eq!(loaded.gateway.port, 19999);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_or_default_reads_existing_toml_config() {
        let path = std::env::temp_dir().join(format!(
            "frankclaw-resolved-config-load-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[gateway]\nport = 18888\n").expect("config should write");

        with_clean_jail(|_jail| {
            let loaded =
                FrankClawConfig::load_or_default(&path).expect("resolved config should load");

            assert_eq!(loaded.gateway.port, 18888);
            Ok(())
        });
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_or_default_applies_env_overrides_on_top_of_file_values() {
        let path = std::env::temp_dir().join(format!(
            "frankclaw-resolved-config-env-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[gateway]\nport = 18888\n").expect("config should write");

        with_clean_jail(|jail| {
            jail.set_env(format!("{ENV_PREFIX}GATEWAY__PORT"), "19999");

            let loaded =
                FrankClawConfig::load_or_default(&path).expect("env overrides should load");

            assert_eq!(loaded.gateway.port, 19999);
            Ok(())
        });

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn to_toml_string_roundtrips() {
        let config = FrankClawConfig::default();
        let toml_str = config.to_toml_string().expect("should serialize");
        let loaded: FrankClawConfig = toml::from_str(&toml_str).expect("should parse back");
        assert_eq!(loaded.gateway.port, config.gateway.port);
    }
}
