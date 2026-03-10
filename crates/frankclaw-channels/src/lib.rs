#![forbid(unsafe_code)]

pub mod discord;
pub mod telegram;
pub mod web;

use std::collections::HashMap;
use std::sync::Arc;

use secrecy::SecretString;

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
            let account = channel_config.accounts.first().ok_or_else(|| {
                FrankClawError::ConfigValidation {
                    msg: "telegram channel requires at least one account".into(),
                }
            })?;
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
            let account = channel_config.accounts.first().ok_or_else(|| {
                FrankClawError::ConfigValidation {
                    msg: "discord channel requires at least one account".into(),
                }
            })?;
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
        other => Err(FrankClawError::ConfigValidation {
            msg: format!(
                "unsupported enabled channel '{}'; currently supported: web, telegram, discord",
                other
            ),
        }),
    }
}

fn resolve_channel_secret(
    account: &serde_json::Value,
    inline_keys: &[&str],
    env_keys: &[&str],
    default_env: &str,
    channel: &str,
) -> Result<SecretString> {
    for key in inline_keys {
        if let Some(value) = account.get(*key).and_then(|value| value.as_str()) {
            if !value.trim().is_empty() {
                return Ok(SecretString::from(value.to_string()));
            }
        }
    }

    for key in env_keys {
        if let Some(env_name) = account.get(*key).and_then(|value| value.as_str()) {
            return resolve_env_secret(env_name, channel);
        }
    }

    resolve_env_secret(default_env, channel)
}

fn resolve_env_secret(env_name: &str, channel: &str) -> Result<SecretString> {
    let env_name = env_name.trim();
    if env_name.is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("channel '{}' references an empty secret environment variable", channel),
        });
    }

    let value = std::env::var(env_name).map_err(|_| FrankClawError::ConfigValidation {
        msg: format!(
            "channel '{}' references missing environment variable '{}'",
            channel, env_name
        ),
    })?;

    if value.trim().is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!(
                "channel '{}' environment variable '{}' is empty",
                channel, env_name
            ),
        });
    }

    Ok(SecretString::from(value))
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
}
