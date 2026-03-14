#![forbid(unsafe_code)]

pub mod discord;
pub mod email;
mod inbound_media;
mod media_text;
mod outbound_media;
mod outbound_text;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod whatsapp;
pub mod web;

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::Client;
use secrecy::SecretString;
use serde_json::Value;

use frankclaw_core::channel::ChannelPlugin;
use frankclaw_core::config::{ChannelConfig, FrankClawConfig};
use frankclaw_core::error::{Result, Internal, ConfigValidation};
use frankclaw_core::types::ChannelId;

/// Default timeout for channel HTTP clients (seconds).
const CHANNEL_HTTP_TIMEOUT_SECS: u64 = 60;

/// Build the standard HTTP client used by channel adapters.
pub fn build_channel_http_client() -> Result<Client> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(CHANNEL_HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| Internal {
            msg: format!("failed to build channel HTTP client: {e}"),
        }.build())
}

pub struct ChannelSet {
    channels: HashMap<ChannelId, Arc<dyn ChannelPlugin>>,
    web: Option<Arc<web::WebChannel>>,
    whatsapp: Option<Arc<whatsapp::WhatsAppChannel>>,
}

impl ChannelSet {
    pub fn from_parts(
        channels: HashMap<ChannelId, Arc<dyn ChannelPlugin>>,
        web: Option<Arc<web::WebChannel>>,
        whatsapp: Option<Arc<whatsapp::WhatsAppChannel>>,
    ) -> Self {
        Self { channels, web, whatsapp }
    }

    pub fn channels(&self) -> &HashMap<ChannelId, Arc<dyn ChannelPlugin>> {
        &self.channels
    }

    pub fn get(&self, id: &ChannelId) -> Option<&Arc<dyn ChannelPlugin>> {
        self.channels.get(id)
    }

    pub fn web(&self) -> Option<Arc<web::WebChannel>> {
        self.web.clone()
    }

    pub fn whatsapp(&self) -> Option<Arc<whatsapp::WhatsAppChannel>> {
        self.whatsapp.clone()
    }
}

pub fn load_from_config(config: &FrankClawConfig) -> Result<ChannelSet> {
    let mut channels: HashMap<ChannelId, Arc<dyn ChannelPlugin>> = HashMap::new();
    let mut web_handle = None;
    let mut whatsapp_handle = None;

    for (channel_id, channel_config) in &config.channels {
        if !channel_config.enabled {
            continue;
        }

        match build_channel(channel_id, channel_config)? {
            LoadedChannel::Web(web) => {
                channels.insert(channel_id.clone(), web.clone());
                web_handle = Some(web);
            }
            LoadedChannel::WhatsApp(whatsapp) => {
                channels.insert(channel_id.clone(), whatsapp.clone());
                whatsapp_handle = Some(whatsapp);
            }
            LoadedChannel::Standard(channel) => {
                channels.insert(channel_id.clone(), channel);
            }
        }
    }

    Ok(ChannelSet::from_parts(
        channels,
        web_handle,
        whatsapp_handle,
    ))
}

enum LoadedChannel {
    Standard(Arc<dyn ChannelPlugin>),
    Web(Arc<web::WebChannel>),
    WhatsApp(Arc<whatsapp::WhatsAppChannel>),
}

#[expect(clippy::too_many_lines, reason = "channel construction dispatch; splitting would scatter related config parsing")]
fn build_channel(channel_id: &ChannelId, channel_config: &ChannelConfig) -> Result<LoadedChannel> {
    match channel_id.as_str() {
        "web" => Ok(LoadedChannel::Web(Arc::new(web::WebChannel::new()))),
        "telegram" => {
            let account = first_account(channel_id, channel_config)?;
            let bot_token = resolve_channel_secret(
                account,
                &["bot_token", "token"],
                &["bot_token_env", "token_env"],
                "TELEGRAM_BOT_TOKEN",
                "telegram",
            )?;
            Ok(LoadedChannel::Standard(Arc::new(
                telegram::TelegramChannel::new(bot_token)?,
            )))
        }
        "discord" => {
            let account = first_account(channel_id, channel_config)?;
            let bot_token = resolve_channel_secret(
                account,
                &["bot_token", "token"],
                &["bot_token_env", "token_env"],
                "DISCORD_BOT_TOKEN",
                "discord",
            )?;
            Ok(LoadedChannel::Standard(Arc::new(
                discord::DiscordChannel::new(bot_token)?,
            )))
        }
        "signal" => {
            let account = first_account(channel_id, channel_config)?;
            let base_url = resolve_channel_value(
                account,
                &["base_url", "http_url"],
                &["base_url_env", "http_url_env"],
                Some("SIGNAL_BASE_URL"),
                "signal",
                "base URL",
            )?;
            let account_id = resolve_optional_channel_value(
                account,
                &["account", "signal_number"],
                &["account_env", "signal_number_env"],
            )?;
            Ok(LoadedChannel::Standard(Arc::new(
                signal::SignalChannel::new(base_url, account_id)?,
            )))
        }
        "whatsapp" => {
            let account = first_account(channel_id, channel_config)?;
            let access_token = resolve_channel_secret(
                account,
                &["access_token", "token"],
                &["access_token_env", "token_env"],
                "WHATSAPP_ACCESS_TOKEN",
                "whatsapp",
            )?;
            let phone_number_id = resolve_channel_value(
                account,
                &["phone_number_id"],
                &["phone_number_id_env"],
                Some("WHATSAPP_PHONE_NUMBER_ID"),
                "whatsapp",
                "phone number id",
            )?;
            let verify_token = SecretString::from(resolve_channel_value(
                account,
                &["verify_token"],
                &["verify_token_env"],
                Some("WHATSAPP_VERIFY_TOKEN"),
                "whatsapp",
                "verify token",
            )?);
            let app_secret = resolve_optional_channel_secret(
                account,
                &["app_secret"],
                &["app_secret_env"],
            )?;
            Ok(LoadedChannel::WhatsApp(Arc::new(
                whatsapp::WhatsAppChannel::new(
                    access_token,
                    phone_number_id,
                    verify_token,
                    app_secret,
                )?,
            )))
        }
        "slack" => {
            let account = first_account(channel_id, channel_config)?;
            let app_token = resolve_channel_secret(
                account,
                &["app_token"],
                &["app_token_env"],
                "SLACK_APP_TOKEN",
                "slack",
            )?;
            let bot_token = resolve_channel_secret(
                account,
                &["bot_token", "token"],
                &["bot_token_env", "token_env"],
                "SLACK_BOT_TOKEN",
                "slack",
            )?;
            Ok(LoadedChannel::Standard(Arc::new(
                slack::SlackChannel::new(app_token, bot_token)?,
            )))
        }
        "email" => {
            let account = first_account(channel_id, channel_config)?;
            let imap_server = resolve_channel_value(
                account,
                &["imap_server"],
                &["imap_server_env"],
                None,
                "email",
                "IMAP server",
            )?;
            let imap_port = account
                .get("imap_port")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(993) as u16;
            let imap_user = resolve_channel_value(
                account,
                &["imap_user", "user"],
                &["imap_user_env"],
                None,
                "email",
                "IMAP user",
            )?;
            let imap_password = resolve_channel_secret(
                account,
                &["imap_password", "password"],
                &["imap_password_env"],
                "EMAIL_PASSWORD",
                "email",
            )?;
            let smtp_server = resolve_channel_value(
                account,
                &["smtp_server"],
                &["smtp_server_env"],
                None,
                "email",
                "SMTP server",
            )?;
            let smtp_port = account
                .get("smtp_port")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(587) as u16;
            let smtp_user = resolve_channel_value(
                account,
                &["smtp_user", "user"],
                &["smtp_user_env"],
                None,
                "email",
                "SMTP user",
            )?;
            let smtp_password = resolve_channel_secret(
                account,
                &["smtp_password", "password"],
                &["smtp_password_env"],
                "EMAIL_PASSWORD",
                "email",
            )?;
            let smtp_from = resolve_channel_value(
                account,
                &["smtp_from", "from", "user"],
                &["smtp_from_env"],
                None,
                "email",
                "SMTP from address",
            )?;
            let poll_interval_secs = account
                .get("poll_interval_secs")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30);
            let allowed_senders: Vec<String> = account
                .get("allowed_senders")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();

            Ok(LoadedChannel::Standard(Arc::new(
                email::EmailChannel::new(
                    imap_server,
                    imap_port,
                    imap_user,
                    imap_password,
                    smtp_server,
                    smtp_port,
                    smtp_user,
                    smtp_password,
                    smtp_from,
                    poll_interval_secs,
                    allowed_senders,
                ),
            )))
        }
        other => ConfigValidation {
            msg: format!(
                "unsupported enabled channel '{other}'; currently supported: web, telegram, discord, signal, slack, whatsapp, email"
            ),
        }.fail(),
    }
}

fn first_account<'cfg>(channel_id: &ChannelId, channel_config: &'cfg ChannelConfig) -> Result<&'cfg Value> {
    channel_config.accounts.first().ok_or_else(|| {
        ConfigValidation {
            msg: format!("{channel_id} channel requires at least one account"),
        }.build()
    })
}

fn resolve_channel_secret(
    account: &Value,
    inline_keys: &[&str],
    env_keys: &[&str],
    default_env: &str,
    channel: &str,
) -> Result<SecretString> {
    Ok(SecretString::from(resolve_channel_value(
        account,
        inline_keys,
        env_keys,
        Some(default_env),
        channel,
        "secret",
    )?))
}

fn resolve_channel_value(
    account: &Value,
    inline_keys: &[&str],
    env_keys: &[&str],
    default_env: Option<&str>,
    channel: &str,
    label: &str,
) -> Result<String> {
    if let Some(value) = resolve_inline_value(account, inline_keys) {
        return Ok(value);
    }

    for key in env_keys {
        if let Some(env_name) = account.get(*key).and_then(|value| value.as_str()) {
            return resolve_env_value(env_name, channel, label);
        }
    }

    if let Some(default_env) = default_env {
        return resolve_env_value(default_env, channel, label);
    }

    ConfigValidation {
        msg: format!("channel '{channel}' requires a non-empty {label}"),
    }.fail()
}

fn resolve_optional_channel_value(
    account: &Value,
    inline_keys: &[&str],
    env_keys: &[&str],
) -> Result<Option<String>> {
    if let Some(value) = resolve_inline_value(account, inline_keys) {
        return Ok(Some(value));
    }

    for key in env_keys {
        if let Some(env_name) = account.get(*key).and_then(|value| value.as_str()) {
            return Ok(Some(resolve_env_value(env_name, "signal", "value")?));
        }
    }

    Ok(None)
}

fn resolve_optional_channel_secret(
    account: &Value,
    inline_keys: &[&str],
    env_keys: &[&str],
) -> Result<Option<SecretString>> {
    resolve_optional_channel_value(account, inline_keys, env_keys)
        .map(|value| value.map(SecretString::from))
}

fn resolve_inline_value(account: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        account
            .get(*key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn resolve_env_value(env_name: &str, channel: &str, label: &str) -> Result<String> {
    let env_name = env_name.trim();
    if env_name.is_empty() {
        return ConfigValidation {
            msg: format!("channel '{channel}' references an empty environment variable for {label}"),
        }.fail();
    }

    let value = std::env::var(env_name).map_err(|_| ConfigValidation {
        msg: format!("channel '{channel}' references missing environment variable '{env_name}'"),
    }.build())?;

    if value.trim().is_empty() {
        return ConfigValidation {
            msg: format!("channel '{channel}' environment variable '{env_name}' is empty"),
        }.fail();
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_config_builds_web_channel() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("web"),
            ChannelConfig {
                enabled: true,
                accounts: Vec::new(),
                extra: serde_json::json!({}),
            },
        );

        let channels = load_from_config(&config).expect("channels should load");
        assert!(channels.web().is_some());
        assert!(channels.get(&ChannelId::new("web")).is_some());
    }

    #[test]
    fn load_from_config_builds_telegram_from_inline_token() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("telegram"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "test-token"
                })],
                extra: serde_json::json!({}),
            },
        );

        let channels = load_from_config(&config).expect("channels should load");
        let channel = channels
            .get(&ChannelId::new("telegram"))
            .expect("telegram channel should exist");
        assert_eq!(channel.label(), "Telegram");
    }

    #[test]
    fn load_from_config_builds_discord_from_inline_token() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("discord"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "bot_token": "test-token"
                })],
                extra: serde_json::json!({}),
            },
        );

        let channels = load_from_config(&config).expect("channels should load");
        let channel = channels
            .get(&ChannelId::new("discord"))
            .expect("discord channel should exist");
        assert_eq!(channel.label(), "Discord");
    }

    #[test]
    fn load_from_config_builds_slack_from_inline_tokens() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("slack"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "app_token": "xapp-test",
                    "bot_token": "xoxb-test"
                })],
                extra: serde_json::json!({}),
            },
        );

        let channels = load_from_config(&config).expect("channels should load");
        let channel = channels
            .get(&ChannelId::new("slack"))
            .expect("slack channel should exist");
        assert_eq!(channel.label(), "Slack");
    }

    #[test]
    fn load_from_config_builds_signal_from_inline_base_url() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("signal"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "base_url": "http://127.0.0.1:8080",
                    "account": "+15551234567"
                })],
                extra: serde_json::json!({}),
            },
        );

        let channels = load_from_config(&config).expect("channels should load");
        let channel = channels
            .get(&ChannelId::new("signal"))
            .expect("signal channel should exist");
        assert_eq!(channel.label(), "Signal");
    }

    #[test]
    fn load_from_config_builds_whatsapp_from_inline_values() {
        let mut config = FrankClawConfig::default();
        config.channels.insert(
            ChannelId::new("whatsapp"),
            ChannelConfig {
                enabled: true,
                accounts: vec![serde_json::json!({
                    "access_token": "test-token",
                    "phone_number_id": "123456789",
                    "verify_token": "verify-me"
                })],
                extra: serde_json::json!({}),
            },
        );

        let channels = load_from_config(&config).expect("channels should load");
        let channel = channels
            .get(&ChannelId::new("whatsapp"))
            .expect("whatsapp channel should exist");
        assert_eq!(channel.label(), "WhatsApp");
        assert!(channels.whatsapp().is_some());
    }
}
