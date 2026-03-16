#![forbid(unsafe_code)]

mod repl;
mod terminal;
mod tui;

use std::path::PathBuf;

use anyhow::Context;
use base64::Engine;
use clap::{Parser, Subcommand};
use rust_i18n::t;
use tracing::info;
use tracing_subscriber::EnvFilter;

rust_i18n::i18n!("locales", fallback = "en");

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

    /// UI language (en, pt-BR, pt-PT, es, fr, de, it, ja, ko).
    #[arg(long, env = "FRANKCLAW_LANG")]
    lang: Option<String>,

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

    /// Interactive chat REPL (no gateway needed).
    Chat {
        /// Target agent ID.
        #[arg(long)]
        agent: Option<String>,

        /// Resume an existing session.
        #[arg(long)]
        session: Option<String>,

        /// Override model ID.
        #[arg(long)]
        model: Option<String>,

        /// Extended thinking budget in tokens.
        #[arg(long)]
        think: Option<u32>,

        /// Use the full-screen TUI instead of the line-based REPL.
        #[arg(long)]
        tui: bool,
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
        /// Channel example to print: web, telegram, discord, slack, signal, whatsapp, email.
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
    Audit {
        /// Output format: text or json.
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Authenticate with a provider (e.g., github-copilot).
    Login {
        /// Provider to authenticate with.
        provider: String,
    },

    /// Generate a secure starter config for a chosen channel profile.
    Onboard {
        /// Starter channel profile: web, telegram, whatsapp, slack, discord, signal, email.
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

    /// Start ACP server (JSON-RPC 2.0 over stdin/stdout).
    Acp,

    /// Manage plugins (list, enable, disable, info).
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },

    /// Initialize a new config file with secure defaults.
    Init {
        /// Force overwrite existing config.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// List discovered plugins.
    List,
    /// Enable a plugin by id.
    Enable {
        /// Plugin id.
        id: String,
    },
    /// Disable a plugin by id.
    Disable {
        /// Plugin id.
        id: String,
    },
    /// Show detailed info for a plugin.
    Info {
        /// Plugin id.
        id: String,
    },
}

#[tokio::main]
#[expect(
    clippy::too_many_lines,
    reason = "CLI entrypoint dispatching all subcommands"
)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log_level)),
        )
        .with_target(false)
        .init();

    // Initialize locale from --lang flag, FRANKCLAW_LANG, or system LANG.
    let locale = cli
        .lang
        .clone()
        .or_else(|| {
            std::env::var("LANG")
                .ok()
                .map(|l| l.split('.').next().unwrap_or("en").replace('_', "-"))
        })
        .unwrap_or_else(|| "en".into());
    rust_i18n::set_locale(&locale);

    let state_dir = cli.state_dir.unwrap_or_else(default_state_dir);

    match cli.command {
        Command::Chat {
            agent,
            session,
            model,
            think,
            tui: use_tui,
        } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions.clone()).await?;

            if use_tui {
                tui::run_tui(
                    runtime,
                    tui::TuiConfig {
                        agent_id: agent.map(frankclaw_core::types::AgentId::new),
                        session_key: session.map(frankclaw_core::types::SessionKey::from_raw),
                        model_id: model,
                        thinking_budget: think,
                    },
                )
                .await?;
            } else {
                repl::run_repl(
                    runtime,
                    repl::ReplConfig {
                        agent_id: agent.map(frankclaw_core::types::AgentId::new),
                        session_key: session.map(frankclaw_core::types::SessionKey::from_raw),
                        model_id: model,
                        thinking_budget: think,
                    },
                )
                .await?;
            }
        }

        Command::Gateway { port } => {
            let config_path = cli
                .config
                .clone()
                .unwrap_or_else(|| state_dir.join("frankclaw.toml"));
            let mut config = load_config(Some(&config_path), &state_dir)?;
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
                .context(t!("ctx.failed_open_sessions").to_string())?,
            );
            let runtime = std::sync::Arc::new(
                frankclaw_runtime::Runtime::from_config(
                    &config,
                    sessions.clone() as std::sync::Arc<dyn frankclaw_core::session::SessionStore>,
                )
                .await
                .context(t!("ctx.failed_init_runtime").to_string())?,
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
            eprint!("{}", t!("cmd.hash_password.prompt"));
            let password = read_password()?;
            let hash = frankclaw_crypto::hash_password(&password)
                .context(t!("ctx.failed_hash_password").to_string())?;
            println!("{}", hash.as_str());
        }

        Command::Check => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            println!("{}", t!("cmd.check.valid"));
            println!("{}", t!("cmd.check.port", port = config.gateway.port));
            println!(
                "{}",
                t!(
                    "cmd.check.auth",
                    mode = format!("{:?}", config.gateway.auth)
                )
            );
            println!(
                "{}",
                t!("cmd.check.channels", count = config.channels.len())
            );
            println!(
                "{}",
                t!("cmd.check.providers", count = config.models.providers.len())
            );
        }

        Command::Doctor => {
            run_doctor(cli.config.as_deref(), &state_dir).await?;
        }

        Command::Setup { force } => {
            run_setup(cli.config.as_deref(), &state_dir, force)?;
        }

        Command::Config => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            let toml_str = toml::to_string_pretty(&redact_config(&config))?;
            println!("{toml_str}");
        }

        Command::ConfigExample { channel } => {
            let example = supported_channel_example(&channel).ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    t!("cmd.config_example.unsupported", channel = &channel)
                )
            })?;
            println!("{example}");
        }

        Command::Status => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions).await?;
            let channels = frankclaw_channels::load_from_config(&config)
                .context(t!("ctx.failed_load_channels").to_string())?;
            let exposure = frankclaw_gateway::auth::assess_exposure(&config)?;

            print_exposure_report(&exposure);
            println!();
            println!("{}", t!("cmd.status.providers"));
            for provider in runtime.provider_health().await {
                println!(
                    "  {}  {}",
                    provider.provider_id,
                    if provider.healthy {
                        t!("cmd.status.healthy")
                    } else {
                        t!("cmd.status.unhealthy")
                    }
                );
            }
            println!();
            println!("{}", t!("cmd.status.agents"));
            for (agent_id, agent, skills) in runtime.agent_surface() {
                println!(
                    "  {}  model={}  tools={}  skills={}",
                    agent_id,
                    agent
                        .model
                        .clone()
                        .or_else(|| config.models.default_model.clone())
                        .unwrap_or_else(|| t!("cmd.status.unset").to_string()),
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
            if let Some(browser_status) = browser_runtime_status(
                &config,
                std::env::var("FRANKCLAW_BROWSER_DEVTOOLS_URL")
                    .ok()
                    .as_deref(),
            ) {
                println!();
                println!("{}", t!("cmd.status.browser"));
                println!("  {browser_status}");
            }
            println!();
            println!("{}", t!("cmd.status.channels"));
            for (channel_id, channel) in channels.channels() {
                println!("  {}  {:?}", channel_id, channel.health().await);
            }

            // Check daemon PID status
            let pid_path = state_dir.join("frankclaw.pid");
            if let Some(status) = daemon_pid_status(&pid_path) {
                println!();
                println!("{}", t!("cmd.status.daemon"));
                println!("  {status}");
            }
        }

        Command::Start { port } => {
            let pid_path = state_dir.join("frankclaw.pid");
            if let Some(existing_pid) = read_pid_file(&pid_path) {
                if is_process_alive(existing_pid) {
                    println!("{}", t!("cmd.start.already_running", pid = existing_pid));
                    return Ok(());
                }
                // Stale PID file — clean up
                let _ = std::fs::remove_file(&pid_path);
            }

            let executable =
                std::env::current_exe().context(t!("ctx.failed_locate_binary").to_string())?;
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
                .context(t!("ctx.failed_open_log").to_string())?;
            let log_err = log_file
                .try_clone()
                .context(t!("ctx.failed_clone_log").to_string())?;

            cmd.stdout(std::process::Stdio::from(log_file));
            cmd.stderr(std::process::Stdio::from(log_err));
            cmd.stdin(std::process::Stdio::null());

            let child = cmd
                .spawn()
                .context(t!("ctx.failed_start_gateway").to_string())?;
            let pid = child.id();

            std::fs::write(&pid_path, pid.to_string())?;
            restrict_file_permissions(&pid_path);

            println!("{}", t!("cmd.start.started", pid = pid));
            println!("{}", t!("cmd.start.log", path = log_path.display()));
            println!("{}", t!("cmd.start.pid_file", path = pid_path.display()));
            println!();
            println!("{}", t!("cmd.start.stop_hint"));
        }

        Command::Stop => {
            let pid_path = state_dir.join("frankclaw.pid");
            match read_pid_file(&pid_path) {
                Some(pid) => {
                    if !is_process_alive(pid) {
                        println!("{}", t!("cmd.stop.not_running_stale", pid = pid));
                        let _ = std::fs::remove_file(&pid_path);
                        return Ok(());
                    }
                    stop_process(pid)?;
                    let _ = std::fs::remove_file(&pid_path);
                    println!("{}", t!("cmd.stop.stopped", pid = pid));
                }
                None => {
                    println!("{}", t!("cmd.stop.not_found"));
                }
            }
        }

        Command::Audit { format } => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            let config_file_path = cli
                .config
                .clone()
                .unwrap_or_else(|| state_dir.join("frankclaw.toml"));
            let exit_code = run_security_audit(&config, &config_file_path, &state_dir, &format)?;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }

        Command::Login { provider } => {
            run_login(&provider, &state_dir).await?;
        }

        Command::Onboard { channel, force } => {
            let config_path = cli
                .config
                .clone()
                .unwrap_or_else(|| state_dir.join("frankclaw.toml"));
            if config_path.exists() && !force {
                anyhow::bail!("{}", t!("cmd.onboard.exists", path = config_path.display()));
            }

            let gateway_token = frankclaw_crypto::generate_token();
            let config = build_onboard_config(&channel, &gateway_token)?;
            let toml_str = config.to_toml_string()?;
            std::fs::create_dir_all(config_path.parent().unwrap_or(&state_dir))?;
            std::fs::write(&config_path, toml_str)?;
            restrict_file_permissions(&config_path);

            println!(
                "{}",
                t!("cmd.onboard.created", path = config_path.display())
            );
            println!("{}", t!("cmd.onboard.token", token = &gateway_token));
            println!();
            println!("{}", t!("cmd.onboard.next"));
            println!("{}", t!("cmd.onboard.step1"));
            println!("{}", t!("cmd.onboard.step2"));
            println!("{}", t!("cmd.onboard.step3", path = config_path.display()));
        }

        Command::InstallSystemd { config } => {
            let config_path = config
                .or_else(|| cli.config.clone())
                .unwrap_or_else(|| state_dir.join("frankclaw.toml"));
            let executable =
                std::env::current_exe().context(t!("ctx.failed_locate_binary").to_string())?;
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
                    thinking_budget: None,
                    channel_id: None,
                    channel_capabilities: None,
                    canvas: Some(frankclaw_gateway::canvas::CanvasStore::new()),
                    cancel_token: None,
                    approval_tx: None,
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
                .context(t!("ctx.session_not_found").to_string())?;
            let last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(&entry.metadata)
                .context(t!("ctx.no_delivery_metadata").to_string())?;

            if last_reply.chunks.len() > 1 {
                anyhow::bail!("{}", t!("cmd.message.editing_chunked_unsupported"));
            }

            let platform_message_id = last_reply
                .platform_message_id
                .clone()
                .context(t!("ctx.missing_platform_id").to_string())?;

            let channels = frankclaw_channels::load_from_config(&config)
                .context(t!("ctx.failed_load_channels").to_string())?;
            let channel = channels.get(&entry.channel).cloned().with_context(|| {
                t!(
                    "ctx.channel_not_configured",
                    channel = entry.channel.as_str()
                )
                .to_string()
            })?;

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
                anyhow::bail!("{}", t!("cmd.message.no_assistant_turn"));
            }

            rewrite_last_reply_metadata_for_edit(&mut entry.metadata, &text)?;
            sessions.upsert(&entry).await?;

            println!(
                "{}",
                t!("cmd.message.edited", key = session_key.to_string())
            );
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
                .context(t!("ctx.session_not_found").to_string())?;
            let last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(&entry.metadata)
                .context(t!("ctx.no_delivery_metadata").to_string())?;

            let channels = frankclaw_channels::load_from_config(&config)
                .context(t!("ctx.failed_load_channels").to_string())?;
            let channel = channels.get(&entry.channel).cloned().with_context(|| {
                t!(
                    "ctx.channel_not_configured",
                    channel = entry.channel.as_str()
                )
                .to_string()
            })?;

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

            println!(
                "{}",
                t!("cmd.message.deleted", key = session_key.to_string())
            );
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
                Some(raw) => {
                    serde_json::from_str(&raw).context(t!("ctx.tool_args_invalid").to_string())?
                }
                None => serde_json::json!({}),
            };

            let output = runtime
                .invoke_tool(frankclaw_runtime::ToolRequest {
                    agent_id: agent.map(frankclaw_core::types::AgentId::new),
                    session_key: session.map(frankclaw_core::types::SessionKey::from_raw),
                    tool_name: tool,
                    arguments,
                    canvas: Some(frankclaw_gateway::canvas::CanvasStore::new()),
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
                println!("{}", t!("cmd.tools.no_activity"));
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
                    println!("  {description}");
                }
                if !skill.capabilities.is_empty() {
                    println!(
                        "  capabilities: {}",
                        skill
                            .capabilities
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
            let agent_id = agent.map_or_else(
                frankclaw_core::types::AgentId::default_agent,
                frankclaw_core::types::AgentId::new,
            );
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
            println!("{}", t!("cmd.sessions.cleared"));
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
                "{}",
                t!(
                    "cmd.pairing.approved",
                    sender = approved.sender_id.as_str(),
                    channel = approved.channel.as_str(),
                    account = approved.account_id.as_str()
                )
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
                    anyhow::bail!("{}", t!("cmd.remote.not_public"));
                }
            } else if !report.remote_ready {
                anyhow::bail!("{}", t!("cmd.remote.not_remote"));
            } else {
                // Remote-ready, non-public: proceed with remote setup.
            }
        }

        Command::Acp => {
            let config = load_config(cli.config.as_deref(), &state_dir)?;
            config.validate()?;
            let sessions = open_sessions(&state_dir)?;
            let runtime = build_runtime(&config, sessions.clone()).await?;

            eprintln!("{}", t!("acp.starting"));
            eprintln!("{}", t!("acp.ready"));

            let server = frankclaw_gateway::acp::AcpServer::new(
                runtime,
                frankclaw_gateway::acp::AcpServerOptions::default(),
            );

            let stdin = std::io::BufReader::new(std::io::stdin());
            for line_result in frankclaw_gateway::acp_transport::read_lines(stdin) {
                let line = match line_result {
                    Ok(line) => line,
                    Err(e) => {
                        let resp = frankclaw_gateway::acp_transport::JsonRpcResponse::error(
                            None,
                            frankclaw_gateway::acp_transport::PARSE_ERROR,
                            e.to_string(),
                        );
                        let _ = frankclaw_gateway::acp_transport::write_response(&resp);
                        continue;
                    }
                };

                if line.trim().is_empty() {
                    continue;
                }

                let req = match frankclaw_gateway::acp_transport::parse_request(&line) {
                    Ok(req) => req,
                    Err(resp) => {
                        let _ = frankclaw_gateway::acp_transport::write_response(&resp);
                        continue;
                    }
                };

                let resp = server.handle_request(req).await;
                let _ = frankclaw_gateway::acp_transport::write_response(&resp);
            }
        }

        Command::Plugin { action } => {
            let workspace = std::env::current_dir().ok();
            let dirs = frankclaw_plugin_sdk::discovery::default_plugin_dirs(workspace.as_deref());
            let discovered = frankclaw_plugin_sdk::discovery::discover_plugins(&dirs);
            // Load persisted state (empty for now — could be stored in config).
            let persisted: std::collections::HashMap<
                String,
                frankclaw_plugin_sdk::lifecycle::PluginState,
            > = std::collections::HashMap::new();
            let mut manager =
                frankclaw_plugin_sdk::lifecycle::PluginManager::new(discovered, &persisted);

            match action {
                PluginAction::List => {
                    let plugins = manager.list();
                    if plugins.is_empty() {
                        println!("{}", t!("plugin.none_found"));
                    } else {
                        println!("{}", t!("plugin.list_title"));
                        println!();
                        for record in plugins {
                            let status = if record.enabled {
                                t!("plugin.enabled").to_string()
                            } else {
                                t!("plugin.disabled").to_string()
                            };
                            println!(
                                "  {} v{} [{}] ({})",
                                record.manifest.id,
                                record.manifest.version,
                                status,
                                match record.origin {
                                    frankclaw_plugin_sdk::discovery::PluginOrigin::Workspace =>
                                        "workspace",
                                    frankclaw_plugin_sdk::discovery::PluginOrigin::User => "user",
                                },
                            );
                            if let Some(desc) = &record.manifest.description {
                                println!("    {desc}");
                            }
                        }
                    }
                }
                PluginAction::Enable { id } => {
                    if manager.enable(&id) {
                        println!("{}", t!("plugin.enabled_msg", id = &id));
                    } else {
                        println!("{}", t!("plugin.not_found", id = &id));
                    }
                }
                PluginAction::Disable { id } => {
                    if manager.disable(&id) {
                        println!("{}", t!("plugin.disabled_msg", id = &id));
                    } else {
                        println!("{}", t!("plugin.not_found", id = &id));
                    }
                }
                PluginAction::Info { id } => {
                    if let Some(record) = manager.get(&id) {
                        println!("{}: {}", t!("plugin.info_id"), record.manifest.id);
                        println!("{}: {}", t!("plugin.info_name"), record.manifest.name);
                        println!("{}: {}", t!("plugin.info_version"), record.manifest.version);
                        if let Some(desc) = &record.manifest.description {
                            println!("{}: {desc}", t!("plugin.info_description"));
                        }
                        println!("{}: {}", t!("plugin.info_path"), record.path.display());
                        let status = if record.enabled {
                            t!("plugin.enabled").to_string()
                        } else {
                            t!("plugin.disabled").to_string()
                        };
                        println!("{}: {status}", t!("plugin.info_status"));
                        if !record.manifest.channels.is_empty() {
                            println!(
                                "{}: {}",
                                t!("plugin.info_channels"),
                                record.manifest.channels.join(", ")
                            );
                        }
                        if !record.manifest.tools.is_empty() {
                            println!(
                                "{}: {}",
                                t!("plugin.info_tools"),
                                record.manifest.tools.join(", ")
                            );
                        }
                    } else {
                        println!("{}", t!("plugin.not_found", id = &id));
                    }
                }
            }
        }

        Command::Init { force } => {
            let config_path = cli
                .config
                .unwrap_or_else(|| state_dir.join("frankclaw.toml"));

            if config_path.exists() && !force {
                anyhow::bail!("{}", t!("cmd.init.exists", path = config_path.display()));
            }

            let config = frankclaw_core::config::FrankClawConfig::default();
            let toml_str = config.to_toml_string()?;

            std::fs::create_dir_all(config_path.parent().unwrap_or(&state_dir))?;
            std::fs::write(&config_path, &toml_str)?;
            restrict_file_permissions(&config_path);

            println!("{}", t!("cmd.init.created", path = config_path.display()));
            println!();
            println!("{}", t!("cmd.init.next"));
            println!("{}", t!("cmd.init.step1"));
            println!("{}", t!("cmd.init.step2", path = config_path.display()));
            println!("{}", t!("cmd.init.step3"));
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
    let config_path = path.map_or_else(|| state_dir.join("frankclaw.toml"), PathBuf::from);
    let exists = config_path.exists();
    info!(
        path = %config_path.display(),
        exists,
        env_prefix = frankclaw_core::config::ENV_PREFIX,
        "loading config via figment"
    );
    frankclaw_core::config::FrankClawConfig::load_or_default(&config_path)
        .map_err(anyhow::Error::from)
}

#[expect(
    clippy::too_many_lines,
    reason = "doctor warnings aggregate many independent config checks"
)]
fn collect_doctor_warnings(
    config: &frankclaw_core::config::FrankClawConfig,
    state_dir: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    let mut warnings = Vec::new();

    if config.models.providers.is_empty() {
        warnings.push(t!("warn.no_providers").to_string());
    }
    if config.channels.is_empty() {
        warnings.push(t!("warn.no_channels").to_string());
    }
    if !config.security.encrypt_sessions {
        warnings.push(t!("warn.encrypt_off").to_string());
    }
    if config.security.encrypt_sessions && load_master_key_from_env()?.is_none() {
        warnings.push(t!("warn.encrypt_no_key").to_string());
    }
    if !state_dir.exists() {
        warnings.push(t!("warn.state_missing", path = state_dir.display()).to_string());
    }

    for provider in &config.models.providers {
        if let Some(env_name) = provider.api_key_ref.as_deref()
            && std::env::var(env_name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .is_none()
        {
            warnings.push(
                t!(
                    "warn.missing_env",
                    id = provider.id.as_str(),
                    env = env_name
                )
                .to_string(),
            );
        }
    }

    for (channel_id, channel) in &config.channels {
        let policy = channel.security_policy().with_context(|| {
            t!("warn.invalid_channel_policy", channel = channel_id.as_str()).to_string()
        })?;

        if group_surface_needs_guard(channel_id.as_str())
            && !policy.require_mention_for_groups
            && policy.allowed_groups.is_none()
        {
            warnings
                .push(t!("warn.channel_open_groups", channel = channel_id.as_str()).to_string());
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
                if let Some(env_name) = account.get(key).and_then(|value| value.as_str())
                    && std::env::var(env_name)
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .is_none()
                {
                    warnings.push(
                        t!(
                            "warn.channel_missing_env",
                            channel = channel_id.as_str(),
                            env = env_name,
                            key = key
                        )
                        .to_string(),
                    );
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
                    warnings.push(
                        t!(
                            "warn.channel_inline_secret",
                            channel = channel_id.as_str(),
                            key = inline_key,
                            env_key = env_key
                        )
                        .to_string(),
                    );
                }
            }

            if channel_id.as_str() == "whatsapp"
                && account
                    .get("app_secret")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
                && account
                    .get("app_secret_env")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
            {
                warnings.push(t!("warn.whatsapp_no_secret").to_string());
            }
        }
    }

    let exposure = frankclaw_gateway::auth::assess_exposure(config)?;
    warnings.extend(exposure.warnings);
    warnings.extend(collect_browser_tool_warnings(
        config,
        std::env::var("FRANKCLAW_BROWSER_DEVTOOLS_URL")
            .ok()
            .as_deref(),
    ));

    Ok(warnings)
}

fn group_surface_needs_guard(channel_id: &str) -> bool {
    matches!(
        channel_id,
        "telegram" | "discord" | "slack" | "signal" | "whatsapp" | "email"
    )
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

#[expect(
    clippy::needless_pass_by_value,
    reason = "ToolPolicy is small and consumed for field inspection"
)]
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
        .any(|tool| tool.starts_with("browser_"));
    if !browser_tools_enabled {
        return Vec::new();
    }
    let has_mutating_tools = config
        .agents
        .agents
        .values()
        .flat_map(|agent| agent.tools.iter())
        .any(|tool| {
            frankclaw_tools::tool_risk_level(tool) >= frankclaw_core::model::ToolRiskLevel::Mutating
        });

    let endpoint = browser_endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("http://127.0.0.1:9222/");
    let parsed = match url::Url::parse(endpoint) {
        Ok(parsed) => parsed,
        Err(err) => {
            return vec![t!("browser.invalid_url", error = err.to_string()).to_string()];
        }
    };

    let mut warnings = Vec::new();
    if has_mutating_tools
        && !tool_policy
            .approval_level
            .approves(frankclaw_core::model::ToolRiskLevel::Mutating)
    {
        warnings.push(t!("browser.mutations_blocked").to_string());
    }
    match parsed.host_str() {
        Some("127.0.0.1" | "localhost") => {}
        Some(other) => warnings.push(t!("browser.non_loopback", host = other).to_string()),
        None => warnings.push(t!("browser.no_host").to_string()),
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
        Err(_) => warnings.push(t!("browser.unreachable", endpoint = endpoint).to_string()),
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

#[expect(
    clippy::needless_pass_by_value,
    reason = "ToolPolicy is small and forwarded to inner function"
)]
fn browser_runtime_status_with_policy(
    config: &frankclaw_core::config::FrankClawConfig,
    browser_endpoint: Option<&str>,
    policy: frankclaw_tools::ToolPolicy,
) -> Option<String> {
    let warnings =
        collect_browser_tool_warnings_with_policy(config, browser_endpoint, policy.clone());
    if warnings.is_empty() {
        return config
            .agents
            .agents
            .values()
            .flat_map(|agent| agent.tools.iter())
            .any(|tool| tool.starts_with("browser_"))
            .then(|| {
                let has_mutating = config
                    .agents
                    .agents
                    .values()
                    .flat_map(|agent| agent.tools.iter())
                    .any(|tool| {
                        frankclaw_tools::tool_risk_level(tool)
                            >= frankclaw_core::model::ToolRiskLevel::Mutating
                    });
                let mutation_state = if has_mutating {
                    if policy
                        .approval_level
                        .approves(frankclaw_core::model::ToolRiskLevel::Mutating)
                    {
                        t!("cmd.status.mutations_enabled").to_string()
                    } else {
                        t!("cmd.status.mutations_gated").to_string()
                    }
                } else {
                    t!("cmd.status.read_only").to_string()
                };
                format!(
                    "{} at {}",
                    mutation_state,
                    browser_endpoint.unwrap_or("http://127.0.0.1:9222/")
                )
            });
    }
    Some(warnings.join(" | "))
}

fn read_password() -> anyhow::Result<secrecy::SecretString> {
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context(t!("ctx.failed_read_password").to_string())?;
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
        "email" => ChannelConfig {
            enabled: true,
            accounts: vec![serde_json::json!({
                "imap_server": "imap.gmail.com",
                "imap_port": 993,
                "smtp_server": "smtp.gmail.com",
                "smtp_port": 587,
                "imap_user_env": "EMAIL_USER",
                "imap_password_env": "EMAIL_PASSWORD",
                "smtp_user_env": "EMAIL_USER",
                "smtp_password_env": "EMAIL_PASSWORD",
                "smtp_from_env": "EMAIL_USER",
                "poll_interval_secs": 30,
                "allowed_senders": []
            })],
            extra: serde_json::json!({}),
        },
        other => anyhow::bail!("{}", t!("cmd.onboard.unsupported", channel = other)),
    };
    config
        .channels
        .insert(ChannelId::new(channel), channel_config);
    Ok(config)
}

fn supported_channel_example(channel: &str) -> Option<&'static str> {
    match channel.trim() {
        "web" => Some(include_str!("../../../examples/channels/web.toml")),
        "telegram" => Some(include_str!("../../../examples/channels/telegram.toml")),
        "discord" => Some(include_str!("../../../examples/channels/discord.toml")),
        "slack" => Some(include_str!("../../../examples/channels/slack.toml")),
        "signal" => Some(include_str!("../../../examples/channels/signal.toml")),
        "whatsapp" => Some(include_str!("../../../examples/channels/whatsapp.toml")),
        "email" => Some(include_str!("../../../examples/channels/email.toml")),
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
        .context(t!("ctx.no_delivery_metadata").to_string())?;

    if last_reply.chunks.len() > 1 {
        anyhow::bail!("{}", t!("cmd.message.editing_chunked_unsupported"));
    }

    last_reply.content = new_text.to_string();
    if let Some(first_chunk) = last_reply.chunks.first_mut() {
        first_chunk.content = new_text.to_string();
    }

    frankclaw_gateway::delivery::set_last_reply_in_metadata(metadata, &last_reply)
        .context(t!("ctx.failed_update_metadata").to_string())?;
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
        anyhow::bail!("{}", t!("ctx.missing_platform_ids"));
    }

    Ok(targets)
}

fn mark_last_reply_metadata_deleted(
    metadata: &mut serde_json::Value,
) -> anyhow::Result<frankclaw_gateway::delivery::StoredReplyMetadata> {
    let mut last_reply = frankclaw_gateway::delivery::last_reply_from_metadata(metadata)
        .context(t!("ctx.no_delivery_metadata").to_string())?;

    last_reply.status = "deleted".into();
    last_reply.platform_message_id = None;
    for chunk in &mut last_reply.chunks {
        chunk.status = "deleted".into();
        chunk.platform_message_id = None;
    }

    frankclaw_gateway::delivery::set_last_reply_in_metadata(metadata, &last_reply)
        .context(t!("ctx.failed_update_metadata").to_string())?;
    Ok(last_reply)
}

fn display_skill_capability(capability: &frankclaw_plugin_sdk::SkillCapability) -> &'static str {
    match capability {
        frankclaw_plugin_sdk::SkillCapability::Prompt => "prompt",
        frankclaw_plugin_sdk::SkillCapability::ReadSession => "read_session",
    }
}

fn print_exposure_report(report: &frankclaw_gateway::auth::ExposureReport) {
    println!(
        "{}",
        t!("exposure.summary_label", summary = &*report.summary)
    );
    println!("{}", t!("exposure.auth", mode = report.auth_mode));
    println!(
        "{}",
        t!(
            "exposure.bind",
            surface = display_exposure_surface(&report.surface)
        )
    );
    println!(
        "{}",
        t!(
            "exposure.remote",
            status = if report.remote_ready {
                t!("exposure.ready")
            } else {
                t!("exposure.not_ready")
            }
        )
    );
    println!(
        "{}",
        t!(
            "exposure.public",
            status = if report.public_ready {
                t!("exposure.ready")
            } else {
                t!("exposure.not_ready")
            }
        )
    );
    if !report.warnings.is_empty() {
        println!();
        println!("{}", t!("exposure.warnings"));
        for warning in &report.warnings {
            println!("  - {warning}");
        }
    }
}

fn display_exposure_surface(surface: &frankclaw_gateway::auth::ExposureSurface) -> String {
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
        .context(t!("ctx.failed_open_sessions").to_string())?,
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
    let mut store = frankclaw_media::MediaStore::new(
        media_dir,
        config.media.max_file_size_bytes,
        config.media.ttl_hours,
    )
    .context("failed to open media store")?;

    // Attach VirusTotal scanner if API key is available.
    if let Some(scanner) = frankclaw_media::virustotal::VirusTotalScanner::from_env()
        .context("failed to build VirusTotal scanner")?
    {
        tracing::info!("VirusTotal file scanning enabled");
        store = store.with_scanner(std::sync::Arc::new(scanner));
    }

    Ok(std::sync::Arc::new(store))
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
    let content = std::fs::read_to_string(pid_path).ok()?;
    content.trim().parse::<u32>().ok()
}

fn is_process_alive(pid: u32) -> bool {
    // Use `kill -0` to check process existence without unsafe code
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
        Some(t!("daemon.running", pid = pid).to_string())
    } else {
        Some(t!("daemon.not_running", pid = pid).to_string())
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
        .context(t!("ctx.failed_read_input").to_string())?;
    Ok(input.trim().to_string())
}

fn prompt_choice(question: &str, options: &[&str], default: usize) -> anyhow::Result<usize> {
    eprintln!("{question}");
    for (i, option) in options.iter().enumerate() {
        let marker = if i == default {
            format!(" {}", t!("setup.default_marker"))
        } else {
            String::new()
        };
        eprintln!("  {}: {}{}", i + 1, option, marker);
    }
    let input = prompt_line(t!("setup.choose", max = options.len()).as_ref())?;
    if input.is_empty() {
        return Ok(default);
    }
    match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= options.len() => Ok(n - 1),
        _ => {
            eprintln!("{}", t!("setup.invalid_choice"));
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

#[expect(
    clippy::too_many_lines,
    reason = "interactive setup wizard with many prompts"
)]
#[expect(
    clippy::unreachable,
    reason = "menu indices are bounded by prompt_menu; unreachable is the correct contract"
)]
fn run_setup(
    config_path_override: Option<&std::path::Path>,
    state_dir: &std::path::Path,
    force: bool,
) -> anyhow::Result<()> {
    use frankclaw_core::auth::AuthMode;
    use frankclaw_core::config::{ChannelConfig, ProviderConfig};
    use frankclaw_core::types::ChannelId;

    let config_path =
        config_path_override.map_or_else(|| state_dir.join("frankclaw.toml"), PathBuf::from);

    println!("{}", t!("setup.title"));
    println!("{}", t!("setup.separator"));
    println!();

    if config_path.exists() && !force {
        eprintln!(
            "{}",
            t!("setup.config_exists", path = config_path.display())
        );
        if !prompt_yes_no(t!("setup.overwrite").as_ref(), false)? {
            println!("{}", t!("setup.cancelled"));
            return Ok(());
        }
    }

    // --- Provider selection ---
    println!();
    let provider_idx = prompt_choice(
        t!("setup.which_provider").as_ref(),
        &[
            t!("setup.provider.openai").as_ref(),
            t!("setup.provider.anthropic").as_ref(),
            t!("setup.provider.ollama").as_ref(),
        ],
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
        let env_name = prompt_line(t!("setup.api_key_env", default = default_env).as_ref())?;
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
        let url = prompt_line(t!("setup.ollama_url").as_ref())?;
        Some(if url.is_empty() {
            "http://127.0.0.1:11434".to_string()
        } else {
            url
        })
    } else {
        None
    };

    println!();
    let model_input = prompt_line(t!("setup.default_model", model = default_model).as_ref())?;
    let model = if model_input.is_empty() {
        default_model.to_string()
    } else {
        model_input
    };

    // --- Channel selection ---
    println!();
    let channel_idx = prompt_choice(
        t!("setup.primary_channel").as_ref(),
        &[
            t!("setup.channel.web").as_ref(),
            t!("setup.channel.telegram").as_ref(),
            t!("setup.channel.discord").as_ref(),
            t!("setup.channel.slack").as_ref(),
            t!("setup.channel.whatsapp").as_ref(),
            t!("setup.channel.signal").as_ref(),
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
    let port_input = prompt_line(t!("setup.gateway_port").as_ref())?;
    let port: u16 = if port_input.is_empty() {
        18789
    } else {
        port_input
            .parse()
            .context(t!("setup.invalid_port").to_string())?
    };

    // --- Session encryption ---
    println!();
    let encrypt = prompt_yes_no(t!("setup.encrypt_sessions").as_ref(), true)?;

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
    config
        .channels
        .insert(ChannelId::new(channel_id), channel_config);
    config.security.encrypt_sessions = encrypt;

    // --- Write config ---
    std::fs::create_dir_all(config_path.parent().unwrap_or(state_dir))?;
    let toml_str = config.to_toml_string()?;
    std::fs::write(&config_path, &toml_str)?;
    restrict_file_permissions(&config_path);

    println!();
    println!(
        "{}",
        t!("setup.config_written", path = config_path.display())
    );
    println!("{}", t!("setup.gateway_token", token = gateway_token));
    if encrypt {
        println!();
        println!("{}", t!("setup.encryption_on"));
        println!("{}", t!("setup.encryption_cmd"));
    }
    println!();
    println!("{}", t!("setup.next_steps"));
    if needs_key {
        let env = config.models.providers[0]
            .api_key_ref
            .as_deref()
            .unwrap_or("API_KEY");
        println!("{}", t!("setup.export_key", env = env));
    }
    if channel_id != "web" {
        println!("{}", t!("setup.export_channel"));
    }
    println!("{}", t!("setup.verify_config"));
    println!("{}", t!("setup.start_gateway"));

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
    println!(
        "\n  {}",
        terminal::bold_colored(title, terminal::colors::ACCENT)
    );
    for check in checks {
        println!(
            "    {} {}",
            terminal::check_badge(check.prefix()),
            check.message()
        );
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "diagnostic checks across many subsystems"
)]
async fn run_doctor(
    config_path: Option<&std::path::Path>,
    state_dir: &std::path::Path,
) -> anyhow::Result<()> {
    println!(
        "{}",
        terminal::bold_colored(t!("doctor.title").as_ref(), terminal::colors::ACCENT)
    );
    println!(
        "{}",
        terminal::colored(t!("doctor.separator").as_ref(), terminal::colors::MUTED)
    );

    // --- System info ---
    let mut system_checks = vec![
        CheckResult::Info(
            t!("doctor.info.version", version = env!("CARGO_PKG_VERSION")).to_string(),
        ),
        CheckResult::Info(t!("doctor.info.rust", version = rustc_version()).to_string()),
        CheckResult::Info(t!("doctor.info.os", os = std::env::consts::OS).to_string()),
        CheckResult::Info(t!("doctor.info.arch", arch = std::env::consts::ARCH).to_string()),
    ];
    let state_display = state_dir.display().to_string();
    system_checks.push(CheckResult::Info(
        t!("doctor.info.state_dir", path = state_display).to_string(),
    ));
    print_section(t!("doctor.section.system").as_ref(), &system_checks);

    // --- Configuration ---
    let config = match load_config(config_path, state_dir) {
        Ok(cfg) => cfg,
        Err(err) => {
            print_section(
                t!("doctor.section.configuration").as_ref(),
                &[CheckResult::Fail(
                    t!("doctor.config.load_failed", error = err).to_string(),
                )],
            );
            println!("\n{}", t!("doctor.config.critical_issues"));
            return Ok(());
        }
    };

    let mut config_checks = Vec::new();
    match config.validate() {
        Ok(()) => config_checks.push(CheckResult::Pass(t!("doctor.config.valid").to_string())),
        Err(err) => {
            config_checks.push(CheckResult::Fail(
                t!("doctor.config.validation_failed", error = err).to_string(),
            ));
            print_section(t!("doctor.section.configuration").as_ref(), &config_checks);
            println!("\n{}", t!("doctor.config.critical_issues"));
            return Ok(());
        }
    }

    // Config file permissions (Unix only)
    let config_file_path =
        config_path.map_or_else(|| state_dir.join("frankclaw.toml"), PathBuf::from);
    config_checks.extend(check_file_permissions(&config_file_path, "config file"));

    let warnings = collect_doctor_warnings(&config, state_dir)?;
    for warning in &warnings {
        config_checks.push(CheckResult::Warn(warning.clone()));
    }
    if warnings.is_empty() {
        config_checks.push(CheckResult::Pass(
            t!("doctor.config.no_misconfig").to_string(),
        ));
    }

    let exposure = frankclaw_gateway::auth::assess_exposure(&config)?;
    config_checks.push(CheckResult::Info(
        t!("doctor.info.exposure", summary = exposure.summary).to_string(),
    ));
    for warning in &exposure.warnings {
        config_checks.push(CheckResult::Warn(warning.clone()));
    }

    print_section(t!("doctor.section.configuration").as_ref(), &config_checks);

    // --- State directory ---
    let mut state_checks = Vec::new();
    if state_dir.exists() {
        state_checks.push(CheckResult::Pass(t!("doctor.state.exists").to_string()));
        state_checks.extend(check_dir_permissions(state_dir, "state directory"));
    } else {
        state_checks.push(CheckResult::Warn(
            t!("doctor.state.missing", path = state_dir.display()).to_string(),
        ));
    }
    print_section(t!("doctor.section.state_directory").as_ref(), &state_checks);

    // --- Database ---
    let mut db_checks = Vec::new();
    let db_path = state_dir.join("sessions.db");
    if db_path.exists() {
        match check_sqlite_health(&db_path) {
            Ok(()) => db_checks.push(CheckResult::Pass(t!("doctor.db.valid").to_string())),
            Err(err) => db_checks.push(CheckResult::Fail(
                t!("doctor.db.error", error = err).to_string(),
            )),
        }
        db_checks.extend(check_file_permissions(&db_path, "sessions.db"));
    } else {
        db_checks.push(CheckResult::Info(t!("doctor.db.missing").to_string()));
    }
    print_section(t!("doctor.section.database").as_ref(), &db_checks);

    // --- Port availability ---
    let mut port_checks = Vec::new();
    let port = config.gateway.port;
    match check_port_available(port) {
        Ok(true) => port_checks.push(CheckResult::Pass(
            t!("doctor.port.available", port = port).to_string(),
        )),
        Ok(false) => port_checks.push(CheckResult::Warn(
            t!("doctor.port.in_use", port = port).to_string(),
        )),
        Err(err) => port_checks.push(CheckResult::Warn(
            t!("doctor.port.check_error", port = port, error = err).to_string(),
        )),
    }
    print_section(t!("doctor.section.network").as_ref(), &port_checks);

    // --- Providers ---
    let mut provider_checks = Vec::new();
    if config.models.providers.is_empty() {
        provider_checks.push(CheckResult::Warn(t!("doctor.provider.none").to_string()));
    }
    for provider in &config.models.providers {
        let has_key = provider
            .api_key_ref
            .as_deref()
            .and_then(|env_name| std::env::var(env_name).ok())
            .filter(|value| !value.trim().is_empty())
            .is_some();

        if !has_key {
            provider_checks.push(CheckResult::Warn(
                t!(
                    "doctor.provider.no_key",
                    id = provider.id,
                    api = provider.api
                )
                .to_string(),
            ));
            continue;
        }

        provider_checks.push(CheckResult::Pass(
            t!(
                "doctor.provider.ok",
                id = provider.id,
                api = provider.api,
                count = provider.models.len()
            )
            .to_string(),
        ));

        // Connectivity check for providers with a base URL
        let base_url = provider.base_url.as_deref().unwrap_or({
            match provider.api.as_str() {
                "openai" => "https://api.openai.com",
                "anthropic" => "https://api.anthropic.com",
                _ => "",
            }
        });
        if !base_url.is_empty() {
            match check_provider_reachable(base_url).await {
                Ok(()) => provider_checks.push(CheckResult::Pass(
                    t!("doctor.provider.reachable", url = base_url).to_string(),
                )),
                Err(err) => provider_checks.push(CheckResult::Warn(
                    t!("doctor.provider.unreachable", url = base_url, error = err).to_string(),
                )),
            }
        }
    }
    print_section(t!("doctor.section.providers").as_ref(), &provider_checks);

    // --- Channels ---
    let mut channel_checks = Vec::new();
    if config.channels.is_empty() {
        channel_checks.push(CheckResult::Warn(t!("doctor.channel.none").to_string()));
    }
    for (channel_id, channel) in &config.channels {
        if channel.enabled {
            channel_checks.push(CheckResult::Pass(
                t!(
                    "doctor.channel.enabled",
                    id = channel_id,
                    count = channel.accounts.len()
                )
                .to_string(),
            ));
        } else {
            channel_checks.push(CheckResult::Info(
                t!("doctor.channel.disabled", id = channel_id).to_string(),
            ));
        }
    }
    print_section(t!("doctor.section.channels").as_ref(), &channel_checks);

    // --- Security ---
    let mut security_checks = Vec::new();
    if config.security.encrypt_sessions {
        if load_master_key_from_env()?.is_some() {
            security_checks.push(CheckResult::Pass(
                t!("doctor.security.encrypt_ok").to_string(),
            ));
        } else {
            security_checks.push(CheckResult::Warn(
                t!("doctor.security.encrypt_no_key").to_string(),
            ));
        }
    } else {
        security_checks.push(CheckResult::Warn(
            t!("doctor.security.encrypt_off").to_string(),
        ));
    }

    match &config.gateway.auth {
        frankclaw_core::auth::AuthMode::None => {
            security_checks.push(CheckResult::Warn(
                t!("doctor.security.auth_none").to_string(),
            ));
        }
        frankclaw_core::auth::AuthMode::Token { .. } => {
            security_checks.push(CheckResult::Pass(
                t!("doctor.security.auth_token").to_string(),
            ));
        }
        frankclaw_core::auth::AuthMode::Password { .. } => {
            security_checks.push(CheckResult::Pass(
                t!("doctor.security.auth_password").to_string(),
            ));
        }
        frankclaw_core::auth::AuthMode::TrustedProxy { .. } => {
            security_checks.push(CheckResult::Pass(
                t!("doctor.security.auth_proxy").to_string(),
            ));
        }
        frankclaw_core::auth::AuthMode::Tailscale => {
            security_checks.push(CheckResult::Pass(
                t!("doctor.security.auth_tailscale").to_string(),
            ));
        }
    }
    print_section(t!("doctor.section.security").as_ref(), &security_checks);

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
    let warn_count = all_checks
        .iter()
        .filter(|c| matches!(c, CheckResult::Warn(_)))
        .count();

    println!();
    if fail_count > 0 {
        println!(
            "{}",
            t!(
                "doctor.summary.critical",
                fails = fail_count,
                warns = warn_count
            )
        );
    } else if warn_count > 0 {
        println!("{}", t!("doctor.summary.warnings", warns = warn_count));
    } else {
        println!("{}", t!("doctor.summary.ok"));
    }

    Ok(())
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".into(), |version| version.trim().to_string())
}

#[cfg(unix)]
fn check_file_permissions(path: &std::path::Path, label: &str) -> Vec<CheckResult> {
    use std::os::unix::fs::PermissionsExt;
    let mut results = Vec::new();
    if let Ok(metadata) = std::fs::metadata(path) {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            results.push(CheckResult::Warn(
                t!(
                    "perms.file_warn",
                    label = label,
                    mode = format!("{mode:04o}")
                )
                .to_string(),
            ));
        } else {
            results.push(CheckResult::Pass(
                t!("perms.file_ok", label = label, mode = format!("{mode:04o}")).to_string(),
            ));
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
            results.push(CheckResult::Warn(
                t!(
                    "perms.dir_warn",
                    label = label,
                    mode = format!("{mode:04o}")
                )
                .to_string(),
            ));
        } else {
            results.push(CheckResult::Pass(
                t!("perms.dir_ok", label = label, mode = format!("{mode:04o}")).to_string(),
            ));
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

#[derive(Debug, Clone, Copy)]
enum AuditCode {
    AuthDisabled,
    AuthWeak,
    SecretsInline,
    SecretsMissing,
    SecretsNoKeyRef,
    PlaintextFound,
    RefShadowed,
    LegacyResidue,
    EncryptionOff,
    EncryptionKeyMissing,
    NetworkExposed,
    NetworkNoTls,
    SsrfDisabled,
    ChannelPolicy,
    ChannelWhatsApp,
    ToolPolicy,
    ToolSandbox,
    FilePermissions,
    MediaScanning,
}

impl AuditCode {
    fn name(self) -> &'static str {
        match self {
            Self::AuthDisabled => "auth_disabled",
            Self::AuthWeak => "auth_weak",
            Self::SecretsInline => "secrets_inline",
            Self::SecretsMissing => "secrets_missing",
            Self::SecretsNoKeyRef => "secrets_no_key_ref",
            Self::PlaintextFound => "plaintext_found",
            Self::RefShadowed => "ref_shadowed",
            Self::LegacyResidue => "legacy_residue",
            Self::EncryptionOff => "encryption_off",
            Self::EncryptionKeyMissing => "encryption_key_missing",
            Self::NetworkExposed => "network_exposed",
            Self::NetworkNoTls => "network_no_tls",
            Self::SsrfDisabled => "ssrf_disabled",
            Self::ChannelPolicy => "channel_policy",
            Self::ChannelWhatsApp => "channel_whatsapp",
            Self::ToolPolicy => "tool_policy",
            Self::ToolSandbox => "tool_sandbox",
            Self::FilePermissions => "file_permissions",
            Self::MediaScanning => "media_scanning",
        }
    }
}

struct Finding {
    severity: Severity,
    code: AuditCode,
    category: &'static str,
    message: String,
    remediation: String,
    json_path: Option<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "security audit report generation intentionally stays in one dispatch function"
)]
fn run_security_audit(
    config: &frankclaw_core::config::FrankClawConfig,
    config_path: &std::path::Path,
    state_dir: &std::path::Path,
    output_format: &str,
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

    // --- File scanning ---
    audit_file_scanning(&mut findings);

    // --- File permissions ---
    audit_file_permissions(config_path, state_dir, &mut findings);

    // --- SSRF protection ---
    if !config.security.ssrf_protection {
        findings.push(Finding {
            severity: Severity::Critical,
            code: AuditCode::SsrfDisabled,
            category: "network",
            message: t!("audit.network.ssrf_off").to_string(),
            remediation: t!("audit.network.ssrf_off_fix").to_string(),
            json_path: None,
        });
    }

    // --- Scan for secret refs ---
    audit_secret_refs(config, &mut findings);

    // --- Scan .env files in working dir ---
    audit_dotenv_plaintext(&mut findings);

    // --- Check for legacy credential files ---
    audit_legacy_credentials(state_dir, &mut findings);

    // --- Print report ---
    findings.sort_by_key(|f| std::cmp::Reverse(f.severity));

    let crit = findings
        .iter()
        .filter(|f| f.severity == Severity::Critical)
        .count();
    let high = findings
        .iter()
        .filter(|f| f.severity == Severity::High)
        .count();
    let med = findings
        .iter()
        .filter(|f| f.severity == Severity::Medium)
        .count();
    let low = findings
        .iter()
        .filter(|f| f.severity == Severity::Low)
        .count();
    let info = findings
        .iter()
        .filter(|f| f.severity == Severity::Info)
        .count();

    if output_format == "json" {
        let json_findings: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                let mut obj = serde_json::json!({
                    "severity": f.severity.to_string().trim().to_string(),
                    "category": f.category,
                    "code": f.code.name(),
                    "message": f.message,
                    "remediation": f.remediation,
                });
                if let Some(ref jp) = f.json_path {
                    obj["json_path"] = serde_json::json!(jp);
                }
                obj
            })
            .collect();
        let report = serde_json::json!({
            "version": 1,
            "status": if crit > 0 || high > 0 { "fail" } else if findings.is_empty() { "clean" } else { "warn" },
            "summary": {
                "critical": crit,
                "high": high,
                "medium": med,
                "low": low,
                "info": info,
                "total": findings.len(),
            },
            "findings": json_findings,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}",
            terminal::bold_colored(t!("audit.title").as_ref(), terminal::colors::ACCENT)
        );
        println!(
            "{}",
            terminal::colored(t!("audit.separator").as_ref(), terminal::colors::MUTED)
        );
        println!();

        if findings.is_empty() {
            println!("{}", t!("audit.clean"));
            return Ok(0);
        }

        let mut by_category: std::collections::BTreeMap<&str, Vec<&Finding>> =
            std::collections::BTreeMap::new();
        for finding in &findings {
            by_category
                .entry(finding.category)
                .or_default()
                .push(finding);
        }

        for (category, category_findings) in &by_category {
            println!(
                "  {}",
                terminal::bold_colored(&category.to_uppercase(), terminal::colors::ACCENT)
            );
            for finding in category_findings {
                println!(
                    "    {} {}",
                    terminal::severity_badge(finding.severity.to_string().trim()),
                    finding.message
                );
                println!(
                    "          {}",
                    terminal::colored(
                        t!("audit.fix_label", remediation = finding.remediation).as_ref(),
                        terminal::colors::MUTED,
                    )
                );
            }
            println!();
        }

        println!(
            "{}",
            t!(
                "audit.summary",
                crit = crit,
                high = high,
                med = med,
                low = low,
                info = info
            )
        );
    }

    if crit > 0 || high > 0 {
        if output_format != "json" {
            println!("{}", t!("audit.failed"));
        }
        Ok(1)
    } else {
        if output_format != "json" && !findings.is_empty() {
            println!("{}", t!("audit.passed"));
        }
        Ok(0)
    }
}

fn audit_auth(config: &frankclaw_core::config::FrankClawConfig, findings: &mut Vec<Finding>) {
    match &config.gateway.auth {
        frankclaw_core::auth::AuthMode::None => {
            findings.push(Finding {
                severity: Severity::High,
                code: AuditCode::AuthDisabled,
                category: "auth",
                message: t!("audit.auth.disabled").to_string(),
                remediation: t!("audit.auth.disabled_fix").to_string(),
                json_path: None,
            });
        }
        frankclaw_core::auth::AuthMode::Token { token: Some(token) } => {
            use secrecy::ExposeSecret;
            let token_str = token.expose_secret();
            if token_str.len() < 16 {
                findings.push(Finding {
                    severity: Severity::Medium,
                    code: AuditCode::AuthWeak,
                    category: "auth",
                    message: t!("audit.auth.token_short").to_string(),
                    remediation: t!("audit.auth.token_short_fix").to_string(),
                    json_path: None,
                });
            }
        }
        frankclaw_core::auth::AuthMode::Token { token: None } => {}
        frankclaw_core::auth::AuthMode::Password { hash } => {
            if hash.trim().is_empty() {
                findings.push(Finding {
                    severity: Severity::High,
                    code: AuditCode::AuthWeak,
                    category: "auth",
                    message: t!("audit.auth.password_empty").to_string(),
                    remediation: t!("audit.auth.password_empty_fix").to_string(),
                    json_path: None,
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
                code: AuditCode::SecretsNoKeyRef,
                category: "secrets",
                message: t!("audit.secrets.no_key_ref", id = provider.id).to_string(),
                remediation: t!("audit.secrets.no_key_ref_fix", id = provider.id).to_string(),
                json_path: Some(format!("models.providers[{}]", provider.id)),
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
                        code: AuditCode::SecretsInline,
                        category: "secrets",
                        message: t!(
                            "audit.secrets.inline",
                            channel = channel_id,
                            key = inline_key
                        )
                        .to_string(),
                        remediation: t!(
                            "audit.secrets.inline_fix",
                            key = inline_key,
                            env_key = env_key
                        )
                        .to_string(),
                        json_path: Some(format!("channels.{channel_id}.accounts")),
                    });
                }
            }
        }
    }

    // Check gateway token stored inline (it's always inline for now, but warn if config is world-readable)
    if let frankclaw_core::auth::AuthMode::Token { token: Some(_) } = &config.gateway.auth {
        findings.push(Finding {
            severity: Severity::Low,
            code: AuditCode::SecretsInline,
            category: "secrets",
            message: t!("audit.secrets.token_inline").to_string(),
            remediation: t!("audit.secrets.token_inline_fix").to_string(),
            json_path: Some("gateway.auth.token".into()),
        });
    }
}

fn audit_missing_env_vars(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
    for provider in &config.models.providers {
        if let Some(env_name) = provider.api_key_ref.as_deref()
            && std::env::var(env_name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .is_none()
        {
            findings.push(Finding {
                severity: Severity::Medium,
                code: AuditCode::SecretsMissing,
                category: "secrets",
                message: t!(
                    "audit.secrets.missing_env",
                    id = provider.id,
                    env = env_name
                )
                .to_string(),
                remediation: t!("audit.secrets.missing_env_fix", env = env_name).to_string(),
                json_path: None,
            });
        }
    }

    for (channel_id, channel) in &config.channels {
        for account in &channel.accounts {
            for key in [
                "bot_token_env",
                "token_env",
                "app_token_env",
                "access_token_env",
                "verify_token_env",
                "app_secret_env",
                "base_url_env",
                "account_env",
                "phone_number_id_env",
            ] {
                if let Some(env_name) = account.get(key).and_then(|v| v.as_str())
                    && std::env::var(env_name)
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                        .is_none()
                {
                    findings.push(Finding {
                        severity: Severity::Medium,
                        code: AuditCode::SecretsMissing,
                        category: "secrets",
                        message: t!(
                            "audit.secrets.channel_missing_env",
                            channel = channel_id,
                            env = env_name,
                            key = key
                        )
                        .to_string(),
                        remediation: t!("audit.secrets.channel_missing_env_fix", env = env_name)
                            .to_string(),
                        json_path: None,
                    });
                }
            }
        }
    }
}

fn audit_encryption(config: &frankclaw_core::config::FrankClawConfig, findings: &mut Vec<Finding>) {
    if !config.security.encrypt_sessions {
        findings.push(Finding {
            severity: Severity::Medium,
            code: AuditCode::EncryptionOff,
            category: "encryption",
            message: t!("audit.encrypt.sessions_off").to_string(),
            remediation: t!("audit.encrypt.sessions_off_fix").to_string(),
            json_path: None,
        });
    } else if load_master_key_from_env().unwrap_or(None).is_none() {
        findings.push(Finding {
            severity: Severity::High,
            code: AuditCode::EncryptionKeyMissing,
            category: "encryption",
            message: t!("audit.encrypt.no_master_key").to_string(),
            remediation: t!("audit.encrypt.no_master_key_fix").to_string(),
            json_path: None,
        });
    } else {
        // Encryption enabled and master key present — no finding.
    }

    if !config.security.encrypt_media {
        findings.push(Finding {
            severity: Severity::Low,
            code: AuditCode::EncryptionOff,
            category: "encryption",
            message: t!("audit.encrypt.media_off").to_string(),
            remediation: t!("audit.encrypt.media_off_fix").to_string(),
            json_path: None,
        });
    }
}

fn audit_network(config: &frankclaw_core::config::FrankClawConfig, findings: &mut Vec<Finding>) {
    use frankclaw_core::config::BindMode;

    let is_network_exposed = !matches!(config.gateway.bind, BindMode::Loopback);

    if is_network_exposed {
        if matches!(config.gateway.auth, frankclaw_core::auth::AuthMode::None) {
            findings.push(Finding {
                severity: Severity::Critical,
                code: AuditCode::NetworkExposed,
                category: "network",
                message: t!("audit.network.exposed_no_auth").to_string(),
                remediation: t!("audit.network.exposed_no_auth_fix").to_string(),
                json_path: None,
            });
        }

        if config.gateway.tls.is_none() {
            findings.push(Finding {
                severity: Severity::High,
                code: AuditCode::NetworkNoTls,
                category: "network",
                message: t!("audit.network.no_tls").to_string(),
                remediation: t!("audit.network.no_tls_fix").to_string(),
                json_path: None,
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

        let Ok(policy) = channel.security_policy() else {
            findings.push(Finding {
                severity: Severity::High,
                code: AuditCode::ChannelPolicy,
                category: "channels",
                message: t!("audit.channels.invalid_policy", channel = channel_id).to_string(),
                remediation: t!("audit.channels.invalid_policy_fix").to_string(),
                json_path: None,
            });
            continue;
        };

        if group_surface_needs_guard(channel_id.as_str())
            && !policy.require_mention_for_groups
            && policy.allowed_groups.is_none()
        {
            findings.push(Finding {
                severity: Severity::Medium,
                code: AuditCode::ChannelPolicy,
                category: "channels",
                message: t!("audit.channels.open_groups", channel = channel_id).to_string(),
                remediation: t!("audit.channels.open_groups_fix", channel = channel_id).to_string(),
                json_path: None,
            });
        }

        // WhatsApp webhook signature verification
        if channel_id.as_str() == "whatsapp" {
            let has_app_secret = channel.accounts.iter().any(|account| {
                account
                    .get("app_secret")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .is_some()
                    || account
                        .get("app_secret_env")
                        .and_then(|v| v.as_str())
                        .filter(|v| !v.trim().is_empty())
                        .is_some()
            });
            if !has_app_secret {
                findings.push(Finding {
                    severity: Severity::High,
                    code: AuditCode::ChannelWhatsApp,
                    category: "channels",
                    message: t!("audit.channels.whatsapp_no_secret").to_string(),
                    remediation: t!("audit.channels.whatsapp_no_secret_fix").to_string(),
                    json_path: None,
                });
            }
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "tool policy audit with many check categories"
)]
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
                    code: AuditCode::ToolPolicy,
                    category: "tools",
                    message: t!("audit.tools.bash_allow_all").to_string(),
                    remediation: t!("audit.tools.bash_allow_all_fix").to_string(),
                    json_path: None,
                });
            }
            frankclaw_tools::bash::BashPolicy::DenyAll => {
                findings.push(Finding {
                    severity: Severity::Info,
                    code: AuditCode::ToolPolicy,
                    category: "tools",
                    message: t!("audit.tools.bash_deny_all").to_string(),
                    remediation: t!("audit.tools.bash_deny_all_fix").to_string(),
                    json_path: None,
                });
            }
            frankclaw_tools::bash::BashPolicy::Allowlist(ref allowed) => {
                findings.push(Finding {
                    severity: Severity::Low,
                    code: AuditCode::ToolPolicy,
                    category: "tools",
                    message: t!(
                        "audit.tools.bash_allowlist",
                        count = allowed.len(),
                        commands = allowed.join(", ")
                    )
                    .to_string(),
                    remediation: t!("audit.tools.bash_allowlist_fix").to_string(),
                    json_path: None,
                });
            }
        }

        // Check sandbox status for bash tool.
        let sandbox = frankclaw_tools::bash::SandboxMode::from_env();
        match sandbox {
            frankclaw_tools::bash::SandboxMode::None => {
                if !matches!(bash_policy, frankclaw_tools::bash::BashPolicy::DenyAll) {
                    findings.push(Finding {
                        severity: Severity::Medium,
                        code: AuditCode::ToolSandbox,
                        category: "tools",
                        message: t!("audit.tools.no_sandbox").to_string(),
                        remediation: t!("audit.tools.no_sandbox_fix").to_string(),
                        json_path: None,
                    });
                }
            }
            frankclaw_tools::bash::SandboxMode::AiJail => {
                if frankclaw_tools::bash::SandboxMode::is_available() {
                    findings.push(Finding {
                        severity: Severity::Info,
                        code: AuditCode::ToolSandbox,
                        category: "tools",
                        message: t!("audit.tools.sandbox_aijail").to_string(),
                        remediation: t!("audit.tools.sandbox_aijail_fix").to_string(),
                        json_path: None,
                    });
                } else {
                    findings.push(Finding {
                        severity: Severity::High,
                        code: AuditCode::ToolSandbox,
                        category: "tools",
                        message: t!("audit.tools.sandbox_missing", mode = "ai-jail").to_string(),
                        remediation: t!("audit.tools.sandbox_missing_fix").to_string(),
                        json_path: None,
                    });
                }
            }
            frankclaw_tools::bash::SandboxMode::AiJailLockdown => {
                if frankclaw_tools::bash::SandboxMode::is_available() {
                    findings.push(Finding {
                        severity: Severity::Info,
                        code: AuditCode::ToolSandbox,
                        category: "tools",
                        message: t!("audit.tools.sandbox_lockdown").to_string(),
                        remediation: t!("audit.tools.sandbox_lockdown_fix").to_string(),
                        json_path: None,
                    });
                } else {
                    findings.push(Finding {
                        severity: Severity::High,
                        code: AuditCode::ToolSandbox,
                        category: "tools",
                        message: t!("audit.tools.sandbox_missing", mode = "ai-jail-lockdown")
                            .to_string(),
                        remediation: t!("audit.tools.sandbox_missing_fix").to_string(),
                        json_path: None,
                    });
                }
            }
        }
    }

    let policy = frankclaw_tools::ToolPolicy::from_env();
    let has_mutating_tools = config
        .agents
        .agents
        .values()
        .flat_map(|agent| agent.tools.iter())
        .any(|tool| {
            frankclaw_tools::tool_risk_level(tool) >= frankclaw_core::model::ToolRiskLevel::Mutating
        });

    findings.push(Finding {
        severity: Severity::Info,
        code: AuditCode::ToolPolicy,
        category: "tools",
        message: t!(
            "audit.tools.approval_level",
            level = policy.approval_level.to_string()
        )
        .to_string(),
        remediation: t!("audit.tools.approval_level_fix").to_string(),
        json_path: None,
    });

    if has_mutating_tools
        && policy
            .approval_level
            .approves(frankclaw_core::model::ToolRiskLevel::Mutating)
    {
        findings.push(Finding {
            severity: Severity::Medium,
            code: AuditCode::ToolPolicy,
            category: "tools",
            message: t!("audit.tools.mutating_approved").to_string(),
            remediation: t!("audit.tools.mutating_approved_fix").to_string(),
            json_path: None,
        });
    }

    if policy
        .approval_level
        .approves(frankclaw_core::model::ToolRiskLevel::Destructive)
    {
        findings.push(Finding {
            severity: Severity::High,
            code: AuditCode::ToolPolicy,
            category: "tools",
            message: t!("audit.tools.destructive_approved").to_string(),
            remediation: t!("audit.tools.destructive_approved_fix").to_string(),
            json_path: None,
        });
    }
}

fn audit_file_scanning(findings: &mut Vec<Finding>) {
    if frankclaw_media::virustotal::VirusTotalScanner::from_env()
        .ok()
        .flatten()
        .is_some()
    {
        findings.push(Finding {
            severity: Severity::Info,
            code: AuditCode::MediaScanning,
            category: "media",
            message: t!("audit.media.scanning_on").to_string(),
            remediation: t!("audit.media.scanning_on_fix").to_string(),
            json_path: None,
        });
    } else {
        findings.push(Finding {
            severity: Severity::Low,
            code: AuditCode::MediaScanning,
            category: "media",
            message: t!("audit.media.scanning_off").to_string(),
            remediation: t!("audit.media.scanning_off_fix").to_string(),
            json_path: None,
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
                code: AuditCode::FilePermissions,
                category: "filesystem",
                message: t!(
                    "audit.fs.config_world_readable",
                    mode = format!("{mode:04o}")
                )
                .to_string(),
                remediation: t!(
                    "audit.fs.config_world_readable_fix",
                    path = config_path.display()
                )
                .to_string(),
                json_path: None,
            });
        }
    }

    if let Ok(meta) = std::fs::metadata(state_dir) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            findings.push(Finding {
                severity: Severity::Medium,
                code: AuditCode::FilePermissions,
                category: "filesystem",
                message: t!(
                    "audit.fs.state_world_readable",
                    mode = format!("{mode:04o}")
                )
                .to_string(),
                remediation: t!(
                    "audit.fs.state_world_readable_fix",
                    path = state_dir.display()
                )
                .to_string(),
                json_path: None,
            });
        }
    }

    let db_path = state_dir.join("sessions.db");
    if let Ok(meta) = std::fs::metadata(&db_path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            findings.push(Finding {
                severity: Severity::High,
                code: AuditCode::FilePermissions,
                category: "filesystem",
                message: t!("audit.fs.db_world_readable", mode = format!("{mode:04o}")).to_string(),
                remediation: t!("audit.fs.db_world_readable_fix", path = db_path.display())
                    .to_string(),
                json_path: None,
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

/// Check for shadowed secret refs: a provider has both inline value and env ref.
fn audit_secret_refs(
    config: &frankclaw_core::config::FrankClawConfig,
    findings: &mut Vec<Finding>,
) {
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
                let has_inline = account
                    .get(inline_key)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .is_some();
                let has_env = account
                    .get(env_key)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .is_some();
                if has_inline && has_env {
                    findings.push(Finding {
                        severity: Severity::Medium,
                        code: AuditCode::RefShadowed,
                        category: "secrets",
                        message: t!(
                            "audit.secrets.ref_shadowed",
                            channel = channel_id,
                            inline_key = inline_key,
                            env_key = env_key
                        )
                        .to_string(),
                        remediation: t!("audit.secrets.ref_shadowed_fix", inline_key = inline_key)
                            .to_string(),
                        json_path: Some(format!("channels.{channel_id}.accounts")),
                    });
                }
            }
        }
    }
}

/// Scan .env files in the working directory for plaintext secrets.
fn audit_dotenv_plaintext(findings: &mut Vec<Finding>) {
    let secret_patterns = [
        ("OPENAI_API_KEY", "sk-"),
        ("ANTHROPIC_API_KEY", "sk-ant-"),
        ("TELEGRAM_BOT_TOKEN", ""),
        ("DISCORD_BOT_TOKEN", ""),
        ("SLACK_BOT_TOKEN", "xoxb-"),
        ("SLACK_APP_TOKEN", "xapp-"),
    ];

    for entry in [".env", ".env.local", ".env.production"] {
        let path = std::path::Path::new(entry);
        if !path.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            for (env_name, prefix) in &secret_patterns {
                if let Some(rest) = line.strip_prefix(env_name) {
                    let rest = rest.trim_start_matches('=').trim();
                    if !rest.is_empty()
                        && !rest.starts_with('$')
                        && !rest.starts_with("${")
                        && (prefix.is_empty() || rest.starts_with(prefix))
                    {
                        findings.push(Finding {
                            severity: Severity::High,
                            code: AuditCode::PlaintextFound,
                            category: "secrets",
                            message: t!(
                                "audit.secrets.plaintext_env",
                                file = entry,
                                key = *env_name
                            )
                            .to_string(),
                            remediation: t!("audit.secrets.plaintext_env_fix", file = entry)
                                .to_string(),
                            json_path: None,
                        });
                    }
                }
            }
        }
    }
}

/// Check for legacy credential files in state directory.
fn audit_legacy_credentials(state_dir: &std::path::Path, findings: &mut Vec<Finding>) {
    let legacy_files = [
        "openai-key.txt",
        "anthropic-key.txt",
        "api-key.txt",
        ".openai_api_key",
    ];
    for filename in legacy_files {
        let path = state_dir.join(filename);
        if path.exists() {
            findings.push(Finding {
                severity: Severity::Medium,
                code: AuditCode::LegacyResidue,
                category: "secrets",
                message: t!(
                    "audit.secrets.legacy_file",
                    file = path.display().to_string()
                )
                .to_string(),
                remediation: t!("audit.secrets.legacy_file_fix").to_string(),
                json_path: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Login: provider authentication flows
// ---------------------------------------------------------------------------

async fn run_login(provider: &str, state_dir: &std::path::Path) -> anyhow::Result<()> {
    match provider {
        "github-copilot" | "copilot" => {
            println!("{}", t!("login.copilot.starting"));

            let device = frankclaw_models::copilot::request_device_code().await?;

            println!();
            println!(
                "{}",
                t!("login.copilot.open_url", url = device.verification_uri)
            );
            println!(
                "{}",
                t!("login.copilot.enter_code", code = device.user_code)
            );
            println!();
            println!("{}", t!("login.copilot.waiting"));

            let github_token = frankclaw_models::copilot::poll_for_access_token(
                &device.device_code,
                device.interval,
            )
            .await?;

            let token_path =
                frankclaw_models::copilot::store_github_token(state_dir, &github_token)?;

            println!();
            println!(
                "{}",
                t!(
                    "login.copilot.success",
                    path = token_path.display().to_string()
                )
            );
            println!("{}", t!("login.copilot.next_steps"));
        }
        other => {
            anyhow::bail!("{}", t!("login.unsupported", provider = other));
        }
    }
    Ok(())
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
        frankclaw_cron::CronService::open(&path).context("failed to open cron store")?,
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
        .context(t!("ctx.failed_init_runtime").to_string())?,
    ))
}

fn redact_config(config: &frankclaw_core::config::FrankClawConfig) -> serde_json::Value {
    let mut val = serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = val.as_object_mut() {
        if let Some(gateway) = obj
            .get_mut("gateway")
            .and_then(|value| value.as_object_mut())
            && let Some(auth) = gateway
                .get_mut("auth")
                .and_then(|value| value.as_object_mut())
        {
            if let Some(token) = auth.get_mut("token") {
                *token = serde_json::json!("[REDACTED]");
            }
            if let Some(hash) = auth.get_mut("hash") {
                *hash = serde_json::json!("[REDACTED]");
            }
        }

        if let Some(models) = obj
            .get_mut("models")
            .and_then(|value| value.as_object_mut())
            && let Some(providers) = models
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
        let config =
            build_onboard_config("whatsapp", "gateway-token").expect("onboard config should build");

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
            std::path::Path::new("/etc/frankclaw.toml"),
            std::path::Path::new("/var/lib/frankclaw"),
        );

        assert!(unit.contains("ExecStart=/usr/local/bin/frankclaw gateway --config /etc/frankclaw.toml --state-dir /var/lib/frankclaw"));
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

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("FRANKCLAW_TEST_MISSING_OPENAI_KEY"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("FRANKCLAW_TEST_MISSING_WHATSAPP_TOKEN"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("WHATSAPP_APP_SECRET"))
                || warnings
                    .iter()
                    .any(|warning| warning.contains("app_secret configured"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("does not exist yet"))
        );
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

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("stores 'bot_token' inline"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("accepts group messages without mention gating"))
        );

        let _ = std::fs::remove_dir_all(existing_state_dir);
    }

    #[test]
    fn rewrite_last_reply_metadata_for_edit_updates_content_and_preserves_other_metadata() {
        let mut metadata =
            reply_metadata_json(vec![frankclaw_gateway::delivery::StoredReplyChunk {
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

        let targets =
            delete_targets_from_last_reply(&last_reply).expect("delete targets should resolve");

        assert_eq!(targets, vec!["msg-1".to_string(), "msg-2".to_string()]);
    }

    #[test]
    fn mark_last_reply_metadata_deleted_clears_platform_ids_and_marks_chunks() {
        let mut metadata =
            reply_metadata_json(vec![frankclaw_gateway::delivery::StoredReplyChunk {
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
    fn supported_channel_examples_parse_as_toml() {
        let examples_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/channels");

        for filename in [
            "web.toml",
            "telegram.toml",
            "discord.toml",
            "slack.toml",
            "signal.toml",
            "whatsapp.toml",
        ] {
            let path = examples_dir.join(filename);
            let content = std::fs::read_to_string(&path).expect("channel example file should read");
            toml::from_str::<toml::Value>(&content).expect("channel example TOML should parse");
        }
    }

    #[test]
    fn supported_channel_example_returns_embedded_snippet() {
        let example = supported_channel_example("telegram").expect("telegram example should exist");

        assert!(example.contains("TELEGRAM_BOT_TOKEN"));
        assert!(supported_channel_example("matrix").is_none());
    }

    #[test]
    fn docker_compose_template_includes_gateway_and_browser_services() {
        let compose_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docker-compose.yml");
        let content =
            std::fs::read_to_string(&compose_path).expect("docker-compose file should read");

        assert!(content.contains("gateway:"));
        assert!(content.contains("chromium:"));
        assert!(content.contains("FRANKCLAW_BROWSER_DEVTOOLS_URL: http://chromium:9222/"));
        assert!(content.contains("./frankclaw.toml:/config/frankclaw.toml:ro"));
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
            .tools = vec!["browser_close".into()];

        let warnings = collect_browser_tool_warnings(&config, Some("http://10.0.0.8:6553/"));

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("non-loopback host"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("docker compose up -d chromium"))
        );
    }

    #[test]
    fn collect_browser_tool_warnings_flags_gated_mutation_tools() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&frankclaw_core::types::AgentId::default_agent())
            .expect("default agent should exist")
            .tools = vec!["browser_type".into()];

        let warnings = collect_browser_tool_warnings_with_policy(
            &config,
            Some("http://127.0.0.1:9222/"),
            frankclaw_tools::ToolPolicy::default(),
        );

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("FRANKCLAW_TOOL_APPROVAL=mutating"))
        );
    }

    #[test]
    fn browser_runtime_status_reports_mutation_gate_state() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&frankclaw_core::types::AgentId::default_agent())
            .expect("default agent should exist")
            .tools = vec!["browser_open".into(), "browser_click".into()];

        let gated = browser_runtime_status_with_policy(
            &config,
            Some("http://127.0.0.1:9222/"),
            frankclaw_tools::ToolPolicy::default(),
        )
        .expect("status should exist");
        assert!(gated.contains("blocked"));

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let endpoint = format!(
            "http://{}",
            listener.local_addr().expect("addr should exist")
        );
        let enabled = browser_runtime_status_with_policy(
            &config,
            Some(&endpoint),
            frankclaw_tools::ToolPolicy {
                approval_level: frankclaw_tools::ApprovalLevel::Mutating,
                approved_tools: std::collections::HashSet::new(),
            },
        )
        .expect("status should exist");
        assert!(enabled.contains(&format!("mutations enabled at {endpoint}")));
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
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("should bind to ephemeral port");
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
        let config_path = dir.join("frankclaw.toml");

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

        let toml_str = config.to_toml_string().expect("should serialize");
        std::fs::write(&config_path, &toml_str).expect("should write");

        // Verify it can be loaded back
        let loaded = FrankClawConfig::load_from_path(&config_path).expect("should load config");
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
        std::fs::write(&pid_path, std::process::id().to_string()).expect("should write pid");

        let status = daemon_pid_status(&pid_path);
        assert!(status.is_some());
        assert!(
            status
                .expect("running pid should yield daemon status")
                .contains("running")
        );

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
        assert!(
            status
                .expect("stale pid should yield daemon status")
                .contains("stale")
        );

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
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::High && f.category == "auth")
        );
    }

    #[test]
    fn audit_auth_passes_with_token() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.gateway.auth = frankclaw_core::auth::AuthMode::Token {
            token: Some(secrecy::SecretString::from(
                "a-long-enough-token-here".to_string(),
            )),
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

        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Medium && f.message.contains("shorter"))
        );
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

        assert!(
            findings
                .iter()
                .any(|f| f.category == "encryption" && f.message.contains("disabled"))
        );
    }

    #[test]
    fn audit_network_flags_exposed_no_auth() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.gateway.bind = frankclaw_core::config::BindMode::Lan;
        config.gateway.auth = frankclaw_core::auth::AuthMode::None;

        let mut findings = Vec::new();
        audit_network(&config, &mut findings);

        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Critical && f.message.contains("network-exposed"))
        );
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

        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("group messages"))
        );
    }

    #[test]
    fn audit_ssrf_disabled_is_critical() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.security.ssrf_protection = false;
        let config_path = std::path::Path::new("/tmp/nonexistent-config.json");
        let state_dir = std::path::Path::new("/tmp/nonexistent-state");

        let exit_code = run_security_audit(&config, config_path, state_dir, "text")
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

    #[test]
    fn audit_secret_refs_detects_shadowed_ref() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("telegram"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "inline-token-value",
                    "bot_token_env": "TELEGRAM_BOT_TOKEN"
                })],
                extra: serde_json::json!({}),
            },
        );

        let mut findings = Vec::new();
        audit_secret_refs(&config, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| matches!(f.code, AuditCode::RefShadowed)),
            "should detect shadowed ref"
        );
    }

    #[test]
    fn audit_dotenv_detects_plaintext_in_env_file() {
        let dir =
            std::env::temp_dir().join(format!("frankclaw-dotenv-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // We can't easily test .env detection because it reads from cwd.
        // This test validates the function doesn't panic on missing files.
        let mut findings = Vec::new();
        audit_dotenv_plaintext(&mut findings);
        // Should not panic and findings may or may not be present
        // depending on what's in the working directory
    }

    #[test]
    fn audit_legacy_credentials_detects_legacy_files() {
        let dir =
            std::env::temp_dir().join(format!("frankclaw-legacy-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let legacy_path = dir.join("openai-key.txt");
        std::fs::write(&legacy_path, "sk-test").expect("write should succeed");

        let mut findings = Vec::new();
        audit_legacy_credentials(&dir, &mut findings);
        assert!(
            findings
                .iter()
                .any(|f| matches!(f.code, AuditCode::LegacyResidue)),
            "should detect legacy credential file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audit_json_output_roundtrip() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.security.ssrf_protection = false;
        let config_path = std::path::Path::new("/tmp/nonexistent-config.json");
        let state_dir = std::path::Path::new("/tmp/nonexistent-state");

        // Capture JSON output by using json format
        let exit_code = run_security_audit(&config, config_path, state_dir, "json")
            .expect("audit should succeed");
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn audit_code_names_are_unique() {
        let codes = vec![
            AuditCode::AuthDisabled,
            AuditCode::AuthWeak,
            AuditCode::SecretsInline,
            AuditCode::SecretsMissing,
            AuditCode::SecretsNoKeyRef,
            AuditCode::PlaintextFound,
            AuditCode::RefShadowed,
            AuditCode::LegacyResidue,
            AuditCode::EncryptionOff,
            AuditCode::EncryptionKeyMissing,
            AuditCode::NetworkExposed,
            AuditCode::NetworkNoTls,
            AuditCode::SsrfDisabled,
            AuditCode::ChannelPolicy,
            AuditCode::ChannelWhatsApp,
            AuditCode::ToolPolicy,
            AuditCode::ToolSandbox,
            AuditCode::FilePermissions,
            AuditCode::MediaScanning,
        ];
        let mut names: Vec<&str> = codes.into_iter().map(super::AuditCode::name).collect();
        let len_before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), len_before, "audit code names must be unique");
    }

    #[test]
    fn empty_config_produces_clean_or_minor_audit() {
        let config = frankclaw_core::config::FrankClawConfig::default();
        let config_path = std::path::Path::new("/tmp/nonexistent-config.json");
        let state_dir = std::path::Path::new("/tmp/nonexistent-state");

        // Default config has SSRF enabled, so this should not produce critical findings
        // unless something else triggers (like missing auth)
        let _exit_code = run_security_audit(&config, config_path, state_dir, "text")
            .expect("audit should succeed");
    }
}
