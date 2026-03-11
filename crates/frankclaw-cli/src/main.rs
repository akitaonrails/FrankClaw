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

    /// Print a supported channel config example.
    ConfigExample {
        /// Channel example to print: web, telegram, discord, slack, signal, whatsapp.
        #[arg(long)]
        channel: String,
    },

    /// Show runtime and exposure status for the configured gateway.
    Status,

    /// Generate a secure starter config for a chosen channel profile.
    Onboard {
        /// Starter channel profile: web, telegram, whatsapp, slack, discord, signal.
        #[arg(long, default_value = "web")]
        channel: String,

        /// Force overwrite an existing config.
        #[arg(long)]
        force: bool,
    },

    /// Print a systemd unit for the current install.
    InstallSystemd {
        /// Optional explicit config path for ExecStart.
        #[arg(long)]
        config: Option<PathBuf>,
    },

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

    /// Delete the last tracked assistant reply for a session.
    MessageDeleteLast {
        /// Session key whose last assistant reply should be deleted.
        #[arg(long)]
        session: String,
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

    /// Show recent tool activity for one session.
    ToolsActivity {
        /// Session key to inspect.
        #[arg(long)]
        session: String,

        /// Maximum tool activity entries to return.
        #[arg(long, default_value_t = 10)]
        limit: usize,
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
            let config_path = cli
                .config
                .clone()
                .unwrap_or_else(|| state_dir.join("frankclaw.json"));
            let config = load_config(Some(&config_path), &state_dir)?;
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
            let media = open_media_store(&config, &state_dir)?;

            info!(
                port = config.gateway.port,
                bind = ?config.gateway.bind,
                "starting frankclaw gateway"
            );

            frankclaw_gateway::server::run(
                config,
                Some(config_path),
                sessions,
                runtime,
                pairing,
                cron,
                media,
            )
            .await?;
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

            let warnings = collect_doctor_warnings(&config, &state_dir)?;
            let exposure = frankclaw_gateway::auth::assess_exposure(&config)?;

            println!("Doctor check passed.");
            println!("  Exposure: {}", exposure.summary);
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

        Command::ConfigExample { channel } => {
            let example = supported_channel_example(&channel)
                .ok_or_else(|| anyhow::anyhow!(
                    "unsupported channel example '{}'; expected web, telegram, discord, slack, signal, or whatsapp",
                    channel
                ))?;
            println!("{example}");
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
            println!("Agents:");
            for (agent_id, agent, skills) in runtime.agent_surface() {
                println!(
                    "  {}  model={}  tools={}  skills={}",
                    agent_id,
                    agent
                        .model
                        .clone()
                        .or_else(|| config.models.default_model.clone())
                        .unwrap_or_else(|| "<unset>".into()),
                    if agent.tools.is_empty() {
                        "-".into()
                    } else {
                        agent.tools.join(", ")
                    },
                    if skills.is_empty() {
                        "-".into()
                    } else {
                        skills
                            .iter()
                            .map(|skill| skill.id.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                );
            }
            if let Some(browser_status) = browser_runtime_status(&config, std::env::var("FRANKCLAW_BROWSER_DEVTOOLS_URL").ok().as_deref()) {
                println!();
                println!("Browser:");
                println!("  {}", browser_status);
            }
            println!();
            println!("Channels:");
            for (channel_id, channel) in channels.channels() {
                println!("  {}  {:?}", channel_id, channel.health().await);
            }
        }

        Command::Onboard { channel, force } => {
            let config_path = cli
                .config
                .clone()
                .unwrap_or_else(|| state_dir.join("frankclaw.json"));
            if config_path.exists() && !force {
                anyhow::bail!(
                    "config already exists at {}. Use --force to overwrite.",
                    config_path.display()
                );
            }

            let gateway_token = frankclaw_crypto::generate_token();
            let config = build_onboard_config(&channel, &gateway_token)?;
            let json = serde_json::to_string_pretty(&config)?;
            std::fs::create_dir_all(config_path.parent().unwrap_or(&state_dir))?;
            std::fs::write(&config_path, json)?;
            restrict_file_permissions(&config_path);

            println!("Starter config created at: {}", config_path.display());
            println!("Gateway token: {gateway_token}");
            println!();
            println!("Next steps:");
            println!("  1. Fill the provider env vars referenced in the config.");
            println!("  2. If using channel-specific env vars, export those too.");
            println!("  3. Start locally: frankclaw gateway --config {}", config_path.display());
        }

        Command::InstallSystemd { config } => {
            let config_path = config
                .or_else(|| cli.config.clone())
                .unwrap_or_else(|| state_dir.join("frankclaw.json"));
            let executable = std::env::current_exe().context("failed to locate frankclaw binary")?;
            println!(
                "{}",
                render_systemd_unit(&executable, &config_path, &state_dir)
            );
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
                    attachments: Vec::new(),
                    model_id: model,
                    max_tokens: None,
                    temperature: None,
                    stream_tx: None,
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
            let last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(&entry.metadata)
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

            rewrite_last_reply_metadata_for_edit(&mut entry.metadata, &text)?;
            sessions.upsert(&entry).await?;

            println!("Edited last reply for session {}.", session_key);
        }

        Command::MessageDeleteLast { session } => {
            use frankclaw_core::channel::DeleteMessageTarget;
            use frankclaw_core::session::SessionStore;

            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let session_key = frankclaw_core::types::SessionKey::from_raw(session);
            let mut entry = sessions
                .get(&session_key)
                .await?
                .context("session not found")?;
            let last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(&entry.metadata)
                .context("session has no tracked delivery metadata")?;

            let channels = frankclaw_channels::load_from_config(&config)
                .context("failed to load configured channels")?;
            let channel = channels
                .get(&entry.channel)
                .cloned()
                .with_context(|| format!("channel '{}' is not configured", entry.channel))?;

            for platform_message_id in delete_targets_from_last_reply(&last_reply)? {
                channel
                    .delete_message(&DeleteMessageTarget {
                        account_id: last_reply.account_id.clone(),
                        to: last_reply.recipient_id.clone(),
                        thread_id: last_reply.thread_id.clone(),
                        platform_message_id,
                    })
                    .await?;
            }

            mark_last_reply_metadata_deleted(&mut entry.metadata)?;
            sessions.upsert(&entry).await?;

            println!("Deleted last reply for session {}.", session_key);
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

        Command::ToolsActivity { session, limit } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions).await?;
            let activity = runtime
                .tool_activity(&frankclaw_core::types::SessionKey::from_raw(session), limit)
                .await?;

            if activity.is_empty() {
                println!("No tool activity found.");
            } else {
                for entry in activity {
                    println!(
                        "[{}] {}  {}{}",
                        entry.seq,
                        entry.timestamp.to_rfc3339(),
                        entry.tool_name,
                        entry
                            .tool_call_id
                            .as_deref()
                            .map(|value| format!(" ({value})"))
                            .unwrap_or_default()
                    );
                    println!("  {}", entry.output_preview);
                }
            }
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
            restrict_file_permissions(&config_path);

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
        return frankclaw_core::config::FrankClawConfig::load_or_default(&config_path)
            .map_err(anyhow::Error::from);
    }
    frankclaw_core::config::FrankClawConfig::load_from_path(&config_path)
        .map_err(anyhow::Error::from)
}

fn collect_doctor_warnings(
    config: &frankclaw_core::config::FrankClawConfig,
    state_dir: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    let mut warnings = Vec::new();

    if config.models.providers.is_empty() {
        warnings.push("no model providers configured".into());
    }
    if config.channels.is_empty() {
        warnings.push("no channels configured".into());
    }
    if !config.security.encrypt_sessions {
        warnings.push("session encryption is disabled".into());
    }
    if config.security.encrypt_sessions && load_master_key_from_env()?.is_none() {
        warnings.push("session encryption is enabled but FRANKCLAW_MASTER_KEY is not set".into());
    }
    if !state_dir.exists() {
        warnings.push(format!(
            "state directory '{}' does not exist yet",
            state_dir.display()
        ));
    }

    for provider in &config.models.providers {
        if let Some(env_name) = provider.api_key_ref.as_deref() {
            if std::env::var(env_name).ok().filter(|value| !value.trim().is_empty()).is_none() {
                warnings.push(format!(
                    "provider '{}' references missing environment variable '{}'",
                    provider.id, env_name
                ));
            }
        }
    }

    for (channel_id, channel) in &config.channels {
        let policy = channel
            .security_policy()
            .with_context(|| format!("invalid security policy for channel '{}'", channel_id))?;

        if group_surface_needs_guard(channel_id.as_str()) && !policy.require_mention_for_groups && policy.allowed_groups.is_none() {
            warnings.push(format!(
                "channel '{}' accepts group messages without mention gating and without a groups allowlist",
                channel_id
            ));
        }

        for account in &channel.accounts {
            for key in [
                "bot_token_env",
                "token_env",
                "app_token_env",
                "base_url_env",
                "phone_number_id_env",
                "verify_token_env",
                "access_token_env",
                "app_secret_env",
            ] {
                if let Some(env_name) = account.get(key).and_then(|value| value.as_str()) {
                    if std::env::var(env_name).ok().filter(|value| !value.trim().is_empty()).is_none() {
                        warnings.push(format!(
                            "channel '{}' references missing environment variable '{}' via {}",
                            channel_id, env_name, key
                        ));
                    }
                }
            }

            for (inline_key, env_key) in [
                ("bot_token", "bot_token_env"),
                ("token", "token_env"),
                ("app_token", "app_token_env"),
                ("access_token", "access_token_env"),
                ("verify_token", "verify_token_env"),
                ("app_secret", "app_secret_env"),
            ] {
                if account
                    .get(inline_key)
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some()
                {
                    warnings.push(format!(
                        "channel '{}' stores '{}' inline; prefer '{}' environment references for secrets",
                        channel_id, inline_key, env_key
                    ));
                }
            }

            if channel_id.as_str() == "whatsapp"
                && account.get("app_secret").and_then(|value| value.as_str()).map(str::trim).filter(|value| !value.is_empty()).is_none()
                && account.get("app_secret_env").and_then(|value| value.as_str()).map(str::trim).filter(|value| !value.is_empty()).is_none()
            {
                warnings.push(
                    "whatsapp channel has no app_secret configured; inbound webhook signatures will not be verified"
                        .into(),
                );
            }
        }
    }

    let exposure = frankclaw_gateway::auth::assess_exposure(config)?;
    warnings.extend(exposure.warnings);
    warnings.extend(collect_browser_tool_warnings(
        config,
        std::env::var("FRANKCLAW_BROWSER_DEVTOOLS_URL").ok().as_deref(),
    ));

    Ok(warnings)
}

fn group_surface_needs_guard(channel_id: &str) -> bool {
    matches!(channel_id, "telegram" | "discord" | "slack" | "signal" | "whatsapp")
}

fn collect_browser_tool_warnings(
    config: &frankclaw_core::config::FrankClawConfig,
    browser_endpoint: Option<&str>,
) -> Vec<String> {
    collect_browser_tool_warnings_with_policy(
        config,
        browser_endpoint,
        frankclaw_tools::ToolPolicy::from_env(),
    )
}

fn collect_browser_tool_warnings_with_policy(
    config: &frankclaw_core::config::FrankClawConfig,
    browser_endpoint: Option<&str>,
    tool_policy: frankclaw_tools::ToolPolicy,
) -> Vec<String> {
    let browser_tools_enabled = config
        .agents
        .agents
        .values()
        .flat_map(|agent| agent.tools.iter())
        .any(|tool| tool.starts_with("browser."));
    if !browser_tools_enabled {
        return Vec::new();
    }
    let browser_mutation_tools_enabled = config
        .agents
        .agents
        .values()
        .flat_map(|agent| agent.tools.iter())
        .any(|tool| frankclaw_tools::tool_requires_operator_approval(tool));

    let endpoint = browser_endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("http://127.0.0.1:9222/");
    let parsed = match url::Url::parse(endpoint) {
        Ok(parsed) => parsed,
        Err(err) => {
            return vec![format!(
                "browser tools are enabled but FRANKCLAW_BROWSER_DEVTOOLS_URL is invalid: {}",
                err
            )];
        }
    };

    let mut warnings = Vec::new();
    if browser_mutation_tools_enabled && !tool_policy.allow_browser_mutations {
        warnings.push(
            "browser mutation tools are configured but blocked until FRANKCLAW_ALLOW_BROWSER_MUTATIONS=1 is set".into(),
        );
    }
    match parsed.host_str() {
        Some("127.0.0.1") | Some("localhost") => {}
        Some(other) => warnings.push(format!(
            "browser tools are pointed at non-loopback host '{}'; keep Chromium DevTools local-only",
            other
        )),
        None => warnings.push("browser tools endpoint has no host".into()),
    }

    let port = parsed.port_or_known_default().unwrap_or(9222);
    let Some(host) = parsed.host_str() else {
        return warnings;
    };
    match std::net::TcpStream::connect_timeout(
        &format!("{host}:{port}")
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        std::time::Duration::from_millis(250),
    ) {
        Ok(_) => {}
        Err(_) => warnings.push(format!(
            "browser tools are enabled but Chromium DevTools is unreachable at {}; start it locally or run `docker compose up -d chromium`",
            endpoint
        )),
    }

    warnings
}

fn browser_runtime_status(
    config: &frankclaw_core::config::FrankClawConfig,
    browser_endpoint: Option<&str>,
) -> Option<String> {
    browser_runtime_status_with_policy(
        config,
        browser_endpoint,
        frankclaw_tools::ToolPolicy::from_env(),
    )
}

fn browser_runtime_status_with_policy(
    config: &frankclaw_core::config::FrankClawConfig,
    browser_endpoint: Option<&str>,
    policy: frankclaw_tools::ToolPolicy,
) -> Option<String> {
    let warnings = collect_browser_tool_warnings_with_policy(config, browser_endpoint, policy);
    if warnings.is_empty() {
        if config
            .agents
            .agents
            .values()
            .flat_map(|agent| agent.tools.iter())
            .any(|tool| tool.starts_with("browser."))
        {
            let mutation_state = if config
                .agents
                .agents
                .values()
                .flat_map(|agent| agent.tools.iter())
                .any(|tool| frankclaw_tools::tool_requires_operator_approval(tool))
            {
                if policy.allow_browser_mutations {
                    "mutations enabled"
                } else {
                    "mutations gated"
                }
            } else {
                "read-only"
            };
            Some(format!(
                "{} at {}",
                mutation_state,
                browser_endpoint.unwrap_or("http://127.0.0.1:9222/")
            ))
        } else {
            None
        }
    } else {
        Some(warnings.join(" | "))
    }
}

fn read_password() -> anyhow::Result<secrecy::SecretString> {
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("failed to read password")?;
    Ok(secrecy::SecretString::from(input.trim().to_string()))
}

fn build_onboard_config(
    channel: &str,
    gateway_token: &str,
) -> anyhow::Result<frankclaw_core::config::FrankClawConfig> {
    use frankclaw_core::auth::AuthMode;
    use frankclaw_core::config::{ChannelConfig, ProviderConfig};
    use frankclaw_core::types::ChannelId;

    let mut config = frankclaw_core::config::FrankClawConfig::default();
    config.gateway.auth = AuthMode::Token {
        token: Some(secrecy::SecretString::from(gateway_token.to_string())),
    };
    config.models.providers = vec![ProviderConfig {
        id: "openai".into(),
        api: "openai".into(),
        base_url: None,
        api_key_ref: Some("OPENAI_API_KEY".into()),
        models: vec!["gpt-4o-mini".into()],
        cooldown_secs: 30,
    }];
    config.models.default_model = Some("gpt-4o-mini".into());

    let channel_config = match channel {
        "web" => ChannelConfig {
            enabled: true,
            accounts: Vec::new(),
            extra: serde_json::json!({
                "dm_policy": "open"
            }),
        },
        "telegram" => ChannelConfig {
            enabled: true,
            accounts: vec![serde_json::json!({
                "bot_token_env": "TELEGRAM_BOT_TOKEN"
            })],
            extra: serde_json::json!({}),
        },
        "whatsapp" => ChannelConfig {
            enabled: true,
            accounts: vec![serde_json::json!({
                "access_token_env": "WHATSAPP_ACCESS_TOKEN",
                "phone_number_id_env": "WHATSAPP_PHONE_NUMBER_ID",
                "verify_token_env": "WHATSAPP_VERIFY_TOKEN",
                "app_secret_env": "WHATSAPP_APP_SECRET"
            })],
            extra: serde_json::json!({}),
        },
        "slack" => ChannelConfig {
            enabled: true,
            accounts: vec![serde_json::json!({
                "app_token_env": "SLACK_APP_TOKEN",
                "bot_token_env": "SLACK_BOT_TOKEN"
            })],
            extra: serde_json::json!({}),
        },
        "discord" => ChannelConfig {
            enabled: true,
            accounts: vec![serde_json::json!({
                "bot_token_env": "DISCORD_BOT_TOKEN"
            })],
            extra: serde_json::json!({}),
        },
        "signal" => ChannelConfig {
            enabled: true,
            accounts: vec![serde_json::json!({
                "base_url_env": "SIGNAL_BASE_URL",
                "account_env": "SIGNAL_ACCOUNT"
            })],
            extra: serde_json::json!({}),
        },
        other => anyhow::bail!(
            "unsupported onboard channel '{}'; expected web, telegram, whatsapp, slack, discord, or signal",
            other
        ),
    };
    config.channels.insert(ChannelId::new(channel), channel_config);
    Ok(config)
}

fn supported_channel_example(channel: &str) -> Option<&'static str> {
    match channel.trim() {
        "web" => Some(include_str!("../../../examples/channels/web.json")),
        "telegram" => Some(include_str!("../../../examples/channels/telegram.json")),
        "discord" => Some(include_str!("../../../examples/channels/discord.json")),
        "slack" => Some(include_str!("../../../examples/channels/slack.json")),
        "signal" => Some(include_str!("../../../examples/channels/signal.json")),
        "whatsapp" => Some(include_str!("../../../examples/channels/whatsapp.json")),
        _ => None,
    }
}

fn render_systemd_unit(
    executable: &std::path::Path,
    config_path: &std::path::Path,
    state_dir: &std::path::Path,
) -> String {
    format!(
        "[Unit]\nDescription=FrankClaw Gateway\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={} gateway --config {} --state-dir {}\nWorkingDirectory={}\nRestart=on-failure\nRestartSec=5\nEnvironment=RUST_LOG=info\n# Environment=FRANKCLAW_MASTER_KEY=...\n\n[Install]\nWantedBy=default.target\n",
        executable.display(),
        config_path.display(),
        state_dir.display(),
        state_dir.display(),
    )
}

fn rewrite_last_reply_metadata_for_edit(
    metadata: &mut serde_json::Value,
    new_text: &str,
) -> anyhow::Result<frankclaw_gateway::delivery::StoredReplyMetadata> {
    let mut last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(metadata)
        .context("session has no tracked delivery metadata")?;

    if last_reply.chunks.len() > 1 {
        anyhow::bail!("editing chunked replies is not supported yet");
    }

    last_reply.content = new_text.to_string();
    if let Some(first_chunk) = last_reply.chunks.first_mut() {
        first_chunk.content = new_text.to_string();
    }

    frankclaw_gateway::delivery::set_last_reply_in_metadata(metadata, &last_reply)
        .context("failed to update delivery metadata")?;
    Ok(last_reply)
}

fn delete_targets_from_last_reply(
    last_reply: &frankclaw_gateway::delivery::StoredReplyMetadata,
) -> anyhow::Result<Vec<String>> {
    let targets = if last_reply.chunks.is_empty() {
        last_reply
            .platform_message_id
            .clone()
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        last_reply
            .chunks
            .iter()
            .filter_map(|chunk| chunk.platform_message_id.clone())
            .collect::<Vec<_>>()
    };

    if targets.is_empty() {
        anyhow::bail!("tracked reply is missing platform message ids");
    }

    Ok(targets)
}

fn mark_last_reply_metadata_deleted(
    metadata: &mut serde_json::Value,
) -> anyhow::Result<frankclaw_gateway::delivery::StoredReplyMetadata> {
    let mut last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(metadata)
        .context("session has no tracked delivery metadata")?;

    last_reply.status = "deleted".into();
    last_reply.platform_message_id = None;
    for chunk in &mut last_reply.chunks {
        chunk.status = "deleted".into();
        chunk.platform_message_id = None;
    }

    frankclaw_gateway::delivery::set_last_reply_in_metadata(metadata, &last_reply)
        .context("failed to update delivery metadata")?;
    Ok(last_reply)
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

fn open_media_store(
    config: &frankclaw_core::config::FrankClawConfig,
    state_dir: &std::path::Path,
) -> anyhow::Result<std::sync::Arc<frankclaw_media::MediaStore>> {
    let media_dir = config
        .media
        .storage_path
        .clone()
        .unwrap_or_else(|| state_dir.join("media"));
    Ok(std::sync::Arc::new(
        frankclaw_media::MediaStore::new(
            media_dir,
            config.media.max_file_size_bytes,
            config.media.ttl_hours,
        )
        .context("failed to open media store")?,
    ))
}

fn restrict_file_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::config::{ChannelConfig, ProviderConfig};
    use frankclaw_core::types::ChannelId;

    fn reply_metadata_json(
        chunks: Vec<frankclaw_gateway::delivery::StoredReplyChunk>,
    ) -> serde_json::Value {
        let reply = frankclaw_gateway::delivery::StoredReplyMetadata {
            channel: "telegram".into(),
            account_id: "default".into(),
            recipient_id: "user-1".into(),
            thread_id: Some("thread-1".into()),
            reply_to: Some("incoming-1".into()),
            content: "old reply".into(),
            platform_message_id: Some("msg-1".into()),
            status: "sent".into(),
            attempts: 1,
            retry_after_secs: None,
            error: None,
            chunks,
            recorded_at: chrono::Utc::now(),
        };
        serde_json::json!({
            "delivery": {
                "last_reply": reply
            },
            "other": {
                "preserve": true
            }
        })
    }

    #[test]
    fn onboard_whatsapp_profile_uses_env_refs_and_token_auth() {
        let config = build_onboard_config("whatsapp", "gateway-token")
            .expect("onboard config should build");

        assert!(matches!(
            config.gateway.auth,
            frankclaw_core::auth::AuthMode::Token { .. }
        ));
        let channel = config
            .channels
            .get(&frankclaw_core::types::ChannelId::new("whatsapp"))
            .expect("whatsapp channel should exist");
        assert_eq!(
            channel.accounts[0]["access_token_env"],
            serde_json::json!("WHATSAPP_ACCESS_TOKEN")
        );
        assert_eq!(
            channel.accounts[0]["app_secret_env"],
            serde_json::json!("WHATSAPP_APP_SECRET")
        );
    }

    #[test]
    fn render_systemd_unit_contains_execstart() {
        let unit = render_systemd_unit(
            std::path::Path::new("/usr/local/bin/frankclaw"),
            std::path::Path::new("/etc/frankclaw.json"),
            std::path::Path::new("/var/lib/frankclaw"),
        );

        assert!(unit.contains("ExecStart=/usr/local/bin/frankclaw gateway --config /etc/frankclaw.json --state-dir /var/lib/frankclaw"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn collect_doctor_warnings_flags_missing_envs_and_unsigned_whatsapp_webhooks() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.models.providers = vec![ProviderConfig {
            id: "openai".into(),
            api: "openai".into(),
            base_url: None,
            api_key_ref: Some("FRANKCLAW_TEST_MISSING_OPENAI_KEY".into()),
            models: vec!["gpt-4o-mini".into()],
            cooldown_secs: 30,
        }];
        config.channels.insert(
            ChannelId::new("whatsapp"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "access_token_env": "FRANKCLAW_TEST_MISSING_WHATSAPP_TOKEN",
                    "phone_number_id_env": "FRANKCLAW_TEST_MISSING_WHATSAPP_PHONE",
                    "verify_token_env": "FRANKCLAW_TEST_MISSING_WHATSAPP_VERIFY"
                })],
                extra: serde_json::json!({}),
            },
        );

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let missing_state_dir = std::env::temp_dir().join(format!(
            "frankclaw-cli-missing-state-{}-{}",
            std::process::id(),
            unique
        ));
        let warnings = collect_doctor_warnings(&config, &missing_state_dir)
            .expect("doctor warnings should collect");

        assert!(warnings
            .iter()
            .any(|warning| warning.contains("FRANKCLAW_TEST_MISSING_OPENAI_KEY")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("FRANKCLAW_TEST_MISSING_WHATSAPP_TOKEN")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("WHATSAPP_APP_SECRET"))
            || warnings.iter().any(|warning| warning.contains("app_secret configured")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("does not exist yet")));
    }

    #[test]
    fn collect_doctor_warnings_flags_inline_secrets_and_open_group_surface() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("discord"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "inline-secret"
                })],
                extra: serde_json::json!({
                    "require_mention_for_groups": false
                }),
            },
        );

        let existing_state_dir = std::env::temp_dir().join(format!(
            "frankclaw-cli-existing-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&existing_state_dir).expect("state dir should create");

        let warnings = collect_doctor_warnings(&config, &existing_state_dir)
            .expect("doctor warnings should collect");

        assert!(warnings.iter().any(|warning| warning.contains("stores 'bot_token' inline")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("accepts group messages without mention gating")));

        let _ = std::fs::remove_dir_all(existing_state_dir);
    }

    #[test]
    fn rewrite_last_reply_metadata_for_edit_updates_content_and_preserves_other_metadata() {
        let mut metadata = reply_metadata_json(vec![frankclaw_gateway::delivery::StoredReplyChunk {
            content: "old reply".into(),
            platform_message_id: Some("msg-1".into()),
            status: "sent".into(),
            attempts: 1,
            retry_after_secs: None,
            error: None,
        }]);

        let rewritten = rewrite_last_reply_metadata_for_edit(&mut metadata, "new reply")
            .expect("metadata rewrite should succeed");

        assert_eq!(rewritten.content, "new reply");
        assert_eq!(rewritten.chunks[0].content, "new reply");
        assert_eq!(metadata["other"]["preserve"], serde_json::json!(true));
        assert_eq!(
            frankclaw_gateway::delivery::last_reply_from_metadata(&metadata)
                .expect("last reply should exist")
                .content,
            "new reply"
        );
    }

    #[test]
    fn rewrite_last_reply_metadata_for_edit_rejects_chunked_replies() {
        let mut metadata = reply_metadata_json(vec![
            frankclaw_gateway::delivery::StoredReplyChunk {
                content: "first".into(),
                platform_message_id: Some("msg-1".into()),
                status: "sent".into(),
                attempts: 1,
                retry_after_secs: None,
                error: None,
            },
            frankclaw_gateway::delivery::StoredReplyChunk {
                content: "second".into(),
                platform_message_id: Some("msg-2".into()),
                status: "sent".into(),
                attempts: 1,
                retry_after_secs: None,
                error: None,
            },
        ]);

        let err = rewrite_last_reply_metadata_for_edit(&mut metadata, "new reply")
            .expect_err("chunked replies should be rejected");

        assert!(err.to_string().contains("chunked replies"));
    }

    #[test]
    fn delete_targets_from_last_reply_prefers_chunk_ids_when_present() {
        let metadata = reply_metadata_json(vec![
            frankclaw_gateway::delivery::StoredReplyChunk {
                content: "first".into(),
                platform_message_id: Some("msg-1".into()),
                status: "sent".into(),
                attempts: 1,
                retry_after_secs: None,
                error: None,
            },
            frankclaw_gateway::delivery::StoredReplyChunk {
                content: "second".into(),
                platform_message_id: Some("msg-2".into()),
                status: "sent".into(),
                attempts: 1,
                retry_after_secs: None,
                error: None,
            },
        ]);
        let last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(&metadata)
            .expect("last reply should exist");

        let targets = delete_targets_from_last_reply(&last_reply)
            .expect("delete targets should resolve");

        assert_eq!(targets, vec!["msg-1".to_string(), "msg-2".to_string()]);
    }

    #[test]
    fn mark_last_reply_metadata_deleted_clears_platform_ids_and_marks_chunks() {
        let mut metadata = reply_metadata_json(vec![frankclaw_gateway::delivery::StoredReplyChunk {
            content: "old reply".into(),
            platform_message_id: Some("msg-1".into()),
            status: "sent".into(),
            attempts: 1,
            retry_after_secs: None,
            error: None,
        }]);

        let deleted = mark_last_reply_metadata_deleted(&mut metadata)
            .expect("metadata delete should succeed");

        assert_eq!(deleted.status, "deleted");
        assert!(deleted.platform_message_id.is_none());
        assert_eq!(deleted.chunks[0].status, "deleted");
        assert!(deleted.chunks[0].platform_message_id.is_none());
        assert_eq!(metadata["other"]["preserve"], serde_json::json!(true));
    }

    #[test]
    fn supported_channel_examples_parse_as_json() {
        let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/channels");

        for filename in [
            "web.json",
            "telegram.json",
            "discord.json",
            "slack.json",
            "signal.json",
            "whatsapp.json",
        ] {
            let path = examples_dir.join(filename);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {}: {}", path.display(), err));
            serde_json::from_str::<serde_json::Value>(&content)
                .unwrap_or_else(|err| panic!("invalid JSON in {}: {}", path.display(), err));
        }
    }

    #[test]
    fn supported_channel_example_returns_embedded_snippet() {
        let example = supported_channel_example("telegram")
            .expect("telegram example should exist");

        assert!(example.contains("TELEGRAM_BOT_TOKEN"));
        assert!(supported_channel_example("matrix").is_none());
    }

    #[test]
    fn docker_compose_template_includes_gateway_and_browser_services() {
        let compose_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docker-compose.yml");
        let content = std::fs::read_to_string(&compose_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {}", compose_path.display(), err));

        assert!(content.contains("gateway:"));
        assert!(content.contains("chromium:"));
        assert!(content.contains("FRANKCLAW_BROWSER_DEVTOOLS_URL: http://chromium:9222/"));
        assert!(content.contains("./frankclaw.json:/config/frankclaw.json:ro"));
    }

    #[test]
    fn build_onboard_config_rejects_unknown_channel_profiles() {
        let err = build_onboard_config("matrix", "gateway-token")
            .expect_err("unsupported channel should fail");

        assert!(err.to_string().contains("unsupported onboard channel"));
    }

    #[test]
    fn collect_browser_tool_warnings_flags_unreachable_non_loopback_devtools() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&frankclaw_core::types::AgentId::default_agent())
            .expect("default agent should exist")
            .tools = vec!["browser.close".into()];

        let warnings = collect_browser_tool_warnings(&config, Some("http://10.0.0.8:6553/"));

        assert!(warnings
            .iter()
            .any(|warning| warning.contains("non-loopback host")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("docker compose up -d chromium")));
    }

    #[test]
    fn collect_browser_tool_warnings_flags_gated_mutation_tools() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&frankclaw_core::types::AgentId::default_agent())
            .expect("default agent should exist")
            .tools = vec!["browser.type".into()];

        let warnings = collect_browser_tool_warnings_with_policy(
            &config,
            Some("http://127.0.0.1:9222/"),
            frankclaw_tools::ToolPolicy {
                allow_browser_mutations: false,
            },
        );

        assert!(warnings
            .iter()
            .any(|warning| warning.contains("FRANKCLAW_ALLOW_BROWSER_MUTATIONS=1")));
    }

    #[test]
    fn browser_runtime_status_reports_mutation_gate_state() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&frankclaw_core::types::AgentId::default_agent())
            .expect("default agent should exist")
            .tools = vec!["browser.open".into(), "browser.click".into()];

        let gated = browser_runtime_status_with_policy(
            &config,
            Some("http://127.0.0.1:9222/"),
            frankclaw_tools::ToolPolicy {
                allow_browser_mutations: false,
            },
        )
        .expect("status should exist");
        assert!(gated.contains("blocked until FRANKCLAW_ALLOW_BROWSER_MUTATIONS=1"));

        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("listener should bind");
        let endpoint = format!("http://{}", listener.local_addr().expect("addr should exist"));
        let enabled = browser_runtime_status_with_policy(
            &config,
            Some(&endpoint),
            frankclaw_tools::ToolPolicy {
                allow_browser_mutations: true,
            },
        )
        .expect("status should exist");
        assert!(enabled.contains(&format!("mutations enabled at {}", endpoint)));
    }
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
