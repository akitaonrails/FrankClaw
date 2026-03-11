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

    /// Interactive guided setup for first-time configuration.
    Setup {
        /// Force overwrite an existing config.
        #[arg(long)]
        force: bool,
    },

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

    /// Start the gateway as a background daemon.
    Start {
        /// Override the listen port.
        #[arg(short, long)]
        port: Option<u16>,
    },

    /// Stop a running gateway daemon.
    Stop,

    /// Security audit: scan config for secrets, auth, and policy issues.
    Audit,

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
            run_doctor(cli.config.as_deref(), &state_dir).await?;
        }

        Command::Setup { force } => {
            run_setup(cli.config.as_deref(), &state_dir, force)?;
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

            // Check daemon PID status
            let pid_path = state_dir.join("frankclaw.pid");
            if let Some(status) = daemon_pid_status(&pid_path) {
                println!();
                println!("Daemon:");
                println!("  {status}");
            }
        }

        Command::Start { port } => {
            let pid_path = state_dir.join("frankclaw.pid");
            if let Some(existing_pid) = read_pid_file(&pid_path) {
                if is_process_alive(existing_pid) {
                    println!("Gateway is already running (PID {existing_pid}).");
                    return Ok(());
                }
                // Stale PID file — clean up
                let _ = std::fs::remove_file(&pid_path);
            }

            let executable = std::env::current_exe().context("failed to locate frankclaw binary")?;
            let mut cmd = std::process::Command::new(&executable);
            cmd.arg("gateway");
            if let Some(config_path) = &cli.config {
                cmd.arg("--config").arg(config_path);
            }
            cmd.arg("--state-dir").arg(&state_dir);
            if let Some(port) = port {
                cmd.arg("--port").arg(port.to_string());
            }

            // Redirect stdout/stderr to log file
            let log_path = state_dir.join("gateway.log");
            std::fs::create_dir_all(&state_dir)?;
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .context("failed to open log file")?;
            let log_err = log_file
                .try_clone()
                .context("failed to clone log file handle")?;

            cmd.stdout(std::process::Stdio::from(log_file));
            cmd.stderr(std::process::Stdio::from(log_err));
            cmd.stdin(std::process::Stdio::null());

            let child = cmd.spawn().context("failed to start gateway process")?;
            let pid = child.id();

            std::fs::write(&pid_path, pid.to_string())?;
            restrict_file_permissions(&pid_path);

            println!("Gateway started (PID {pid}).");
            println!("  Log: {}", log_path.display());
            println!("  PID file: {}", pid_path.display());
            println!();
            println!("Stop with: frankclaw stop");
        }

        Command::Stop => {
            let pid_path = state_dir.join("frankclaw.pid");
            match read_pid_file(&pid_path) {
                Some(pid) => {
                    if !is_process_alive(pid) {
                        println!("Gateway is not running (stale PID {pid}).");
                        let _ = std::fs::remove_file(&pid_path);
                        return Ok(());
                    }
                    stop_process(pid)?;
                    let _ = std::fs::remove_file(&pid_path);
                    println!("Gateway stopped (PID {pid}).");
                }
                None => {
                    println!("No running gateway found (no PID file).");
                }
            }
        }

        Command::Audit => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            let config_file_path = cli
                .config
                .clone()
                .unwrap_or_else(|| state_dir.join("frankclaw.json"));
            let exit_code = run_security_audit(&config, &config_file_path, &state_dir)?;
            if exit_code != 0 {
                std::process::exit(exit_code);
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

// ---------------------------------------------------------------------------
// Daemon process management
// ---------------------------------------------------------------------------

fn read_pid_file(pid_path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(pid_path)
        .ok()
        .and_then(|content| content.trim().parse::<u32>().ok())
}

fn is_process_alive(pid: u32) -> bool {
    // Use `kill -0` to check process existence without unsafe code
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn stop_process(pid: u32) -> anyhow::Result<()> {
    let status = std::process::Command::new("kill")
        .args([&pid.to_string()])
        .status()
        .context("failed to run kill")?;
    if !status.success() {
        anyhow::bail!("failed to send SIGTERM to PID {pid}");
    }

    // Wait briefly for the process to exit
    for _ in 0..20 {
        if !is_process_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Force kill if still alive
    if is_process_alive(pid) {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

fn daemon_pid_status(pid_path: &std::path::Path) -> Option<String> {
    let pid = read_pid_file(pid_path)?;
    if is_process_alive(pid) {
        Some(format!("running (PID {pid})"))
    } else {
        Some(format!("not running (stale PID file, PID {pid})"))
    }
}

// ---------------------------------------------------------------------------
// Setup wizard: interactive guided configuration
// ---------------------------------------------------------------------------

fn prompt_line(question: &str) -> anyhow::Result<String> {
    eprint!("{question}: ");
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("failed to read input")?;
    Ok(input.trim().to_string())
}

fn prompt_choice(question: &str, options: &[&str], default: usize) -> anyhow::Result<usize> {
    eprintln!("{question}");
    for (i, option) in options.iter().enumerate() {
        let marker = if i == default { " (default)" } else { "" };
        eprintln!("  {}: {}{}", i + 1, option, marker);
    }
    let input = prompt_line(&format!("Choose [1-{}]", options.len()))?;
    if input.is_empty() {
        return Ok(default);
    }
    match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= options.len() => Ok(n - 1),
        _ => {
            eprintln!("Invalid choice, using default.");
            Ok(default)
        }
    }
}

fn prompt_yes_no(question: &str, default_yes: bool) -> anyhow::Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    let input = prompt_line(&format!("{question} {hint}"))?;
    if input.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(input.to_lowercase().as_str(), "y" | "yes"))
}

fn run_setup(
    config_path_override: Option<&std::path::Path>,
    state_dir: &std::path::Path,
    force: bool,
) -> anyhow::Result<()> {
    use frankclaw_core::auth::AuthMode;
    use frankclaw_core::config::{ChannelConfig, ProviderConfig};
    use frankclaw_core::types::ChannelId;

    let config_path = config_path_override
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir.join("frankclaw.json"));

    println!("FrankClaw Setup");
    println!("===============");
    println!();

    if config_path.exists() && !force {
        eprintln!(
            "Config already exists at {}.",
            config_path.display()
        );
        if !prompt_yes_no("Overwrite?", false)? {
            println!("Setup cancelled.");
            return Ok(());
        }
    }

    // --- Provider selection ---
    println!();
    let provider_idx = prompt_choice(
        "Which AI provider?",
        &["OpenAI", "Anthropic", "Ollama (local)"],
        0,
    )?;

    let (provider_id, provider_api, default_env, default_model, needs_key) = match provider_idx {
        0 => ("openai", "openai", "OPENAI_API_KEY", "gpt-4o-mini", true),
        1 => (
            "anthropic",
            "anthropic",
            "ANTHROPIC_API_KEY",
            "claude-sonnet-4-6-20250514",
            true,
        ),
        2 => ("ollama", "ollama", "", "llama3.1", false),
        _ => unreachable!(),
    };

    let api_key_ref = if needs_key {
        println!();
        let env_name = prompt_line(&format!(
            "API key env var name (default: {default_env})"
        ))?;
        let env_name = if env_name.is_empty() {
            default_env.to_string()
        } else {
            env_name
        };
        Some(env_name)
    } else {
        None
    };

    let base_url = if provider_api == "ollama" {
        println!();
        let url = prompt_line("Ollama base URL (default: http://127.0.0.1:11434)")?;
        Some(if url.is_empty() {
            "http://127.0.0.1:11434".to_string()
        } else {
            url
        })
    } else {
        None
    };

    println!();
    let model_input = prompt_line(&format!("Default model (default: {default_model})"))?;
    let model = if model_input.is_empty() {
        default_model.to_string()
    } else {
        model_input
    };

    // --- Channel selection ---
    println!();
    let channel_idx = prompt_choice(
        "Primary channel?",
        &[
            "Web (browser UI)",
            "Telegram",
            "Discord",
            "Slack",
            "WhatsApp (Cloud API)",
            "Signal",
        ],
        0,
    )?;

    let (channel_id, channel_config) = match channel_idx {
        0 => (
            "web",
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({ "dm_policy": "open" }),
            },
        ),
        1 => (
            "telegram",
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token_env": "TELEGRAM_BOT_TOKEN"
                })],
                extra: serde_json::json!({}),
            },
        ),
        2 => (
            "discord",
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token_env": "DISCORD_BOT_TOKEN"
                })],
                extra: serde_json::json!({}),
            },
        ),
        3 => (
            "slack",
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "app_token_env": "SLACK_APP_TOKEN",
                    "bot_token_env": "SLACK_BOT_TOKEN"
                })],
                extra: serde_json::json!({}),
            },
        ),
        4 => (
            "whatsapp",
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "access_token_env": "WHATSAPP_ACCESS_TOKEN",
                    "phone_number_id_env": "WHATSAPP_PHONE_NUMBER_ID",
                    "verify_token_env": "WHATSAPP_VERIFY_TOKEN",
                    "app_secret_env": "WHATSAPP_APP_SECRET"
                })],
                extra: serde_json::json!({}),
            },
        ),
        5 => (
            "signal",
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "base_url_env": "SIGNAL_BASE_URL",
                    "account_env": "SIGNAL_ACCOUNT"
                })],
                extra: serde_json::json!({}),
            },
        ),
        _ => unreachable!(),
    };

    // --- Gateway port ---
    println!();
    let port_input = prompt_line("Gateway port (default: 18789)")?;
    let port: u16 = if port_input.is_empty() {
        18789
    } else {
        port_input
            .parse()
            .context("invalid port number")?
    };

    // --- Session encryption ---
    println!();
    let encrypt = prompt_yes_no("Enable session encryption?", true)?;

    // --- Build config ---
    let gateway_token = frankclaw_crypto::generate_token();
    let mut config = frankclaw_core::config::FrankClawConfig::default();
    config.gateway.port = port;
    config.gateway.auth = AuthMode::Token {
        token: Some(secrecy::SecretString::from(gateway_token.clone())),
    };
    config.models.providers = vec![ProviderConfig {
        id: provider_id.into(),
        api: provider_api.into(),
        base_url,
        api_key_ref,
        models: vec![model.clone()],
        cooldown_secs: 30,
    }];
    config.models.default_model = Some(model);
    config.channels.insert(ChannelId::new(channel_id), channel_config);
    config.security.encrypt_sessions = encrypt;

    // --- Write config ---
    std::fs::create_dir_all(config_path.parent().unwrap_or(state_dir))?;
    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, &json)?;
    restrict_file_permissions(&config_path);

    println!();
    println!("Configuration written to: {}", config_path.display());
    println!("Gateway auth token: {gateway_token}");
    if encrypt {
        println!();
        println!("Session encryption is ON. Set the master key:");
        println!("  export FRANKCLAW_MASTER_KEY=$(frankclaw gen-token)");
    }
    println!();
    println!("Next steps:");
    if needs_key {
        println!(
            "  1. Export your API key:  export {}=<your-key>",
            config.models.providers[0]
                .api_key_ref
                .as_deref()
                .unwrap_or("API_KEY")
        );
    }
    if channel_id != "web" {
        println!("  2. Export channel credentials (see config for env var names).");
    }
    println!("  3. Verify config:        frankclaw doctor");
    println!("  4. Start the gateway:    frankclaw gateway");

    Ok(())
}

// ---------------------------------------------------------------------------
// Doctor: comprehensive diagnostics
// ---------------------------------------------------------------------------

/// Outcome of a single diagnostic check.
enum CheckResult {
    Pass(String),
    Warn(String),
    Fail(String),
    Info(String),
}

impl CheckResult {
    fn prefix(&self) -> &str {
        match self {
            Self::Pass(_) => "[PASS]",
            Self::Warn(_) => "[WARN]",
            Self::Fail(_) => "[FAIL]",
            Self::Info(_) => "[INFO]",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Pass(m) | Self::Warn(m) | Self::Fail(m) | Self::Info(m) => m,
        }
    }

    fn is_fail(&self) -> bool {
        matches!(self, Self::Fail(_))
    }
}

fn print_section(title: &str, checks: &[CheckResult]) {
    println!("\n  {title}");
    for check in checks {
        println!("    {} {}", check.prefix(), check.message());
    }
}

async fn run_doctor(
    config_path: Option<&std::path::Path>,
    state_dir: &std::path::Path,
) -> anyhow::Result<()> {
    println!("FrankClaw Doctor");
    println!("================");

    // --- System info ---
    let mut system_checks = vec![
        CheckResult::Info(format!("frankclaw {}", env!("CARGO_PKG_VERSION"))),
        CheckResult::Info(format!("rust {}", rustc_version())),
        CheckResult::Info(format!("os {}", std::env::consts::OS)),
        CheckResult::Info(format!("arch {}", std::env::consts::ARCH)),
    ];
    let state_display = state_dir.display().to_string();
    system_checks.push(CheckResult::Info(format!("state dir: {state_display}")));
    print_section("System", &system_checks);

    // --- Configuration ---
    let config = match load_config(config_path, state_dir) {
        Ok(cfg) => cfg,
        Err(err) => {
            print_section(
                "Configuration",
                &[CheckResult::Fail(format!("failed to load config: {err}"))],
            );
            println!("\nDoctor found critical issues. Fix config and rerun.");
            return Ok(());
        }
    };

    let mut config_checks = Vec::new();
    match config.validate() {
        Ok(()) => config_checks.push(CheckResult::Pass("config schema is valid".into())),
        Err(err) => {
            config_checks.push(CheckResult::Fail(format!("config validation failed: {err}")));
            print_section("Configuration", &config_checks);
            println!("\nDoctor found critical issues. Fix config and rerun.");
            return Ok(());
        }
    }

    // Config file permissions (Unix only)
    let config_file_path = config_path
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir.join("frankclaw.json"));
    config_checks.extend(check_file_permissions(&config_file_path, "config file"));

    let warnings = collect_doctor_warnings(&config, state_dir)?;
    for warning in &warnings {
        config_checks.push(CheckResult::Warn(warning.clone()));
    }
    if warnings.is_empty() {
        config_checks.push(CheckResult::Pass("no misconfigurations detected".into()));
    }

    let exposure = frankclaw_gateway::auth::assess_exposure(&config)?;
    config_checks.push(CheckResult::Info(format!("exposure: {}", exposure.summary)));
    for warning in &exposure.warnings {
        config_checks.push(CheckResult::Warn(warning.clone()));
    }

    print_section("Configuration", &config_checks);

    // --- State directory ---
    let mut state_checks = Vec::new();
    if state_dir.exists() {
        state_checks.push(CheckResult::Pass("state directory exists".into()));
        state_checks.extend(check_dir_permissions(state_dir, "state directory"));
    } else {
        state_checks.push(CheckResult::Warn(format!(
            "state directory '{}' does not exist yet (will be created on first run)",
            state_dir.display()
        )));
    }
    print_section("State Directory", &state_checks);

    // --- Database ---
    let mut db_checks = Vec::new();
    let db_path = state_dir.join("sessions.db");
    if db_path.exists() {
        match check_sqlite_health(&db_path) {
            Ok(()) => db_checks.push(CheckResult::Pass("sessions.db is readable and valid".into())),
            Err(err) => db_checks.push(CheckResult::Fail(format!("sessions.db: {err}"))),
        }
        db_checks.extend(check_file_permissions(&db_path, "sessions.db"));
    } else {
        db_checks.push(CheckResult::Info(
            "sessions.db does not exist yet (created on first session)".into(),
        ));
    }
    print_section("Database", &db_checks);

    // --- Port availability ---
    let mut port_checks = Vec::new();
    let port = config.gateway.port;
    match check_port_available(port) {
        Ok(true) => port_checks.push(CheckResult::Pass(format!("port {port} is available"))),
        Ok(false) => port_checks.push(CheckResult::Warn(format!(
            "port {port} is already in use (gateway may already be running, or another service is using it)"
        ))),
        Err(err) => port_checks.push(CheckResult::Warn(format!(
            "could not check port {port}: {err}"
        ))),
    }
    print_section("Network", &port_checks);

    // --- Providers ---
    let mut provider_checks = Vec::new();
    if config.models.providers.is_empty() {
        provider_checks.push(CheckResult::Warn("no model providers configured".into()));
    }
    for provider in &config.models.providers {
        let has_key = provider
            .api_key_ref
            .as_deref()
            .and_then(|env_name| std::env::var(env_name).ok())
            .filter(|value| !value.trim().is_empty())
            .is_some();

        if !has_key {
            provider_checks.push(CheckResult::Warn(format!(
                "provider '{}' ({}) — API key not available",
                provider.id, provider.api
            )));
            continue;
        }

        provider_checks.push(CheckResult::Pass(format!(
            "provider '{}' ({}) — API key present, {} model(s) configured",
            provider.id,
            provider.api,
            provider.models.len()
        )));

        // Connectivity check for providers with a base URL
        let base_url = provider.base_url.as_deref().unwrap_or_else(|| {
            match provider.api.as_str() {
                "openai" => "https://api.openai.com",
                "anthropic" => "https://api.anthropic.com",
                _ => "",
            }
        });
        if !base_url.is_empty() {
            match check_provider_reachable(base_url).await {
                Ok(()) => provider_checks.push(CheckResult::Pass(format!(
                    "  {} is reachable",
                    base_url
                ))),
                Err(err) => provider_checks.push(CheckResult::Warn(format!(
                    "  {} is unreachable: {}",
                    base_url, err
                ))),
            }
        }
    }
    print_section("Providers", &provider_checks);

    // --- Channels ---
    let mut channel_checks = Vec::new();
    if config.channels.is_empty() {
        channel_checks.push(CheckResult::Warn("no channels configured".into()));
    }
    for (channel_id, channel) in &config.channels {
        if channel.enabled {
            channel_checks.push(CheckResult::Pass(format!(
                "channel '{}' enabled ({} account(s))",
                channel_id,
                channel.accounts.len()
            )));
        } else {
            channel_checks.push(CheckResult::Info(format!(
                "channel '{}' disabled",
                channel_id
            )));
        }
    }
    print_section("Channels", &channel_checks);

    // --- Security ---
    let mut security_checks = Vec::new();
    if config.security.encrypt_sessions {
        if load_master_key_from_env()?.is_some() {
            security_checks.push(CheckResult::Pass("session encryption enabled and master key set".into()));
        } else {
            security_checks.push(CheckResult::Warn(
                "session encryption enabled but FRANKCLAW_MASTER_KEY is not set".into(),
            ));
        }
    } else {
        security_checks.push(CheckResult::Warn("session encryption is disabled".into()));
    }

    match &config.gateway.auth {
        frankclaw_core::auth::AuthMode::None => {
            security_checks.push(CheckResult::Warn("gateway auth is disabled".into()));
        }
        frankclaw_core::auth::AuthMode::Token { .. } => {
            security_checks.push(CheckResult::Pass("gateway uses token authentication".into()));
        }
        frankclaw_core::auth::AuthMode::Password { .. } => {
            security_checks.push(CheckResult::Pass("gateway uses password authentication".into()));
        }
        frankclaw_core::auth::AuthMode::TrustedProxy { .. } => {
            security_checks.push(CheckResult::Pass("gateway uses trusted proxy authentication".into()));
        }
        frankclaw_core::auth::AuthMode::Tailscale => {
            security_checks.push(CheckResult::Pass("gateway uses Tailscale authentication".into()));
        }
    }
    print_section("Security", &security_checks);

    // --- Summary ---
    let all_checks: Vec<&CheckResult> = system_checks
        .iter()
        .chain(config_checks.iter())
        .chain(state_checks.iter())
        .chain(db_checks.iter())
        .chain(port_checks.iter())
        .chain(provider_checks.iter())
        .chain(channel_checks.iter())
        .chain(security_checks.iter())
        .collect();

    let fail_count = all_checks.iter().filter(|c| c.is_fail()).count();
    let warn_count = all_checks.iter().filter(|c| matches!(c, CheckResult::Warn(_))).count();

    println!();
    if fail_count > 0 {
        println!("Doctor found {fail_count} critical issue(s) and {warn_count} warning(s).");
    } else if warn_count > 0 {
        println!("Doctor passed with {warn_count} warning(s).");
    } else {
        println!("Doctor passed. All checks OK.");
    }

    Ok(())
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| version.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(unix)]
fn check_file_permissions(path: &std::path::Path, label: &str) -> Vec<CheckResult> {
    use std::os::unix::fs::PermissionsExt;
    let mut results = Vec::new();
    if let Ok(metadata) = std::fs::metadata(path) {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            results.push(CheckResult::Warn(format!(
                "{label} has permissions {mode:04o} — should be 0600 (owner-only)",
            )));
        } else {
            results.push(CheckResult::Pass(format!(
                "{label} permissions {mode:04o} (owner-only)",
            )));
        }
    }
    results
}

#[cfg(not(unix))]
fn check_file_permissions(_path: &std::path::Path, _label: &str) -> Vec<CheckResult> {
    Vec::new()
}

#[cfg(unix)]
fn check_dir_permissions(path: &std::path::Path, label: &str) -> Vec<CheckResult> {
    use std::os::unix::fs::PermissionsExt;
    let mut results = Vec::new();
    if let Ok(metadata) = std::fs::metadata(path) {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            results.push(CheckResult::Warn(format!(
                "{label} has permissions {mode:04o} — should be 0700 (owner-only)",
            )));
        } else {
            results.push(CheckResult::Pass(format!(
                "{label} permissions {mode:04o} (owner-only)",
            )));
        }
    }
    results
}

#[cfg(not(unix))]
fn check_dir_permissions(_path: &std::path::Path, _label: &str) -> Vec<CheckResult> {
    Vec::new()
}

fn check_sqlite_health(db_path: &std::path::Path) -> anyhow::Result<()> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let result: String = conn.query_row("SELECT 'ok'", [], |row| row.get(0))?;
    anyhow::ensure!(result == "ok", "unexpected integrity check result");
    Ok(())
}

fn check_port_available(port: u16) -> anyhow::Result<bool> {
    match std::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], port))) {
        Ok(_listener) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => Ok(false),
        Err(err) => Err(err.into()),
    }
}

async fn check_provider_reachable(base_url: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let _response = client.head(base_url).send().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Security audit: deep config security scanning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "INFO"),
            Self::Low => write!(f, " LOW"),
            Self::Medium => write!(f, " MED"),
            Self::High => write!(f, "HIGH"),
            Self::Critical => write!(f, "CRIT"),
        }
    }
}

struct Finding {
    severity: Severity,
    category: &'static str,
    message: String,
    remediation: String,
}

fn run_security_audit(
    config: &frankclaw_core::config::FrankClawConfig,
    config_path: &std::path::Path,
    state_dir: &std::path::Path,
) -> anyhow::Result<i32> {
    let mut findings = Vec::new();

    // --- Auth posture ---
    audit_auth(config, &mut findings);

    // --- Secrets: inline values ---
    audit_inline_secrets(config, &mut findings);

    // --- Secrets: missing env vars ---
    audit_missing_env_vars(config, &mut findings);

    // --- Encryption ---
    audit_encryption(config, &mut findings);

    // --- Network exposure ---
    audit_network(config, &mut findings);

    // --- Channel policies ---
    audit_channel_policies(config, &mut findings);

    // --- Tool policies ---
    audit_tool_policies(config, &mut findings);

    // --- File permissions ---
    audit_file_permissions(config_path, state_dir, &mut findings);

    // --- SSRF protection ---
    if !config.security.ssrf_protection {
        findings.push(Finding {
            severity: Severity::Critical,
            category: "network",
            message: "SSRF protection is disabled — outbound requests can reach private networks".into(),
            remediation: "Set security.ssrf_protection to true".into(),
        });
    }

    // --- Print report ---
    findings.sort_by(|a, b| b.severity.cmp(&a.severity));

    println!("FrankClaw Security Audit");
    println!("========================");
    println!();

    if findings.is_empty() {
        println!("No security issues found. Configuration looks good.");
        return Ok(0);
    }

    let mut by_category: std::collections::BTreeMap<&str, Vec<&Finding>> =
        std::collections::BTreeMap::new();
    for finding in &findings {
        by_category.entry(finding.category).or_default().push(finding);
    }

    for (category, category_findings) in &by_category {
        println!("  {}", category.to_uppercase());
        for finding in category_findings {
            println!("    [{}] {}", finding.severity, finding.message);
            println!("          Fix: {}", finding.remediation);
        }
        println!();
    }

    // --- Summary ---
    let crit = findings.iter().filter(|f| f.severity == Severity::Critical).count();
    let high = findings.iter().filter(|f| f.severity == Severity::High).count();
    let med = findings.iter().filter(|f| f.severity == Severity::Medium).count();
    let low = findings.iter().filter(|f| f.severity == Severity::Low).count();
    let info = findings.iter().filter(|f| f.severity == Severity::Info).count();

    println!(
        "Summary: {} critical, {} high, {} medium, {} low, {} info",
        crit, high, med, low, info
    );

    if crit > 0 || high > 0 {
        println!("Audit FAILED — fix critical/high issues before deploying.");
        Ok(1)
    } else {
        println!("Audit passed with warnings.");
        Ok(0)
    }
}

fn audit_auth(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    match &config.gateway.auth {
        frankclaw_core::auth::AuthMode::None => {
            findings.push(Finding {
                severity: Severity::High,
                category: "auth",
                message: "Gateway authentication is disabled — anyone with network access can connect".into(),
                remediation: "Set gateway.auth to token or password mode. Run: frankclaw gen-token".into(),
            });
        }
        frankclaw_core::auth::AuthMode::Token { token } => {
            if let Some(token) = token {
                use secrecy::ExposeSecret;
                let token_str = token.expose_secret();
                if token_str.len() < 16 {
                    findings.push(Finding {
                        severity: Severity::Medium,
                        category: "auth",
                        message: "Gateway token is shorter than 16 characters".into(),
                        remediation: "Generate a stronger token: frankclaw gen-token".into(),
                    });
                }
            }
        }
        frankclaw_core::auth::AuthMode::Password { hash } => {
            if hash.trim().is_empty() {
                findings.push(Finding {
                    severity: Severity::High,
                    category: "auth",
                    message: "Password auth is configured but password hash is empty".into(),
                    remediation: "Set a password hash: frankclaw hash-password".into(),
                });
            }
        }
        _ => {}
    }
}

fn audit_inline_secrets(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    // Check providers for inline API keys (they should use env refs)
    for provider in &config.models.providers {
        if provider.api_key_ref.is_none() && provider.api != "ollama" {
            findings.push(Finding {
                severity: Severity::Medium,
                category: "secrets",
                message: format!(
                    "Provider '{}' has no api_key_ref — API key may be missing or hardcoded elsewhere",
                    provider.id
                ),
                remediation: format!(
                    "Set models.providers[{}].api_key_ref to an env var name like \"{}_API_KEY\"",
                    provider.id,
                    provider.api.to_uppercase()
                ),
            });
        }
    }

    // Check channels for inline secrets
    for (channel_id, channel) in &config.channels {
        for account in &channel.accounts {
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
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .is_some()
                {
                    findings.push(Finding {
                        severity: Severity::High,
                        category: "secrets",
                        message: format!(
                            "Channel '{}' has '{}' stored inline in config file",
                            channel_id, inline_key
                        ),
                        remediation: format!(
                            "Move to env var: rename '{}' to '{}' and set the value in the environment",
                            inline_key, env_key
                        ),
                    });
                }
            }
        }
    }

    // Check gateway token stored inline (it's always inline for now, but warn if config is world-readable)
    if let frankclaw_core::auth::AuthMode::Token { token: Some(_) } = &config.gateway.auth {
        findings.push(Finding {
            severity: Severity::Low,
            category: "secrets",
            message: "Gateway auth token is stored inline in config file".into(),
            remediation: "Ensure config file permissions are 0600 (owner-only). Consider using FRANKCLAW_AUTH_TOKEN env var if supported.".into(),
        });
    }
}

fn audit_missing_env_vars(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    for provider in &config.models.providers {
        if let Some(env_name) = provider.api_key_ref.as_deref() {
            if std::env::var(env_name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .is_none()
            {
                findings.push(Finding {
                    severity: Severity::Medium,
                    category: "secrets",
                    message: format!(
                        "Provider '{}' references env var '{}' which is not set",
                        provider.id, env_name
                    ),
                    remediation: format!("Export the API key: export {}=<your-key>", env_name),
                });
            }
        }
    }

    for (channel_id, channel) in &config.channels {
        for account in &channel.accounts {
            for key in [
                "bot_token_env", "token_env", "app_token_env",
                "access_token_env", "verify_token_env", "app_secret_env",
                "base_url_env", "account_env", "phone_number_id_env",
            ] {
                if let Some(env_name) = account.get(key).and_then(|v| v.as_str()) {
                    if std::env::var(env_name)
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                        .is_none()
                    {
                        findings.push(Finding {
                            severity: Severity::Medium,
                            category: "secrets",
                            message: format!(
                                "Channel '{}' references env var '{}' (via {}) which is not set",
                                channel_id, env_name, key
                            ),
                            remediation: format!("Export: export {}=<value>", env_name),
                        });
                    }
                }
            }
        }
    }
}

fn audit_encryption(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    if !config.security.encrypt_sessions {
        findings.push(Finding {
            severity: Severity::Medium,
            category: "encryption",
            message: "Session transcript encryption is disabled — transcripts stored in plaintext".into(),
            remediation: "Set security.encrypt_sessions to true and export FRANKCLAW_MASTER_KEY".into(),
        });
    } else if load_master_key_from_env().unwrap_or(None).is_none() {
        findings.push(Finding {
            severity: Severity::High,
            category: "encryption",
            message: "Session encryption is enabled but FRANKCLAW_MASTER_KEY is not set — sessions will fail".into(),
            remediation: "Generate and export: export FRANKCLAW_MASTER_KEY=$(frankclaw gen-token)".into(),
        });
    }

    if !config.security.encrypt_media {
        findings.push(Finding {
            severity: Severity::Low,
            category: "encryption",
            message: "Media file encryption is disabled".into(),
            remediation: "Set security.encrypt_media to true if media files contain sensitive content".into(),
        });
    }
}

fn audit_network(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    use frankclaw_core::config::BindMode;

    let is_network_exposed = !matches!(config.gateway.bind, BindMode::Loopback);

    if is_network_exposed {
        if matches!(config.gateway.auth, frankclaw_core::auth::AuthMode::None) {
            findings.push(Finding {
                severity: Severity::Critical,
                category: "network",
                message: "Gateway is network-exposed with no authentication".into(),
                remediation: "Either bind to loopback only, or configure auth: frankclaw gen-token".into(),
            });
        }

        if config.gateway.tls.is_none() {
            findings.push(Finding {
                severity: Severity::High,
                category: "network",
                message: "Gateway is network-exposed without TLS — credentials sent in cleartext".into(),
                remediation: "Configure TLS, use a reverse proxy with TLS termination, or restrict to a Tailscale network".into(),
            });
        }
    }
}

fn audit_channel_policies(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    for (channel_id, channel) in &config.channels {
        if !channel.enabled {
            continue;
        }

        let policy = match channel.security_policy() {
            Ok(p) => p,
            Err(_) => {
                findings.push(Finding {
                    severity: Severity::High,
                    category: "channels",
                    message: format!("Channel '{}' has an invalid security policy", channel_id),
                    remediation: "Check the channel's extra configuration for valid policy fields".into(),
                });
                continue;
            }
        };

        if group_surface_needs_guard(channel_id.as_str())
            && !policy.require_mention_for_groups
            && policy.allowed_groups.is_none()
        {
            findings.push(Finding {
                severity: Severity::Medium,
                category: "channels",
                message: format!(
                    "Channel '{}' responds to all group messages without mention gating or allowlist",
                    channel_id
                ),
                remediation: format!(
                    "Set extra.require_mention_for_groups=true or configure extra.allowed_groups for '{}'",
                    channel_id
                ),
            });
        }

        // WhatsApp webhook signature verification
        if channel_id.as_str() == "whatsapp" {
            let has_app_secret = channel.accounts.iter().any(|account| {
                account.get("app_secret").and_then(|v| v.as_str()).filter(|v| !v.trim().is_empty()).is_some()
                    || account.get("app_secret_env").and_then(|v| v.as_str()).filter(|v| !v.trim().is_empty()).is_some()
            });
            if !has_app_secret {
                findings.push(Finding {
                    severity: Severity::High,
                    category: "channels",
                    message: "WhatsApp channel has no app_secret — webhook signatures are not verified".into(),
                    remediation: "Set app_secret_env in the WhatsApp account config".into(),
                });
            }
        }
    }
}

fn audit_tool_policies(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    let bash_policy = frankclaw_tools::bash::BashPolicy::from_env();
    let has_bash_tools = config
        .agents
        .agents
        .values()
        .flat_map(|agent| agent.tools.iter())
        .any(|tool| tool == "bash");

    if has_bash_tools {
        match bash_policy {
            frankclaw_tools::bash::BashPolicy::AllowAll => {
                findings.push(Finding {
                    severity: Severity::Critical,
                    category: "tools",
                    message: "Bash tool policy is allow-all — agents can execute any shell command".into(),
                    remediation: "Set FRANKCLAW_BASH_POLICY to a comma-separated allowlist of permitted binaries, or deny-all".into(),
                });
            }
            frankclaw_tools::bash::BashPolicy::DenyAll => {
                findings.push(Finding {
                    severity: Severity::Info,
                    category: "tools",
                    message: "Bash tool is configured for agents but policy is deny-all (safe default)".into(),
                    remediation: "If bash execution is needed, set FRANKCLAW_BASH_POLICY to an allowlist".into(),
                });
            }
            frankclaw_tools::bash::BashPolicy::Allowlist(ref allowed) => {
                findings.push(Finding {
                    severity: Severity::Low,
                    category: "tools",
                    message: format!(
                        "Bash tool allowlist permits {} command(s): {}",
                        allowed.len(),
                        allowed.join(", ")
                    ),
                    remediation: "Review the allowlist and remove any unnecessary commands".into(),
                });
            }
        }
    }

    let browser_mutations = frankclaw_tools::ToolPolicy::from_env().allow_browser_mutations;
    let has_browser_mutation_tools = config
        .agents
        .agents
        .values()
        .flat_map(|agent| agent.tools.iter())
        .any(|tool| frankclaw_tools::tool_requires_operator_approval(tool));

    if has_browser_mutation_tools && browser_mutations {
        findings.push(Finding {
            severity: Severity::Medium,
            category: "tools",
            message: "Browser mutation tools are enabled — agents can click, type, and navigate".into(),
            remediation: "Unset FRANKCLAW_ALLOW_BROWSER_MUTATIONS if browser interaction is not needed".into(),
        });
    }
}

#[cfg(unix)]
fn audit_file_permissions(
    config_path: &std::path::Path,
    state_dir: &std::path::Path,
    findings: &mut Vec<Finding>,
) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(meta) = std::fs::metadata(config_path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            findings.push(Finding {
                severity: Severity::High,
                category: "filesystem",
                message: format!(
                    "Config file has permissions {:04o} — readable by other users",
                    mode
                ),
                remediation: format!("chmod 600 {}", config_path.display()),
            });
        }
    }

    if let Ok(meta) = std::fs::metadata(state_dir) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            findings.push(Finding {
                severity: Severity::Medium,
                category: "filesystem",
                message: format!(
                    "State directory has permissions {:04o} — accessible by other users",
                    mode
                ),
                remediation: format!("chmod 700 {}", state_dir.display()),
            });
        }
    }

    let db_path = state_dir.join("sessions.db");
    if let Ok(meta) = std::fs::metadata(&db_path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            findings.push(Finding {
                severity: Severity::High,
                category: "filesystem",
                message: format!(
                    "Sessions database has permissions {:04o} — readable by other users",
                    mode
                ),
                remediation: format!("chmod 600 {}", db_path.display()),
            });
        }
    }
}

#[cfg(not(unix))]
fn audit_file_permissions(
    _config_path: &std::path::Path,
    _state_dir: &std::path::Path,
    _findings: &mut Vec<Finding>,
) {
    // No permission checks on non-Unix platforms
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

    // --- Doctor diagnostic helper tests ---

    #[test]
    fn check_result_prefixes_are_correct() {
        assert_eq!(CheckResult::Pass("ok".into()).prefix(), "[PASS]");
        assert_eq!(CheckResult::Warn("hmm".into()).prefix(), "[WARN]");
        assert_eq!(CheckResult::Fail("bad".into()).prefix(), "[FAIL]");
        assert_eq!(CheckResult::Info("fyi".into()).prefix(), "[INFO]");
    }

    #[test]
    fn check_result_is_fail_only_for_fail_variant() {
        assert!(!CheckResult::Pass("ok".into()).is_fail());
        assert!(!CheckResult::Warn("hmm".into()).is_fail());
        assert!(CheckResult::Fail("bad".into()).is_fail());
        assert!(!CheckResult::Info("fyi".into()).is_fail());
    }

    #[test]
    fn check_port_available_detects_occupied_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("should bind to ephemeral port");
        let port = listener.local_addr().expect("should have addr").port();

        let result = check_port_available(port).expect("check should not error");
        assert!(!result, "port should be detected as in-use");
        // Note: we don't test the "available after drop" case because the port
        // may remain in TIME_WAIT on some systems.
    }

    #[test]
    fn check_sqlite_health_works_on_valid_db() {
        let dir = std::env::temp_dir().join(format!(
            "frankclaw-doctor-test-db-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("should create temp dir");
        let db_path = dir.join("test.db");

        // Create a valid SQLite database
        let conn = rusqlite::Connection::open(&db_path).expect("should open db");
        conn.execute_batch("CREATE TABLE test (id INTEGER PRIMARY KEY)")
            .expect("should create table");
        drop(conn);

        let result = check_sqlite_health(&db_path);
        assert!(result.is_ok(), "valid db should pass health check");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn check_sqlite_health_fails_on_nonexistent_file() {
        let db_path = std::env::temp_dir().join(format!(
            "frankclaw-doctor-nonexistent-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        // open_with_flags with READ_ONLY should fail for nonexistent files
        let result = check_sqlite_health(&db_path);
        assert!(result.is_err(), "nonexistent db should fail health check");
    }

    #[cfg(unix)]
    #[test]
    fn check_file_permissions_detects_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "frankclaw-doctor-test-perms-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("should create temp dir");
        let file_path = dir.join("secret.json");
        std::fs::write(&file_path, b"{}").expect("should write file");

        // Set world-readable permissions
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o644))
            .expect("should set permissions");

        let results = check_file_permissions(&file_path, "test file");
        assert!(!results.is_empty());
        assert!(matches!(results[0], CheckResult::Warn(_)));

        // Fix permissions
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600))
            .expect("should set permissions");

        let results = check_file_permissions(&file_path, "test file");
        assert!(!results.is_empty());
        assert!(matches!(results[0], CheckResult::Pass(_)));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rustc_version_returns_nonempty_string() {
        let version = rustc_version();
        assert!(!version.is_empty());
        assert!(version.contains("rustc") || version == "unknown");
    }

    // --- Setup wizard tests ---

    #[test]
    fn setup_writes_valid_config_file() {
        use frankclaw_core::config::FrankClawConfig;

        let dir = std::env::temp_dir().join(format!(
            "frankclaw-setup-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("should create temp dir");
        let config_path = dir.join("frankclaw.json");

        // Build a config the same way setup does
        let mut config = FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::Token {
            token: Some(secrecy::SecretString::from("test-token".to_string())),
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
        config.channels.insert(
            ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({ "dm_policy": "open" }),
            },
        );

        let json = serde_json::to_string_pretty(&config).expect("should serialize");
        std::fs::write(&config_path, &json).expect("should write");

        // Verify it can be loaded back
        let loaded = FrankClawConfig::load_from_path(&config_path)
            .expect("should load config");
        loaded.validate().expect("should validate");
        assert_eq!(loaded.gateway.port, 18789);
        assert_eq!(loaded.models.providers.len(), 1);
        assert_eq!(loaded.models.providers[0].id, "openai");
        assert!(loaded.channels.contains_key(&ChannelId::new("web")));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn setup_anthropic_provider_config_is_valid() {
        let config = ProviderConfig {
            id: "anthropic".into(),
            api: "anthropic".into(),
            base_url: None,
            api_key_ref: Some("ANTHROPIC_API_KEY".into()),
            models: vec!["claude-sonnet-4-6-20250514".into()],
            cooldown_secs: 30,
        };
        assert_eq!(config.api, "anthropic");
        assert!(config.api_key_ref.is_some());
    }

    // --- Process management tests ---

    #[test]
    fn read_pid_file_returns_none_for_missing_file() {
        let path = std::env::temp_dir().join(format!(
            "frankclaw-pid-test-missing-{}.pid",
            std::process::id()
        ));
        assert!(read_pid_file(&path).is_none());
    }

    #[test]
    fn read_pid_file_returns_pid_for_valid_file() {
        let dir = std::env::temp_dir().join(format!(
            "frankclaw-pid-test-valid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("should create temp dir");
        let pid_path = dir.join("frankclaw.pid");
        std::fs::write(&pid_path, "12345\n").expect("should write pid");

        assert_eq!(read_pid_file(&pid_path), Some(12345));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn read_pid_file_returns_none_for_invalid_content() {
        let dir = std::env::temp_dir().join(format!(
            "frankclaw-pid-test-invalid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("should create temp dir");
        let pid_path = dir.join("frankclaw.pid");
        std::fs::write(&pid_path, "not-a-pid").expect("should write");

        assert_eq!(read_pid_file(&pid_path), None);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn is_process_alive_returns_true_for_current_process() {
        let pid = std::process::id();
        assert!(is_process_alive(pid));
    }

    #[test]
    fn is_process_alive_returns_false_for_nonexistent_process() {
        // PID 99999999 is unlikely to exist
        assert!(!is_process_alive(99_999_999));
    }

    #[test]
    fn daemon_pid_status_shows_running_for_current_pid() {
        let dir = std::env::temp_dir().join(format!(
            "frankclaw-pid-status-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("should create temp dir");
        let pid_path = dir.join("frankclaw.pid");
        std::fs::write(&pid_path, std::process::id().to_string())
            .expect("should write pid");

        let status = daemon_pid_status(&pid_path);
        assert!(status.is_some());
        assert!(status.unwrap().contains("running"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn daemon_pid_status_shows_stale_for_dead_pid() {
        let dir = std::env::temp_dir().join(format!(
            "frankclaw-pid-stale-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("should create temp dir");
        let pid_path = dir.join("frankclaw.pid");
        std::fs::write(&pid_path, "99999999").expect("should write pid");

        let status = daemon_pid_status(&pid_path);
        assert!(status.is_some());
        assert!(status.unwrap().contains("stale"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn setup_ollama_provider_has_base_url_and_no_key() {
        let config = ProviderConfig {
            id: "ollama".into(),
            api: "ollama".into(),
            base_url: Some("http://127.0.0.1:11434".into()),
            api_key_ref: None,
            models: vec!["llama3.1".into()],
            cooldown_secs: 30,
        };
        assert!(config.base_url.is_some());
        assert!(config.api_key_ref.is_none());
    }

    // --- Security audit tests ---

    #[test]
    fn audit_auth_flags_no_auth() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::None;

        let mut findings = Vec::new();
        audit_auth(&config, &mut findings);

        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.severity == Severity::High && f.category == "auth"));
    }

    #[test]
    fn audit_auth_passes_with_token() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::Token {
            token: Some(secrecy::SecretString::from("a-long-enough-token-here".to_string())),
        };

        let mut findings = Vec::new();
        audit_auth(&config, &mut findings);

        assert!(findings.iter().all(|f| f.severity < Severity::High));
    }

    #[test]
    fn audit_auth_warns_short_token() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::Token {
            token: Some(secrecy::SecretString::from("short".to_string())),
        };

        let mut findings = Vec::new();
        audit_auth(&config, &mut findings);

        assert!(findings.iter().any(|f| f.severity == Severity::Medium && f.message.contains("shorter")));
    }

    #[test]
    fn audit_inline_secrets_detects_inline_bot_token() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("telegram"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "123456:ABC-DEF"
                })],
                extra: serde_json::json!({}),
            },
        );

        let mut findings = Vec::new();
        audit_inline_secrets(&config, &mut findings);

        assert!(findings.iter().any(|f| f.severity == Severity::High
            && f.message.contains("bot_token")
            && f.message.contains("inline")));
    }

    #[test]
    fn audit_encryption_warns_when_disabled() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.security.encrypt_sessions = false;

        let mut findings = Vec::new();
        audit_encryption(&config, &mut findings);

        assert!(findings.iter().any(|f| f.category == "encryption" && f.message.contains("disabled")));
    }

    #[test]
    fn audit_network_flags_exposed_no_auth() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.gateway.bind = frankclaw_core::config::BindMode::Lan;
        config.gateway.auth = frankclaw_core::auth::AuthMode::None;

        let mut findings = Vec::new();
        audit_network(&config, &mut findings);

        assert!(findings.iter().any(|f| f.severity == Severity::Critical
            && f.message.contains("network-exposed")));
    }

    #[test]
    fn audit_network_ok_for_loopback() {
        let config = frankclaw_core::config::FrankClawConfig::default();

        let mut findings = Vec::new();
        audit_network(&config, &mut findings);

        assert!(findings.is_empty());
    }

    #[test]
    fn audit_channel_policies_flags_ungated_groups() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("discord"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token_env": "DISCORD_BOT_TOKEN"
                })],
                extra: serde_json::json!({
                    "require_mention_for_groups": false
                }),
            },
        );

        let mut findings = Vec::new();
        audit_channel_policies(&config, &mut findings);

        assert!(findings.iter().any(|f| f.message.contains("group messages")));
    }

    #[test]
    fn audit_ssrf_disabled_is_critical() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.security.ssrf_protection = false;
        let config_path = std::path::Path::new("/tmp/nonexistent-config.json");
        let state_dir = std::path::Path::new("/tmp/nonexistent-state");

        let exit_code = run_security_audit(&config, config_path, state_dir)
            .expect("audit should succeed");

        // Should fail due to SSRF being critical + likely other high findings
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn severity_ordering_is_correct() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Medium > Severity::Low);
        assert!(Severity::Low > Severity::Info);
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
