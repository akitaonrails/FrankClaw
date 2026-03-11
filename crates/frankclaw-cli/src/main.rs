#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Context;
use base64::Engine;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

/// FrankClaw — personal AI assistant gateway.
///
/// Hardened Rust rewrite of OpenClaw. Connects messaging channels to AI models
/// with encrypted sessions, SSRF protection, and secure defaults.
#[derive(Parser)]
#[command(name = "frankclaw", version, about)]
struct Cli {
    /// Configuration file path.
    #[arg(short, long, env = "FRANKCLAW_CONFIG")]
    config: Option<PathBuf>,

    /// State directory (sessions, media, logs).
    #[arg(long, env = "FRANKCLAW_STATE_DIR")]
    state_dir: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, env = "FRANKCLAW_LOG", default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the gateway server.
    Gateway {
        /// Override the listen port.
        #[arg(short, long)]
        port: Option<u16>,
    },

    /// Generate a secure auth token.
    GenToken,

    /// Hash a password for config (Argon2id).
    HashPassword,

    /// Validate config file.
    Check,

    /// Run high-signal validation and readiness checks.
    Doctor,

    /// Show resolved configuration (secrets redacted).
    Config,

    /// Show runtime and exposure status for the configured gateway.
    Status,

    /// Send a message through the local runtime.
    MessageSend {
        /// Message text to send.
        #[arg(long)]
        message: String,

        /// Target agent ID.
        #[arg(long)]
        agent: Option<String>,

        /// Explicit session key.
        #[arg(long)]
        session: Option<String>,

        /// Override model ID.
        #[arg(long)]
        model: Option<String>,
    },

    /// Edit the last tracked assistant reply for a session.
    MessageEditLast {
        /// Session key whose last assistant reply should be edited.
        #[arg(long)]
        session: String,

        /// Replacement text.
        #[arg(long)]
        text: String,
    },

    /// List available models from configured providers.
    ModelsList,

    /// List tools allowed for an agent.
    ToolsList {
        /// Agent ID to inspect.
        #[arg(long)]
        agent: Option<String>,
    },

    /// Invoke an allowed read-only tool locally.
    ToolsInvoke {
        /// Tool name.
        #[arg(long)]
        tool: String,

        /// Agent ID whose tool policy should be used.
        #[arg(long)]
        agent: Option<String>,

        /// Optional session key for session-scoped tools.
        #[arg(long)]
        session: Option<String>,

        /// JSON object of tool arguments.
        #[arg(long)]
        args: Option<String>,
    },

    /// List validated skills for an agent.
    SkillsList {
        /// Agent ID to inspect.
        #[arg(long)]
        agent: Option<String>,
    },

    /// Session inspection commands.
    SessionsList {
        /// Agent ID to list sessions for.
        #[arg(long)]
        agent: Option<String>,

        /// Maximum sessions to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// Offset for pagination.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },

    /// Show transcript entries for a session.
    SessionsGet {
        /// Session key.
        #[arg(long)]
        session: String,

        /// Maximum transcript entries to return.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },

    /// Clear transcript entries for a session.
    SessionsReset {
        /// Session key.
        #[arg(long)]
        session: String,
    },

    /// List pending pairing requests.
    PairingList {
        /// Restrict to a specific channel.
        channel: Option<String>,
    },

    /// Approve a pending pairing request by code.
    PairingApprove {
        /// Channel for the pending pairing request.
        channel: String,

        /// Pairing code.
        code: String,

        /// Restrict to a specific account.
        #[arg(long)]
        account: Option<String>,
    },

    /// Show how the current gateway config would be exposed remotely.
    RemoteStatus,

    /// Fail unless the current gateway config is safe for the requested exposure.
    RemoteCheck {
        /// Require the config to be suitable for direct public exposure.
        #[arg(long)]
        public: bool,
    },

    /// Initialize a new config file with secure defaults.
    Init {
        /// Force overwrite existing config.
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&cli.log_level)),
        )
        .with_target(false)
        .init();

    let state_dir = cli
        .state_dir
        .unwrap_or_else(|| default_state_dir());

    match cli.command {
        Command::Gateway { port } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            let mut config = config;
            if let Some(port) = port {
                config.gateway.port = port;
            }
            config.validate()?;

            let db_path = state_dir.join("sessions.db");
            let sessions = std::sync::Arc::new(
                frankclaw_sessions::SqliteSessionStore::open(
                    &db_path,
                    load_master_key_from_env()?.as_ref(),
                )
                    .context("failed to open session store")?,
            );
            let runtime = std::sync::Arc::new(
                frankclaw_runtime::Runtime::from_config(
                    &config,
                    sessions.clone() as std::sync::Arc<dyn frankclaw_core::session::SessionStore>,
                )
                .await
                .context("failed to initialize runtime")?,
            );
            let pairing = open_pairing_store(&state_dir)?;
            let cron = open_cron_service(&state_dir)?;

            info!(
                port = config.gateway.port,
                bind = ?config.gateway.bind,
                "starting frankclaw gateway"
            );

            frankclaw_gateway::server::run(config, sessions, runtime, pairing, cron).await?;
        }

        Command::GenToken => {
            let token = frankclaw_crypto::generate_token();
            println!("{token}");
        }

        Command::HashPassword => {
            eprint!("Enter password: ");
            let password = read_password()?;
            let hash = frankclaw_crypto::hash_password(&password)
                .context("failed to hash password")?;
            println!("{}", hash.as_str());
        }

        Command::Check => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            println!("Configuration is valid.");
            println!("  Gateway port: {}", config.gateway.port);
            println!("  Auth mode: {:?}", config.gateway.auth);
            println!("  Channels: {}", config.channels.len());
            println!("  Providers: {}", config.models.providers.len());
        }

        Command::Doctor => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;

            let mut warnings = Vec::new();
            if config.models.providers.is_empty() {
                warnings.push("no model providers configured");
            }
            if config.channels.is_empty() {
                warnings.push("no channels configured");
            }
            if !config.security.encrypt_sessions {
                warnings.push("session encryption is disabled");
            }
            if config.security.encrypt_sessions && load_master_key_from_env()?.is_none() {
                warnings.push("session encryption is enabled but FRANKCLAW_MASTER_KEY is not set");
            }
            let exposure = frankclaw_gateway::auth::assess_exposure(&config)?;
            warnings.extend(exposure.warnings.iter().map(String::as_str));

            println!("Doctor check passed.");
            if warnings.is_empty() {
                println!("  No obvious misconfigurations found.");
            } else {
                println!("  Warnings:");
                for warning in warnings {
                    println!("    - {warning}");
                }
            }
        }

        Command::Config => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            let json = serde_json::to_string_pretty(&redact_config(&config))?;
            println!("{json}");
        }

        Command::Status => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions).await?;
            let channels = frankclaw_channels::load_from_config(&config)
                .context("failed to load configured channels")?;
            let exposure = frankclaw_gateway::auth::assess_exposure(&config)?;

            print_exposure_report(&exposure);
            println!();
            println!("Providers:");
            for provider in runtime.provider_health().await {
                println!(
                    "  {}  {}",
                    provider.provider_id,
                    if provider.healthy { "healthy" } else { "unhealthy" }
                );
            }
            println!();
            println!("Channels:");
            for (channel_id, channel) in channels.channels() {
                println!("  {}  {:?}", channel_id, channel.health().await);
            }
        }

        Command::MessageSend {
            message,
            agent,
            session,
            model,
        } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions.clone()).await?;

            let response = runtime
                .chat(frankclaw_runtime::ChatRequest {
                    agent_id: agent.map(frankclaw_core::types::AgentId::new),
                    session_key: session.map(frankclaw_core::types::SessionKey::from_raw),
                    message,
                    model_id: model,
                    max_tokens: None,
                    temperature: None,
                })
                .await?;

            println!("Session: {}", response.session_key);
            println!("Model:   {}", response.model_id);
            println!();
            println!("{}", response.content);
        }

        Command::MessageEditLast { session, text } => {
            use frankclaw_core::channel::EditMessageTarget;
            use frankclaw_core::session::SessionStore;

            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let session_key = frankclaw_core::types::SessionKey::from_raw(session);
            let mut entry = sessions
                .get(&session_key)
                .await?
                .context("session not found")?;
            let mut last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(&entry.metadata)
                .context("session has no tracked delivery metadata")?;

            if last_reply.chunks.len() > 1 {
                anyhow::bail!("editing chunked replies is not supported yet");
            }

            let platform_message_id = last_reply
                .platform_message_id
                .clone()
                .context("tracked reply is missing platform_message_id")?;

            let channels = frankclaw_channels::load_from_config(&config)
                .context("failed to load configured channels")?;
            let channel = channels
                .get(&entry.channel)
                .cloned()
                .with_context(|| format!("channel '{}' is not configured", entry.channel))?;

            channel
                .edit_message(
                    &EditMessageTarget {
                        account_id: last_reply.account_id.clone(),
                        to: last_reply.recipient_id.clone(),
                        thread_id: last_reply.thread_id.clone(),
                        platform_message_id,
                    },
                    &text,
                )
                .await?;

            let rewritten = sessions
                .rewrite_last_assistant_message(&session_key, &text)
                .await?;
            if !rewritten {
                anyhow::bail!("session has no assistant turn to rewrite");
            }

            last_reply.content = text.clone();
            if let Some(first_chunk) = last_reply.chunks.first_mut() {
                first_chunk.content = text.clone();
            }

            frankclaw_gateway::delivery::set_last_reply_in_metadata(&mut entry.metadata, &last_reply)
                .context("failed to update delivery metadata")?;
            sessions.upsert(&entry).await?;

            println!("Edited last reply for session {}.", session_key);
        }

        Command::ModelsList => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions).await?;

            for model in runtime.list_models() {
                println!("{} ({:?})", model.id, model.api);
            }
        }

        Command::ToolsList { agent } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions).await?;
            let tools = runtime.list_tools(
                agent
                    .as_ref()
                    .map(|value| frankclaw_core::types::AgentId::new(value.clone()))
                    .as_ref(),
            )?;

            for tool in tools {
                println!("{} - {}", tool.name, tool.description);
            }
        }

        Command::ToolsInvoke {
            tool,
            agent,
            session,
            args,
        } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions).await?;
            let arguments = match args {
                Some(raw) => serde_json::from_str(&raw)
                    .context("tool args must be a valid JSON object")?,
                None => serde_json::json!({}),
            };

            let output = runtime
                .invoke_tool(frankclaw_runtime::ToolRequest {
                    agent_id: agent.map(frankclaw_core::types::AgentId::new),
                    session_key: session.map(frankclaw_core::types::SessionKey::from_raw),
                    tool_name: tool,
                    arguments,
                })
                .await?;
            println!("{}", serde_json::to_string_pretty(&output.output)?);
        }

        Command::SkillsList { agent } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions).await?;
            let skills = runtime.list_skills(
                agent
                    .as_ref()
                    .map(|value| frankclaw_core::types::AgentId::new(value.clone()))
                    .as_ref(),
            )?;

            for skill in skills {
                println!("{} - {}", skill.id, skill.name);
                if let Some(description) = &skill.description {
                    println!("  {}", description);
                }
                if !skill.capabilities.is_empty() {
                    println!(
                        "  capabilities: {}",
                        skill.capabilities
                            .iter()
                            .map(display_skill_capability)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                if !skill.tools.is_empty() {
                    println!("  tools: {}", skill.tools.join(", "));
                }
            }
        }

        Command::SessionsList {
            agent,
            limit,
            offset,
        } => {
            use frankclaw_core::session::SessionStore;

            let sessions = open_sessions(&state_dir)?;
            let agent_id = agent
                .map(frankclaw_core::types::AgentId::new)
                .unwrap_or_else(frankclaw_core::types::AgentId::default_agent);
            let entries = sessions.list(&agent_id, limit, offset).await?;

            for entry in entries {
                println!(
                    "{}  channel={}  account={}",
                    entry.key, entry.channel, entry.account_id
                );
            }
        }

        Command::SessionsGet { session, limit } => {
            use frankclaw_core::session::SessionStore;

            let sessions = open_sessions(&state_dir)?;
            let entries = sessions
                .get_transcript(
                    &frankclaw_core::types::SessionKey::from_raw(session),
                    limit,
                    None,
                )
                .await?;

            for entry in entries {
                println!("[{}] {:?}: {}", entry.seq, entry.role, entry.content);
            }
        }

        Command::SessionsReset { session } => {
            use frankclaw_core::session::SessionStore;

            let sessions = open_sessions(&state_dir)?;
            sessions
                .clear_transcript(&frankclaw_core::types::SessionKey::from_raw(session))
                .await?;
            println!("Session transcript cleared.");
        }

        Command::PairingList { channel } => {
            let store = open_pairing_store(&state_dir)?;
            for pending in store.list_pending(channel.as_deref()) {
                println!(
                    "{}  channel={}  account={}  sender={}",
                    pending.code, pending.channel, pending.account_id, pending.sender_id
                );
            }
        }

        Command::PairingApprove {
            channel,
            code,
            account,
        } => {
            let store = open_pairing_store(&state_dir)?;
            let approved = store.approve(Some(&channel), account.as_deref(), &code)?;
            println!(
                "Approved sender {} on {}/{}",
                approved.sender_id, approved.channel, approved.account_id
            );
        }

        Command::RemoteStatus => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let report = frankclaw_gateway::auth::assess_exposure(&config)?;
            print_exposure_report(&report);
        }

        Command::RemoteCheck { public } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let report = frankclaw_gateway::auth::assess_exposure(&config)?;
            print_exposure_report(&report);

            if public {
                if !report.public_ready {
                    anyhow::bail!("gateway config is not ready for direct public exposure");
                }
            } else if !report.remote_ready {
                anyhow::bail!("gateway config is not ready for remote exposure");
            }
        }

        Command::Init { force } => {
            let config_path = cli
                .config
                .unwrap_or_else(|| state_dir.join("frankclaw.json"));

            if config_path.exists() && !force {
                anyhow::bail!(
                    "config already exists at {}. Use --force to overwrite.",
                    config_path.display()
                );
            }

            let config = frankclaw_core::config::FrankClawConfig::default();
            let json = serde_json::to_string_pretty(&config)?;

            std::fs::create_dir_all(config_path.parent().unwrap_or(&state_dir))?;
            std::fs::write(&config_path, &json)?;

            // Restrict config file permissions.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                let _ = std::fs::set_permissions(&config_path, perms);
            }

            println!("Config created at: {}", config_path.display());
            println!();
            println!("Next steps:");
            println!("  1. Generate an auth token:  frankclaw gen-token");
            println!("  2. Edit the config:         $EDITOR {}", config_path.display());
            println!("  3. Start the gateway:       frankclaw gateway");
        }
    }

    Ok(())
}

fn default_state_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("frankclaw")
}

fn load_config(
    path: Option<&std::path::Path>,
    state_dir: &std::path::Path,
) -> anyhow::Result<frankclaw_core::config::FrankClawConfig> {
    let config_path = path
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir.join("frankclaw.json"));

    if !config_path.exists() {
        info!("no config found at {}, using defaults", config_path.display());
        return Ok(frankclaw_core::config::FrankClawConfig::default());
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;

    let config: frankclaw_core::config::FrankClawConfig =
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse config: {}", config_path.display()))?;

    Ok(config)
}

fn read_password() -> anyhow::Result<secrecy::SecretString> {
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("failed to read password")?;
    Ok(secrecy::SecretString::from(input.trim().to_string()))
}

fn display_skill_capability(
    capability: &frankclaw_plugin_sdk::SkillCapability,
) -> &'static str {
    match capability {
        frankclaw_plugin_sdk::SkillCapability::Prompt => "prompt",
        frankclaw_plugin_sdk::SkillCapability::ReadSession => "read_session",
    }
}

fn print_exposure_report(report: &frankclaw_gateway::auth::ExposureReport) {
    println!("Summary: {}", report.summary);
    println!("Auth:    {}", report.auth_mode);
    println!("Bind:    {}", display_exposure_surface(&report.surface));
    println!("Remote:  {}", if report.remote_ready { "ready" } else { "not ready" });
    println!("Public:  {}", if report.public_ready { "ready" } else { "not ready" });
    if !report.warnings.is_empty() {
        println!();
        println!("Warnings:");
        for warning in &report.warnings {
            println!("  - {warning}");
        }
    }
}

fn display_exposure_surface(
    surface: &frankclaw_gateway::auth::ExposureSurface,
) -> String {
    match surface {
        frankclaw_gateway::auth::ExposureSurface::Loopback => "loopback".into(),
        frankclaw_gateway::auth::ExposureSurface::Lan => "lan".into(),
        frankclaw_gateway::auth::ExposureSurface::PrivateAddress(address) => {
            format!("private_address:{address}")
        }
        frankclaw_gateway::auth::ExposureSurface::PublicAddress(address) => {
            format!("public_address:{address}")
        }
    }
}

fn open_sessions(
    state_dir: &std::path::Path,
) -> anyhow::Result<std::sync::Arc<frankclaw_sessions::SqliteSessionStore>> {
    let db_path = state_dir.join("sessions.db");
    Ok(std::sync::Arc::new(
        frankclaw_sessions::SqliteSessionStore::open(
            &db_path,
            load_master_key_from_env()?.as_ref(),
        )
            .context("failed to open session store")?,
    ))
}

fn open_pairing_store(
    state_dir: &std::path::Path,
) -> anyhow::Result<std::sync::Arc<frankclaw_gateway::pairing::PairingStore>> {
    let path = state_dir.join("pairings.json");
    Ok(std::sync::Arc::new(
        frankclaw_gateway::pairing::PairingStore::open(&path)
            .context("failed to open pairing store")?,
    ))
}

fn open_cron_service(
    state_dir: &std::path::Path,
) -> anyhow::Result<std::sync::Arc<frankclaw_cron::CronService>> {
    let path = state_dir.join("cron-jobs.json");
    Ok(std::sync::Arc::new(
        frankclaw_cron::CronService::open(&path)
            .context("failed to open cron store")?,
    ))
}

async fn build_runtime(
    config: &frankclaw_core::config::FrankClawConfig,
    sessions: std::sync::Arc<frankclaw_sessions::SqliteSessionStore>,
) -> anyhow::Result<std::sync::Arc<frankclaw_runtime::Runtime>> {
    Ok(std::sync::Arc::new(
        frankclaw_runtime::Runtime::from_config(
            config,
            sessions as std::sync::Arc<dyn frankclaw_core::session::SessionStore>,
        )
        .await
        .context("failed to initialize runtime")?,
    ))
}

fn redact_config(config: &frankclaw_core::config::FrankClawConfig) -> serde_json::Value {
    let mut val = serde_json::to_value(config).unwrap_or(serde_json::json!({}));
    if let Some(obj) = val.as_object_mut() {
        if let Some(gateway) = obj.get_mut("gateway").and_then(|value| value.as_object_mut()) {
            if let Some(auth) = gateway.get_mut("auth").and_then(|value| value.as_object_mut()) {
                if let Some(token) = auth.get_mut("token") {
                    *token = serde_json::json!("[REDACTED]");
                }
                if let Some(hash) = auth.get_mut("hash") {
                    *hash = serde_json::json!("[REDACTED]");
                }
            }
        }

        if let Some(models) = obj.get_mut("models").and_then(|value| value.as_object_mut()) {
            if let Some(providers) = models
                .get_mut("providers")
                .and_then(|value| value.as_array_mut())
            {
                for provider in providers {
                    if let Some(api_key_ref) = provider.get_mut("api_key_ref") {
                        *api_key_ref = serde_json::json!("[REDACTED]");
                    }
                }
            }
        }
    }
    val
}

fn load_master_key_from_env() -> anyhow::Result<Option<frankclaw_crypto::MasterKey>> {
    if let Ok(raw_key) = std::env::var("FRANKCLAW_MASTER_KEY") {
        if raw_key.trim().is_empty() {
            anyhow::bail!("FRANKCLAW_MASTER_KEY is set but empty");
        }

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw_key.trim())
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw_key.trim()))
            .context("FRANKCLAW_MASTER_KEY must be valid base64")?;

        if decoded.len() != 32 {
            anyhow::bail!("FRANKCLAW_MASTER_KEY must decode to exactly 32 bytes");
        }

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&decoded);
        return Ok(Some(frankclaw_crypto::MasterKey::from_bytes(bytes)));
    }

    Ok(None)
}
