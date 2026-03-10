#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use secrecy::SecretString;

use frankclaw_core::config::{AgentDef, FrankClawConfig, ProviderConfig};
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::channel::InboundMessage;
use frankclaw_core::model::{CompletionMessage, CompletionRequest, ModelDef, Usage};
use frankclaw_core::session::{SessionEntry, SessionStore, TranscriptEntry};
use frankclaw_core::types::{AgentId, ChannelId, Role, SessionKey};
use frankclaw_models::{
    AnthropicProvider, FailoverChain, OllamaProvider, OpenAiProvider,
};

pub struct Runtime {
    config: FrankClawConfig,
    sessions: Arc<dyn SessionStore>,
    models: FailoverChain,
    model_defs: Vec<ModelDef>,
    channel_ids: Vec<ChannelId>,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub agent_id: Option<AgentId>,
    pub session_key: Option<SessionKey>,
    pub message: String,
    pub model_id: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub session_key: SessionKey,
    pub model_id: String,
    pub content: String,
    pub usage: Usage,
}

impl Runtime {
    pub async fn from_config(
        config: &FrankClawConfig,
        sessions: Arc<dyn SessionStore>,
    ) -> Result<Self> {
        let providers = build_providers(config)?;
        let cooldown_secs = config
            .models
            .providers
            .iter()
            .map(|provider| provider.cooldown_secs)
            .max()
            .unwrap_or(30)
            .max(1);
        let models = FailoverChain::new(providers, cooldown_secs);
        let model_defs = models.list_models().await?;
        let channel_ids = config
            .channels
            .iter()
            .filter_map(|(channel_id, channel)| {
                if channel.enabled {
                    Some(channel_id.clone())
                } else {
                    None
                }
            })
            .collect();

        Ok(Self {
            config: config.clone(),
            sessions,
            models,
            model_defs,
            channel_ids,
        })
    }

    pub fn list_models(&self) -> &[ModelDef] {
        &self.model_defs
    }

    pub fn list_channels(&self) -> &[ChannelId] {
        &self.channel_ids
    }

    pub fn session_key_for_inbound(
        &self,
        inbound: &InboundMessage,
    ) -> SessionKey {
        let account_scope = self.config.session.scoping.resolve_inbound_account_scope(
            &inbound.account_id,
            &inbound.sender_id,
            inbound.thread_id.as_deref(),
            inbound.is_group,
        );

        SessionKey::new(
            &self.config.agents.default_agent,
            &inbound.channel,
            &account_scope,
        )
    }

    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        if request.message.trim().is_empty() {
            return Err(FrankClawError::InvalidRequest {
                msg: "message is required".into(),
            });
        }

        let (agent_id, agent) = self.resolve_agent(request.agent_id.as_ref())?;
        let model_id = self.resolve_model_id(&agent, request.model_id.as_deref())?;
        let session_key = self.resolve_session_key(&agent_id, request.session_key)?;
        let history = self.sessions.get_transcript(&session_key, 200, None).await?;
        let next_seq = history.last().map(|entry| entry.seq + 1).unwrap_or(1);

        self.ensure_session(&session_key, &agent_id).await?;

        let request_messages = history
            .iter()
            .map(|entry| CompletionMessage {
                role: entry.role,
                content: entry.content.clone(),
            })
            .chain(std::iter::once(CompletionMessage {
                role: Role::User,
                content: request.message.clone(),
            }))
            .collect();

        self.sessions
            .append_transcript(
                &session_key,
                &TranscriptEntry {
                    seq: next_seq,
                    role: Role::User,
                    content: request.message,
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await?;

        let response = self
            .models
            .complete(
                CompletionRequest {
                    model_id: model_id.clone(),
                    messages: request_messages,
                    max_tokens: request.max_tokens,
                    temperature: request.temperature,
                    system: agent.system_prompt.clone(),
                    tools: Vec::new(),
                },
                None,
            )
            .await?;

        self.sessions
            .append_transcript(
                &session_key,
                &TranscriptEntry {
                    seq: next_seq + 1,
                    role: Role::Assistant,
                    content: response.content.clone(),
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await?;

        Ok(ChatResponse {
            session_key,
            model_id,
            content: response.content,
            usage: response.usage,
        })
    }

    fn resolve_agent(&self, requested: Option<&AgentId>) -> Result<(AgentId, AgentDef)> {
        let agent_id = requested
            .cloned()
            .unwrap_or_else(|| self.config.agents.default_agent.clone());
        let agent = self
            .config
            .agents
            .agents
            .get(&agent_id)
            .cloned()
            .ok_or_else(|| FrankClawError::AgentNotFound {
                agent_id: agent_id.clone(),
            })?;
        Ok((agent_id, agent))
    }

    fn resolve_model_id(
        &self,
        agent: &AgentDef,
        requested: Option<&str>,
    ) -> Result<String> {
        if let Some(model_id) = requested {
            return Ok(model_id.to_string());
        }
        if let Some(model_id) = &agent.model {
            return Ok(model_id.clone());
        }
        if let Some(model_id) = &self.config.models.default_model {
            return Ok(model_id.clone());
        }
        self.model_defs
            .first()
            .map(|model| model.id.clone())
            .ok_or_else(|| FrankClawError::ConfigValidation {
                msg: "no model providers are configured".into(),
            })
    }

    fn resolve_session_key(
        &self,
        agent_id: &AgentId,
        explicit: Option<SessionKey>,
    ) -> Result<SessionKey> {
        if let Some(session_key) = explicit {
            if let Some((session_agent_id, _, _)) = session_key.parse() {
                if &session_agent_id != agent_id {
                    return Err(FrankClawError::InvalidRequest {
                        msg: format!(
                            "session '{}' does not belong to agent '{}'",
                            session_key, agent_id
                        ),
                    });
                }
            }
            return Ok(session_key);
        }

        Ok(SessionKey::new(
            agent_id,
            &ChannelId::new("web"),
            "control",
        ))
    }

    async fn ensure_session(
        &self,
        session_key: &SessionKey,
        agent_id: &AgentId,
    ) -> Result<()> {
        if self.sessions.get(session_key).await?.is_some() {
            return Ok(());
        }

        let (channel, account_id) = session_key
            .parse()
            .map(|(_, channel, account_id)| (channel, account_id))
            .unwrap_or_else(|| (ChannelId::new("web"), "control".to_string()));

        self.sessions
            .upsert(&SessionEntry {
                key: session_key.clone(),
                agent_id: agent_id.clone(),
                channel,
                account_id,
                scoping: self.config.session.scoping,
                created_at: Utc::now(),
                last_message_at: None,
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
    }
}

fn build_providers(
    config: &FrankClawConfig,
) -> Result<Vec<Arc<dyn frankclaw_core::model::ModelProvider>>> {
    let mut providers: Vec<Arc<dyn frankclaw_core::model::ModelProvider>> = Vec::new();
    let mut seen_ids = HashSet::new();

    for provider in &config.models.providers {
        if !seen_ids.insert(provider.id.clone()) {
            return Err(FrankClawError::ConfigValidation {
                msg: format!("duplicate model provider id '{}'", provider.id),
            });
        }

        let provider_impl: Arc<dyn frankclaw_core::model::ModelProvider> =
            match provider.api.as_str() {
                "openai" => Arc::new(OpenAiProvider::new(
                    provider.id.clone(),
                    provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                    resolve_secret(provider, "OPENAI_API_KEY")?,
                    provider.models.clone(),
                )),
                "anthropic" => Arc::new(AnthropicProvider::new(
                    provider.id.clone(),
                    resolve_secret(provider, "ANTHROPIC_API_KEY")?,
                    provider.models.clone(),
                )),
                "ollama" => Arc::new(OllamaProvider::new(
                    provider.id.clone(),
                    provider.base_url.clone(),
                )),
                other => {
                    return Err(FrankClawError::ConfigValidation {
                        msg: format!(
                            "unsupported model provider api '{}'; expected openai, anthropic, or ollama",
                            other
                        ),
                    });
                }
            };
        providers.push(provider_impl);
    }

    Ok(providers)
}

fn resolve_secret(provider: &ProviderConfig, default_env: &str) -> Result<SecretString> {
    let env_key = provider
        .api_key_ref
        .as_deref()
        .unwrap_or(default_env)
        .trim();
    if env_key.is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("provider '{}' requires an api_key_ref", provider.id),
        });
    }

    let value = std::env::var(env_key).map_err(|_| FrankClawError::ConfigValidation {
        msg: format!(
            "provider '{}' references missing environment variable '{}'",
            provider.id, env_key
        ),
    })?;

    if value.trim().is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!(
                "provider '{}' environment variable '{}' is empty",
                provider.id, env_key
            ),
        });
    }

    Ok(SecretString::from(value))
}
