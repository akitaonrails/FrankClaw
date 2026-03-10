#![forbid(unsafe_code)]

pub mod discord;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod web;

use std::collections::HashMap;
use std::sync::Arc;

use secrecy::SecretString;
use serde_json::Value;

use frankclaw_core::channel::ChannelPlugin;
use frankclaw_core::config::{ChannelConfig, FrankClawConfig};
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::ChannelId;

pub struct ChannelSet {
    channels: HashMap<ChannelId, Arc<dyn ChannelPlugin>>,
    web: Option<Arc<web::WebChannel>>,
}

impl ChannelSet {
    pub fn from_parts(
        channels: HashMap<ChannelId, Arc<dyn ChannelPlugin>>,
        web: Option<Arc<web::WebChannel>>,
    ) -> Self {
        Self { channels, web }
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
}

pub fn load_from_config(config: &FrankClawConfig) -> Result<ChannelSet> {
    let mut channels: HashMap<ChannelId, Arc<dyn ChannelPlugin>> = HashMap::new();
    let mut web_handle = None;

    for (channel_id, channel_config) in &config.channels {
        if !channel_config.enabled {
            continue;
        }

        match build_channel(channel_id, channel_config)? {
            LoadedChannel::Web(web) => {
                channels.insert(channel_id.clone(), web.clone());
                web_handle = Some(web);
            }
            LoadedChannel::Standard(channel) => {
                channels.insert(channel_id.clone(), channel);
            }
        };
    }

    Ok(ChannelSet::from_parts(
        channels,
        web_handle,
    ))
}

enum LoadedChannel {
    Standard(Arc<dyn ChannelPlugin>),
    Web(Arc<web::WebChannel>),
}

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
                telegram::TelegramChannel::new(bot_token),
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
                discord::DiscordChannel::new(bot_token),
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
                signal::SignalChannel::new(base_url, account_id),
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
                slack::SlackChannel::new(app_token, bot_token),
            )))
        }
        other => Err(FrankClawError::ConfigValidation {
            msg: format!(
                "unsupported enabled channel '{}'; currently supported: web, telegram, discord, signal, slack",
                other
            ),
        }),
    }
}

fn first_account<'a>(channel_id: &ChannelId, channel_config: &'a ChannelConfig) -> Result<&'a Value> {
    channel_config.accounts.first().ok_or_else(|| {
        FrankClawError::ConfigValidation {
            msg: format!("{channel_id} channel requires at least one account"),
        }
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

    Err(FrankClawError::ConfigValidation {
        msg: format!("channel '{channel}' requires a non-empty {label}"),
    })
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
        return Err(FrankClawError::ConfigValidation {
            msg: format!("channel '{channel}' references an empty environment variable for {label}"),
        });
    }

    let value = std::env::var(env_name).map_err(|_| FrankClawError::ConfigValidation {
        msg: format!("channel '{channel}' references missing environment variable '{env_name}'"),
    })?;

    if value.trim().is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("channel '{channel}' environment variable '{env_name}' is empty"),
        });
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
}
