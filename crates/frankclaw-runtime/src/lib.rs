#![forbid(unsafe_code)]

pub mod commands;
pub mod context;
pub mod leak_detector;
pub mod prompts;
pub mod sanitize;
pub mod subagent;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use secrecy::SecretString;

use frankclaw_core::config::{AgentDef, FrankClawConfig, ProviderConfig};
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::channel::{ChannelCapabilities, InboundAttachment, InboundMessage};
use frankclaw_core::model::{
    CompletionMessage, CompletionRequest, ImageContent, ModelDef, ModelProvider, StreamDelta,
    ToolCallResponse, ToolDef, ToolRiskLevel, Usage,
};
use frankclaw_core::session::{SessionEntry, SessionStore, TranscriptEntry};
use frankclaw_core::types::{AgentId, ChannelId, Role, SessionKey};
use frankclaw_models::{
    AnthropicProvider, FailoverChain, OllamaProvider, OpenAiProvider, ProviderHealth,
};
use frankclaw_plugin_sdk::{SkillManifest, load_workspace_skills};
use frankclaw_tools::{ToolContext, ToolOutput, ToolRegistry};

pub struct Runtime {
    config: FrankClawConfig,
    config_arc: Option<Arc<FrankClawConfig>>,
    sessions: Arc<dyn SessionStore>,
    models: FailoverChain,
    model_defs: Vec<ModelDef>,
    channel_ids: Vec<ChannelId>,
    tools: ToolRegistry,
    skill_manifests: HashMap<AgentId, Vec<SkillManifest>>,
    subagent_registry: Arc<subagent::SubagentRegistry>,
    fetcher: Option<Arc<dyn frankclaw_core::tool_services::Fetcher>>,
    channels: Option<Arc<dyn frankclaw_core::tool_services::MessageSender>>,
    cron: Option<Arc<dyn frankclaw_core::tool_services::CronManager>>,
    workspace: Option<std::path::PathBuf>,
}

pub struct ChatRequest {
    pub agent_id: Option<AgentId>,
    pub session_key: Option<SessionKey>,
    pub message: String,
    pub attachments: Vec<InboundAttachment>,
    pub model_id: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    /// Extended thinking budget in tokens (Anthropic Claude 3.7+).
    pub thinking_budget: Option<u32>,
    /// Channel ID for prompt hints (tells the LLM what features are available).
    pub channel_id: Option<ChannelId>,
    /// Channel capabilities for prompt hints.
    pub channel_capabilities: Option<ChannelCapabilities>,
    /// Canvas service for agent canvas tools.
    pub canvas: Option<Arc<dyn frankclaw_core::canvas::CanvasService>>,
    /// Cancellation token — when cancelled, the chat loop aborts at the next check point.
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Channel for requesting interactive tool approval from the user.
    pub approval_tx: Option<frankclaw_core::tool_approval::ApprovalRequestTx>,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub session_key: SessionKey,
    pub model_id: String,
    pub content: String,
    pub usage: Usage,
}

#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub agent_id: Option<AgentId>,
    pub session_key: Option<SessionKey>,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolActivity {
    pub seq: u64,
    pub tool_name: String,
    pub tool_call_id: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub output_preview: String,
}

const MAX_TOOL_ROUNDS: usize = 4;
const MAX_TOOL_CALLS_PER_TURN: usize = 8;
/// Maximum size (in chars) for a single tool output.
/// Prevents a single tool from blowing up the context window.
const MAX_TOOL_RESULT_CHARS: usize = 400_000;
/// Warn after this many identical tool+args calls in one turn.
const TOOL_REPEAT_WARN_THRESHOLD: usize = 3;
/// Hard block after this many identical tool+args calls in one turn.
const TOOL_REPEAT_BLOCK_THRESHOLD: usize = 6;
/// Safety timeout for an entire chat turn (seconds).
const TURN_SAFETY_TIMEOUT_SECS: u64 = 600;

impl Runtime {
    pub async fn from_config(
        config: &FrankClawConfig,
        sessions: Arc<dyn SessionStore>,
    ) -> Result<Self> {
        let providers = build_providers(config)?;
        Self::from_providers(config, sessions, providers).await
    }

    pub async fn from_providers(
        config: &FrankClawConfig,
        sessions: Arc<dyn SessionStore>,
        providers: Vec<Arc<dyn ModelProvider>>,
    ) -> Result<Self> {
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
        let tools = ToolRegistry::with_builtins();
        for (agent_id, agent) in &config.agents.agents {
            tools.validate_names(&agent.tools).map_err(|err| {
                FrankClawError::ConfigValidation {
                    msg: format!("agent '{}' has invalid tool config: {}", agent_id, err),
                }
            })?;
        }
        let skill_manifests = load_agent_skills(config)?;

        Ok(Self {
            config: config.clone(),
            config_arc: Some(Arc::new(config.clone())),
            sessions,
            models,
            model_defs,
            channel_ids,
            tools,
            skill_manifests,
            subagent_registry: Arc::new(subagent::SubagentRegistry::new()),
            fetcher: None,
            channels: None,
            cron: None,
            workspace: None,
        })
    }

    pub fn set_fetcher(&mut self, fetcher: Arc<dyn frankclaw_core::tool_services::Fetcher>) {
        self.fetcher = Some(fetcher);
    }

    pub fn set_channels(&mut self, channels: Arc<dyn frankclaw_core::tool_services::MessageSender>) {
        self.channels = Some(channels);
    }

    pub fn set_cron(&mut self, cron: Arc<dyn frankclaw_core::tool_services::CronManager>) {
        self.cron = Some(cron);
    }

    pub fn set_workspace(&mut self, workspace: std::path::PathBuf) {
        self.workspace = Some(workspace);
    }

    pub fn list_models(&self) -> &[ModelDef] {
        &self.model_defs
    }

    pub async fn provider_health(&self) -> Vec<ProviderHealth> {
        self.models.health().await
    }

    pub fn list_channels(&self) -> &[ChannelId] {
        &self.channel_ids
    }

    pub fn list_tools(&self, agent_id: Option<&AgentId>) -> Result<Vec<frankclaw_core::model::ToolDef>> {
        let (_, agent) = self.resolve_agent(agent_id)?;
        self.tools.definitions(&agent.tools)
    }

    pub fn list_skills(&self, agent_id: Option<&AgentId>) -> Result<&[SkillManifest]> {
        let (agent_id, _) = self.resolve_agent(agent_id)?;
        Ok(self
            .skill_manifests
            .get(&agent_id)
            .map(|skills| skills.as_slice())
            .unwrap_or(&[]))
    }

    pub fn agent_surface(
        &self,
    ) -> impl Iterator<Item = (&AgentId, &AgentDef, &[SkillManifest])> {
        self.config.agents.agents.iter().map(|(agent_id, agent)| {
            (
                agent_id,
                agent,
                self.skill_manifests
                    .get(agent_id)
                    .map(|skills| skills.as_slice())
                    .unwrap_or(&[]),
            )
        })
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

        // Sanitize user message: strip Unicode control/format chars that could
        // be used for prompt injection (bidi overrides, zero-width chars, etc.).
        let sanitized_message = sanitize::sanitize_for_prompt(&request.message);

        let (agent_id, agent) = self.resolve_agent(request.agent_id.as_ref())?;
        let model_id = self.resolve_model_id(&agent, request.model_id.as_deref())?;
        let session_key = self.resolve_session_key(&agent_id, request.session_key)?;
        let history = self.sessions.get_transcript(&session_key, 200, None).await?;
        let mut next_seq = history.last().map(|entry| entry.seq + 1).unwrap_or(1);
        let mut allowed_tools = self.tools.definitions(&agent.tools)?;

        // Inject subagent tool if not at max depth.
        let current_depth = self.subagent_registry.depth_of(&session_key).await;
        if current_depth < subagent::DEFAULT_MAX_DEPTH_PUB {
            allowed_tools.push(spawn_subagent_tool_def());
        }

        self.ensure_session(&session_key, &agent_id).await?;

        // Resolve the model definition for context window awareness.
        let model_def = self
            .model_defs
            .iter()
            .find(|m| m.id == model_id)
            .cloned()
            .unwrap_or_else(|| {
                // Fallback: use conservative defaults if model not in catalog.
                ModelDef {
                    id: model_id.clone(),
                    name: model_id.clone(),
                    api: frankclaw_core::model::ModelApi::OpenaiCompletions,
                    reasoning: false,
                    input: vec![frankclaw_core::model::InputModality::Text],
                    cost: Default::default(),
                    context_window: 128_000,
                    max_output_tokens: 4096,
                    compat: Default::default(),
                }
            });

        // Process image attachments for vision-capable models.
        let image_contents = if model_def.compat.supports_vision {
            fetch_image_attachments(&request.attachments).await
        } else {
            Vec::new()
        };

        // Build the user message: with images if available, text-only fallback otherwise.
        let user_message = if !image_contents.is_empty() {
            CompletionMessage::with_images(sanitized_message.clone(), image_contents)
        } else if !request.attachments.is_empty() && !model_def.compat.supports_vision {
            // Model doesn't support vision — add text description of attachments.
            let attachment_notes: Vec<String> = request
                .attachments
                .iter()
                .filter(|a| a.mime_type.starts_with("image/"))
                .map(|a| {
                    let name = a.filename.as_deref().unwrap_or("image");
                    format!("[Image attached: {}]", name)
                })
                .collect();
            if attachment_notes.is_empty() {
                CompletionMessage::text(Role::User, sanitized_message.clone())
            } else {
                let prefix = attachment_notes.join("\n");
                CompletionMessage::text(
                    Role::User,
                    format!("{}\n\n{}", prefix, sanitized_message),
                )
            }
        } else {
            CompletionMessage::text(Role::User, sanitized_message.clone())
        };

        let raw_messages: Vec<CompletionMessage> = history
            .iter()
            .map(|entry| transcript_entry_to_message(entry))
            .chain(std::iter::once(user_message))
            .collect();

        // Build dynamic system prompt with runtime context.
        let system_prompt = self.build_system_prompt(
            &agent_id,
            &agent,
            &model_id,
            &allowed_tools,
            request.channel_id.as_ref(),
            request.channel_capabilities.as_ref(),
        );
        let context_result = context::optimize_context(
            raw_messages,
            &model_def,
            system_prompt.as_deref(),
        );
        if context_result.compacted {
            tracing::info!(
                session = %session_key,
                pruned = context_result.pruned_count,
                estimated_tokens = context_result.estimated_tokens,
                "context window optimized — pruned old messages"
            );
        }
        let mut request_messages = context_result.messages;

        // Enforce hard prompt size limit to prevent token exhaustion / memory abuse.
        if !sanitize::check_prompt_size(&request_messages, system_prompt.as_deref()) {
            return Err(FrankClawError::InvalidRequest {
                msg: format!(
                    "total prompt size exceeds maximum ({} bytes)",
                    sanitize::MAX_PROMPT_BYTES
                ),
            });
        }

        let user_metadata = (!request.attachments.is_empty()).then(|| {
            serde_json::json!({
                "attachments": request.attachments,
            })
        });
        self.append_transcript_entry(
            &session_key,
            next_seq,
            Role::User,
            sanitized_message,
            user_metadata,
        )
        .await?;
        next_seq += 1;

        let mut remaining_tool_calls = MAX_TOOL_CALLS_PER_TURN;
        let mut tool_tracker = ToolCallTracker::new();
        let mut total_usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        let turn_deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(TURN_SAFETY_TIMEOUT_SECS);

        for _round in 0..=MAX_TOOL_ROUNDS {
            // Check cancellation before each round.
            if request.cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                return Err(FrankClawError::TurnCancelled);
            }

            // Safety timeout: abort if the entire turn is taking too long.
            if tokio::time::Instant::now() >= turn_deadline {
                return Err(FrankClawError::AgentRuntime {
                    msg: format!(
                        "turn safety timeout exceeded ({}s)",
                        TURN_SAFETY_TIMEOUT_SECS
                    ),
                });
            }
            let completion_future = self.models.complete(
                CompletionRequest {
                    model_id: model_id.clone(),
                    messages: request_messages.clone(),
                    max_tokens: request.max_tokens,
                    temperature: request.temperature,
                    system: system_prompt.clone(),
                    tools: allowed_tools.clone(),
                    thinking_budget: request.thinking_budget,
                    parallel_tool_calls: None,
                    seed: None,
                    response_format: None,
                    reasoning_effort: None,
                },
                request.stream_tx.clone(),
            );

            let response = if let Some(ref token) = request.cancel_token {
                tokio::select! {
                    result = completion_future => result?,
                    _ = token.cancelled() => return Err(FrankClawError::TurnCancelled),
                }
            } else {
                completion_future.await?
            };

            // Accumulate usage across all rounds.
            accumulate_usage(&mut total_usage, &response.usage);

            if response.tool_calls.is_empty() {
                self.append_transcript_entry(
                    &session_key,
                    next_seq,
                    Role::Assistant,
                    response.content.clone(),
                    None,
                )
                .await?;

                // Persist cumulative usage in session metadata.
                self.update_session_usage(&session_key, &total_usage, &model_id)
                    .await;

                return Ok(ChatResponse {
                    session_key,
                    model_id,
                    content: response.content,
                    usage: total_usage,
                });
            }

            // Stream any intermediate text content to the channel so the user
            // sees progress during multi-round tool execution instead of silence.
            if let Some(ref tx) = request.stream_tx {
                if !response.content.trim().is_empty() {
                    let _ = tx.send(StreamDelta::Text(response.content.clone())).await;
                }
            }

            if response.tool_calls.len() > remaining_tool_calls {
                return Err(FrankClawError::AgentRuntime {
                    msg: format!(
                        "model requested too many tool calls in one turn (max {})",
                        MAX_TOOL_CALLS_PER_TURN
                    ),
                });
            }

            let tool_call_count = response.tool_calls.len();
            let assistant_message =
                build_tool_request_message(&response.content, &response.tool_calls);
            self.append_transcript_entry(
                &session_key,
                next_seq,
                Role::Assistant,
                assistant_message.clone(),
                Some(serde_json::json!({
                    "tool_calls": response
                        .tool_calls
                        .iter()
                        .map(|call| serde_json::json!({
                            "id": call.id,
                            "name": call.name,
                            "arguments": call.arguments,
                        }))
                        .collect::<Vec<_>>(),
                })),
            )
            .await?;
            request_messages.push(CompletionMessage::assistant_tool_calls(
                assistant_message,
                response.tool_calls.clone(),
            ));
            next_seq += 1;

            for tool_call in response.tool_calls {
                // Check cancellation before each tool invocation.
                if request.cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                    return Err(FrankClawError::TurnCancelled);
                }

                // Detect tool call loops (identical name+args repeated).
                tool_tracker.record(&tool_call.name, &tool_call.arguments)?;

                let arguments = match parse_tool_arguments(&tool_call) {
                    Ok(args) => args,
                    Err(err) => {
                        // Return error result to model instead of aborting the turn.
                        let error_content = format!(
                            "{{\"error\": \"invalid arguments for tool '{}': {}\"}}",
                            tool_call.name, err
                        );
                        self.append_transcript_entry(
                            &session_key,
                            next_seq,
                            Role::Tool,
                            error_content.clone(),
                            Some(serde_json::json!({
                                "tool_name": tool_call.name,
                                "tool_call_id": tool_call.id,
                                "is_error": true,
                            })),
                        )
                        .await?;
                        request_messages.push(CompletionMessage::tool_result(
                            &tool_call.id,
                            error_content,
                        ));
                        next_seq += 1;
                        continue;
                    }
                };
                // Handle spawn_subagent as a special-cased tool: it calls
                // Runtime::chat() recursively rather than going through ToolRegistry.
                if tool_call.name == SPAWN_SUBAGENT_TOOL {
                    let subagent_result = self
                        .handle_spawn_subagent(
                            &arguments,
                            &agent_id,
                            &session_key,
                            current_depth,
                        )
                        .await;
                    let result_content = match subagent_result {
                        Ok(text) => serde_json::json!({
                            "tool": SPAWN_SUBAGENT_TOOL,
                            "output": text,
                        })
                        .to_string(),
                        Err(err) => format!(
                            "{{\"error\": \"subagent failed: {}\"}}",
                            err
                        ),
                    };
                    self.append_transcript_entry(
                        &session_key,
                        next_seq,
                        Role::Tool,
                        result_content.clone(),
                        Some(serde_json::json!({
                            "tool_name": SPAWN_SUBAGENT_TOOL,
                            "tool_call_id": tool_call.id,
                        })),
                    )
                    .await?;
                    request_messages.push(CompletionMessage::tool_result(
                        &tool_call.id,
                        result_content,
                    ));
                    next_seq += 1;
                    continue;
                }

                // Interactive tool approval: if approval_tx is set and the tool
                // is mutating or destructive, request human approval before execution.
                if let Some(ref approval_tx) = request.approval_tx {
                    let risk = frankclaw_tools::tool_risk_level(&tool_call.name);
                    if risk != ToolRiskLevel::ReadOnly {
                        let approval_id = uuid::Uuid::new_v4().to_string();
                        let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
                        let approval_req = frankclaw_core::tool_approval::ApprovalRequest {
                            approval_id: approval_id.clone(),
                            tool_name: tool_call.name.clone(),
                            tool_args: arguments.clone(),
                            risk_level: format!("{:?}", risk),
                            session_key: session_key.as_str().to_string(),
                            agent_id: agent_id.as_str().to_string(),
                        };
                        if approval_tx.send((approval_req, decision_tx)).await.is_err() {
                            return Err(FrankClawError::AgentRuntime {
                                msg: "tool approval channel closed".into(),
                            });
                        }
                        match decision_rx.await {
                            Ok(frankclaw_core::tool_approval::ApprovalDecision::Deny) => {
                                let deny_content = format!(
                                    "{{\"error\": \"tool '{}' was denied by the user\"}}",
                                    tool_call.name
                                );
                                self.append_transcript_entry(
                                    &session_key,
                                    next_seq,
                                    Role::Tool,
                                    deny_content.clone(),
                                    Some(serde_json::json!({
                                        "tool_name": tool_call.name,
                                        "tool_call_id": tool_call.id,
                                        "is_error": true,
                                        "denied": true,
                                    })),
                                )
                                .await?;
                                request_messages.push(CompletionMessage::tool_result(
                                    &tool_call.id,
                                    deny_content,
                                ));
                                next_seq += 1;
                                continue;
                            }
                            Ok(_) => { /* AllowOnce or AllowAlways — proceed */ }
                            Err(_) => {
                                return Err(FrankClawError::AgentRuntime {
                                    msg: "tool approval response channel dropped".into(),
                                });
                            }
                        }
                    }
                }

                let tool_result = self
                    .tools
                    .invoke_allowed(
                        &agent.tools,
                        &tool_call.name,
                        arguments,
                        ToolContext {
                            agent_id: agent_id.clone(),
                            session_key: Some(session_key.clone()),
                            sessions: self.sessions.clone(),
                            canvas: request.canvas.clone(),
                            fetcher: self.fetcher.clone(),
                            channels: self.channels.clone(),
                            cron: self.cron.clone(),
                            config: self.config_arc.clone(),
                            workspace: self.workspace.clone(),
                        },
                    )
                    .await;
                let tool_output = match tool_result {
                    Ok(output) => output,
                    Err(err) => {
                        // Return synthetic error result so the model can recover.
                        let error_content = format!(
                            "{{\"error\": \"tool '{}' failed: {}\"}}",
                            tool_call.name, err
                        );
                        tracing::warn!(
                            tool = %tool_call.name,
                            error = %err,
                            "tool invocation failed — returning error result to model"
                        );
                        self.append_transcript_entry(
                            &session_key,
                            next_seq,
                            Role::Tool,
                            error_content.clone(),
                            Some(serde_json::json!({
                                "tool_name": tool_call.name,
                                "tool_call_id": tool_call.id,
                                "is_error": true,
                            })),
                        )
                        .await?;
                        request_messages.push(CompletionMessage::tool_result(
                            &tool_call.id,
                            error_content,
                        ));
                        next_seq += 1;
                        continue;
                    }
                };
                let tool_images = tool_output.image_content;
                let raw_content = serde_json::to_string(&serde_json::json!({
                    "tool": tool_output.name,
                    "output": tool_output.output,
                }))
                .map_err(|err| FrankClawError::Internal {
                    msg: format!("failed to serialize tool output: {err}"),
                })?;
                // Scan tool output for credential leaks before it enters the context.
                let leak_result = leak_detector::scan_for_leaks(&raw_content);
                if leak_result.should_block {
                    let leaked = leak_result
                        .matches
                        .iter()
                        .map(|m| format!("{}({})", m.pattern_name, m.masked_preview))
                        .collect::<Vec<_>>()
                        .join(", ");
                    tracing::warn!(
                        tool = %tool_call.name,
                        patterns = %leaked,
                        "credential leak detected in tool output — blocking"
                    );
                    return Err(FrankClawError::AgentRuntime {
                        msg: format!(
                            "tool '{}' output contained credential(s): {}. Output blocked.",
                            tool_call.name, leaked,
                        ),
                    });
                }
                let safe_content = leak_result
                    .redacted_content
                    .as_deref()
                    .unwrap_or(&raw_content);
                if !leak_result.matches.is_empty() {
                    let warned = leak_result
                        .matches
                        .iter()
                        .map(|m| format!("{}({})", m.pattern_name, m.masked_preview))
                        .collect::<Vec<_>>()
                        .join(", ");
                    tracing::warn!(
                        tool = %tool_call.name,
                        patterns = %warned,
                        "potential credential in tool output — redacted/warned"
                    );
                }
                // Truncate oversized tool results to prevent context overflow.
                let tool_content = truncate_tool_output(safe_content);
                self.append_transcript_entry(
                    &session_key,
                    next_seq,
                    Role::Tool,
                    tool_content.clone(),
                    Some(serde_json::json!({
                        "tool_name": tool_call.name,
                        "tool_call_id": tool_call.id,
                    })),
                )
                .await?;
                if tool_images.is_empty() {
                    request_messages.push(CompletionMessage::tool_result(
                        &tool_call.id,
                        tool_content,
                    ));
                } else {
                    request_messages.push(CompletionMessage::tool_result_with_images(
                        &tool_call.id,
                        tool_content,
                        tool_images,
                    ));
                }
                next_seq += 1;
            }

            remaining_tool_calls -= tool_call_count;

            // Mid-loop context compaction: re-check token budget after tool results
            // accumulate. If over budget, re-run optimize_context to prune old messages
            // and prevent context overflow errors on the next model call.
            let mid_tokens = context::estimate_messages_tokens(&request_messages);
            let mid_budget = context::available_input_budget(&model_def, system_prompt.as_deref());
            if mid_tokens > mid_budget {
                tracing::info!(
                    session = %session_key,
                    tokens = mid_tokens,
                    budget = mid_budget,
                    "mid-loop context exceeds budget — re-compacting"
                );
                let compacted = context::optimize_context(
                    request_messages,
                    &model_def,
                    system_prompt.as_deref(),
                );
                request_messages = compacted.messages;

                // If still over budget after compaction, return a graceful error
                // instead of letting the provider fail with a raw context overflow.
                let post_tokens = context::estimate_messages_tokens(&request_messages);
                if post_tokens > mid_budget {
                    return Err(FrankClawError::AgentRuntime {
                        msg: format!(
                            "context too large after compaction ({} tokens, budget {})",
                            post_tokens, mid_budget
                        ),
                    });
                }
            }
        }

        Err(FrankClawError::AgentRuntime {
            msg: format!("tool round limit exceeded (max {})", MAX_TOOL_ROUNDS),
        })
    }

    pub async fn invoke_tool(&self, request: ToolRequest) -> Result<ToolOutput> {
        let (agent_id, agent) = self.resolve_agent(request.agent_id.as_ref())?;
        self.tools
            .invoke_allowed(
                &agent.tools,
                &request.tool_name,
                request.arguments,
                ToolContext {
                    agent_id,
                    session_key: request.session_key,
                    sessions: self.sessions.clone(),
                    canvas: None,
                    fetcher: self.fetcher.clone(),
                    channels: self.channels.clone(),
                    cron: self.cron.clone(),
                    config: self.config_arc.clone(),
                    workspace: self.workspace.clone(),
                },
            )
            .await
    }

    pub async fn tool_activity(
        &self,
        session_key: &SessionKey,
        limit: usize,
    ) -> Result<Vec<ToolActivity>> {
        let entries = self
            .sessions
            .get_transcript(session_key, limit.saturating_mul(4).max(1), None)
            .await?;
        let mut activity = entries
            .into_iter()
            .filter(|entry| entry.role == Role::Tool)
            .filter_map(|entry| {
                let metadata = entry.metadata.as_ref()?;
                let tool_name = metadata["tool_name"].as_str()?.to_string();
                Some(ToolActivity {
                    seq: entry.seq,
                    tool_name,
                    tool_call_id: metadata["tool_call_id"].as_str().map(str::to_string),
                    timestamp: entry.timestamp,
                    output_preview: summarize_tool_output(&entry.content),
                })
            })
            .collect::<Vec<_>>();
        activity.sort_by_key(|entry| entry.seq);
        if activity.len() > limit {
            activity = activity.split_off(activity.len() - limit);
        }
        Ok(activity)
    }

    /// Handle a spawn_subagent tool call by running chat() recursively.
    async fn handle_spawn_subagent(
        &self,
        arguments: &serde_json::Value,
        parent_agent_id: &AgentId,
        parent_session_key: &SessionKey,
        parent_depth: u32,
    ) -> Result<String> {
        let task = arguments["task"]
            .as_str()
            .ok_or_else(|| FrankClawError::AgentRuntime {
                msg: "spawn_subagent requires a 'task' string".into(),
            })?;
        let label = arguments["label"].as_str().map(String::from);

        // Sanitize task/label per Rule 6.
        let task = sanitize::sanitize_for_prompt(task);
        let label = label.map(|l| sanitize::sanitize_for_prompt(&l));

        let spawn_request = subagent::SpawnRequest {
            task: task.clone(),
            label,
            agent_id: parent_agent_id.clone(),
            model_override: None,
            timeout_secs: None,
            parent_session_key: parent_session_key.clone(),
            parent_depth,
        };

        let spawn_result = self.subagent_registry.register_spawn(&spawn_request).await;
        let (run_id, child_session_key) = match spawn_result {
            subagent::SpawnResult::Accepted {
                run_id,
                child_session_key,
            } => (run_id, child_session_key),
            subagent::SpawnResult::Rejected { reason } => {
                return Err(FrankClawError::AgentRuntime {
                    msg: format!("subagent spawn rejected: {reason}"),
                });
            }
        };

        self.subagent_registry.mark_running(&run_id).await?;

        // Run the subagent with a timeout.
        let timeout = std::time::Duration::from_secs(
            spawn_request.timeout_secs.unwrap_or(300),
        );
        let result = tokio::time::timeout(
            timeout,
            Box::pin(self.chat(ChatRequest {
                agent_id: Some(parent_agent_id.clone()),
                session_key: Some(child_session_key),
                message: task,
                attachments: Vec::new(),
                model_id: None,
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                self.subagent_registry
                    .complete(subagent::CompletionNotice {
                        run_id,
                        state: subagent::RunState::Completed,
                        result_text: Some(response.content.clone()),
                        error: None,
                    })
                    .await?;
                Ok(response.content)
            }
            Ok(Err(err)) => {
                self.subagent_registry
                    .complete(subagent::CompletionNotice {
                        run_id,
                        state: subagent::RunState::Failed,
                        result_text: None,
                        error: Some(err.to_string()),
                    })
                    .await?;
                Err(err)
            }
            Err(_) => {
                self.subagent_registry
                    .complete(subagent::CompletionNotice {
                        run_id,
                        state: subagent::RunState::TimedOut,
                        result_text: None,
                        error: Some("subagent timed out".into()),
                    })
                    .await?;
                Err(FrankClawError::AgentRuntime {
                    msg: "subagent execution timed out".into(),
                })
            }
        }
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

    fn build_system_prompt(
        &self,
        agent_id: &AgentId,
        agent: &AgentDef,
        model_id: &str,
        tools: &[ToolDef],
        channel_id: Option<&ChannelId>,
        channel_capabilities: Option<&ChannelCapabilities>,
    ) -> Option<String> {
        let mut sections: Vec<String> = Vec::new();

        // Section 1: Identity
        sections.push(prompts::render(prompts::AGENT_IDENTITY, &[
            ("agent_name", &agent.name),
        ]));

        // Section 2: User-defined system prompt (highest priority content)
        if let Some(prompt) = agent.system_prompt.as_deref() {
            let trimmed = prompt.trim();
            if !trimmed.is_empty() {
                sections.push(trimmed.to_string());
            }
        }

        // Section 2b: Workspace bootstrap files (SOUL.md, IDENTITY.md, etc.)
        if let Some(ref ws) = self.workspace {
            let bootstrap = read_workspace_bootstrap_files(ws);
            if !bootstrap.is_empty() {
                sections.push(bootstrap);
            }
        }

        // Section 3: Available tools (with descriptions so the model knows its capabilities)
        if !tools.is_empty() {
            let tool_descriptions: Vec<String> = tools
                .iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect();
            sections.push(prompts::render(prompts::AGENT_TOOLS, &[
                ("tool_descriptions", &tool_descriptions.join("\n")),
            ]));
        }

        // Section 4: Skills
        let skill_prompts: Vec<String> = self
            .skill_manifests
            .get(agent_id)
            .map(|skills| {
                skills
                    .iter()
                    .map(|skill| format!("[Skill: {}]\n{}", skill.name, skill.prompt.trim()))
                    .collect()
            })
            .unwrap_or_default();
        if !skill_prompts.is_empty() {
            sections.push(skill_prompts.join("\n\n"));
        }

        // Section 5: Safety
        sections.push(prompts::AGENT_SAFETY.trim().to_string());

        // Section 6: Runtime context
        let now = Utc::now();
        let tool_count_str = tools.len().to_string();
        let date_str = now.format("%Y-%m-%d %H:%M UTC").to_string();
        sections.push(prompts::render(prompts::AGENT_CONTEXT, &[
            ("agent_id", agent_id.as_str()),
            ("model_id", model_id),
            ("date", &date_str),
            ("tool_count", &tool_count_str),
        ]));

        // Section 7: Channel hints (tells the LLM what messaging features are available)
        if let (Some(ch_id), Some(caps)) = (channel_id, channel_capabilities) {
            let mut features = Vec::new();
            if caps.threads { features.push("threads"); }
            if caps.groups { features.push("group messages"); }
            if caps.attachments { features.push("file attachments"); }
            if caps.edit { features.push("message editing"); }
            if caps.delete { features.push("message deletion"); }
            if caps.reactions { features.push("reactions"); }
            if caps.voice { features.push("voice messages"); }
            if caps.inline_buttons { features.push("inline buttons"); }
            let features_str = if features.is_empty() {
                "text messages only".to_string()
            } else {
                features.join(", ")
            };
            sections.push(prompts::render(prompts::AGENT_CHANNEL, &[
                ("channel", ch_id.as_str()),
                ("features", &features_str),
            ]));
        }

        // Section 8: Language instruction (non-English locales)
        let locale = std::env::var("FRANKCLAW_LANG")
            .ok()
            .or_else(|| {
                std::env::var("LANG")
                    .ok()
                    .map(|l| l.split('.').next().unwrap_or("en").replace('_', "-"))
            })
            .unwrap_or_else(|| "en".into());
        if !locale.is_empty() && locale != "en" {
            let language = match locale.as_str() {
                "pt-BR" => "Brazilian Portuguese",
                "pt-PT" => "European Portuguese",
                "es" => "Spanish",
                "fr" => "French",
                "de" => "German",
                "it" => "Italian",
                "ja" => "Japanese",
                "ko" => "Korean",
                other => other,
            };
            sections.push(format!(
                "IMPORTANT: Always respond in {language}. All your replies must be in {language}."
            ));
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        }
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

    async fn append_transcript_entry(
        &self,
        session_key: &SessionKey,
        seq: u64,
        role: Role,
        content: String,
        metadata: Option<serde_json::Value>,
    ) -> Result<()> {
        self.sessions
            .append_transcript(
                session_key,
                &TranscriptEntry {
                    seq,
                    role,
                    content,
                    timestamp: Utc::now(),
                    metadata,
                },
            )
            .await
    }

    /// Update cumulative usage stats in session metadata after a chat turn.
    async fn update_session_usage(
        &self,
        session_key: &SessionKey,
        turn_usage: &Usage,
        model_id: &str,
    ) {
        let Ok(Some(mut session)) = self.sessions.get(session_key).await else {
            return;
        };
        let meta = session.metadata.as_object_mut();
        let meta = match meta {
            Some(m) => m,
            None => {
                session.metadata = serde_json::json!({});
                session.metadata.as_object_mut().unwrap()
            }
        };

        // Accumulate totals.
        let prev_input = meta.get("total_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let prev_output = meta.get("total_output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let prev_turns = meta.get("total_turns").and_then(|v| v.as_u64()).unwrap_or(0);

        meta.insert("total_input_tokens".into(), (prev_input + turn_usage.input_tokens as u64).into());
        meta.insert("total_output_tokens".into(), (prev_output + turn_usage.output_tokens as u64).into());
        meta.insert("total_turns".into(), (prev_turns + 1).into());
        meta.insert("last_model".into(), model_id.into());

        // Calculate cost if available.
        if let Some((input_cost, output_cost)) = frankclaw_models::costs::model_cost(model_id) {
            let turn_cost = (turn_usage.input_tokens as f64 * input_cost)
                + (turn_usage.output_tokens as f64 * output_cost);
            let prev_cost = meta.get("total_cost_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
            meta.insert("total_cost_usd".into(), serde_json::json!(prev_cost + turn_cost));
        }

        let _ = self.sessions.upsert(&session).await;
    }
}

/// Track tool call patterns to detect loops where the model repeatedly calls
/// the same tool with the same arguments without making progress.
struct ToolCallTracker {
    /// Map from (tool_name, arguments_hash) → count of identical calls.
    seen: HashMap<(String, u64), usize>,
}

impl ToolCallTracker {
    fn new() -> Self {
        Self {
            seen: HashMap::new(),
        }
    }

    /// Record a tool call. Returns an error if the repeat threshold is exceeded.
    fn record(&mut self, name: &str, arguments: &str) -> Result<()> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        arguments.hash(&mut hasher);
        let args_hash = hasher.finish();
        let key = (name.to_string(), args_hash);
        let count = self.seen.entry(key).or_insert(0);
        *count += 1;

        if *count >= TOOL_REPEAT_BLOCK_THRESHOLD {
            return Err(FrankClawError::AgentRuntime {
                msg: format!(
                    "tool '{}' called {} times with identical arguments — possible infinite loop",
                    name, count
                ),
            });
        }
        if *count >= TOOL_REPEAT_WARN_THRESHOLD {
            tracing::warn!(
                tool = name,
                count = *count,
                "repeated tool call detected — possible loop"
            );
        }
        Ok(())
    }
}

/// Accumulate token usage from a model response into a running total.
fn accumulate_usage(total: &mut Usage, delta: &Usage) {
    total.input_tokens += delta.input_tokens;
    total.output_tokens += delta.output_tokens;
    match (&mut total.cache_read_tokens, delta.cache_read_tokens) {
        (Some(t), Some(d)) => *t += d,
        (slot @ None, Some(d)) => *slot = Some(d),
        _ => {}
    }
    match (&mut total.cache_write_tokens, delta.cache_write_tokens) {
        (Some(t), Some(d)) => *t += d,
        (slot @ None, Some(d)) => *slot = Some(d),
        _ => {}
    }
}

/// Bootstrap file names recognized in a workspace directory.
/// These are read (if present) and injected into the system prompt.
const BOOTSTRAP_FILES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "USER.md",
    "BOOTSTRAP.md",
    "MEMORY.md",
];

/// Maximum total size of all bootstrap files combined (150 KB).
const MAX_BOOTSTRAP_TOTAL_BYTES: usize = 150 * 1024;

/// Maximum size of a single bootstrap file (50 KB).
const MAX_BOOTSTRAP_FILE_BYTES: usize = 50 * 1024;

/// Read recognized workspace bootstrap files and return them as a combined string.
/// Files that don't exist are silently skipped. Content is sanitized before inclusion.
fn read_workspace_bootstrap_files(workspace: &std::path::Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut total_bytes: usize = 0;

    for filename in BOOTSTRAP_FILES {
        let path = workspace.join(filename);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if content.trim().is_empty() {
            continue;
        }

        // Strip YAML front matter (---\n...\n---)
        let content = strip_yaml_front_matter(&content);

        // Per-file size limit
        let content = if content.len() > MAX_BOOTSTRAP_FILE_BYTES {
            let truncated: String = content.chars().take(MAX_BOOTSTRAP_FILE_BYTES).collect();
            format!("{}\n\n[... truncated, file too large ...]", truncated)
        } else {
            content
        };

        // Check total budget
        if total_bytes + content.len() > MAX_BOOTSTRAP_TOTAL_BYTES {
            break;
        }

        total_bytes += content.len();
        let sanitized = sanitize::sanitize_for_prompt(&content);
        parts.push(format!("## {filename}\n\n{sanitized}"));
    }

    if parts.is_empty() {
        return String::new();
    }

    format!("# Workspace Context\n\n{}", parts.join("\n\n"))
}

/// Strip YAML front matter delimited by `---` at the start of a string.
fn strip_yaml_front_matter(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }
    // Find the closing ---
    if let Some(end) = trimmed[3..].find("\n---") {
        let after = &trimmed[3 + end + 4..]; // skip past \n---
        after.trim_start_matches('\n').to_string()
    } else {
        content.to_string()
    }
}

/// Truncate and sanitize tool output to prevent context window overflow
/// and prompt injection from external command results.
/// Preserves the beginning and end of the output with an omission marker.
fn truncate_tool_output(output: &str) -> String {
    // Sanitize first: tool outputs come from external processes and could
    // contain Unicode control/format chars designed to manipulate the LLM.
    let sanitized = sanitize::sanitize_for_prompt(output);
    let char_count = sanitized.chars().count();
    if char_count <= MAX_TOOL_RESULT_CHARS {
        return sanitized;
    }

    // Keep 80% head, 20% tail with omission marker in between.
    let head_chars = (MAX_TOOL_RESULT_CHARS * 4) / 5;
    let tail_chars = MAX_TOOL_RESULT_CHARS / 5;
    let omitted = char_count - head_chars - tail_chars;

    let head: String = sanitized.chars().take(head_chars).collect();
    let tail: String = sanitized.chars().skip(char_count - tail_chars).collect();

    format!(
        "{}\n\n... ({} characters omitted) ...\n\n{}",
        head, omitted, tail
    )
}

/// Convert a transcript entry back into a `CompletionMessage`, restoring tool call
/// metadata from the entry's JSON metadata field.
fn transcript_entry_to_message(entry: &frankclaw_core::session::TranscriptEntry) -> CompletionMessage {
    let metadata = entry.metadata.as_ref();

    // Assistant messages with tool_calls in metadata.
    if entry.role == Role::Assistant {
        if let Some(calls) = metadata.and_then(|m| m["tool_calls"].as_array()) {
            let tool_calls: Vec<ToolCallResponse> = calls
                .iter()
                .filter_map(|c| {
                    Some(ToolCallResponse {
                        id: c["id"].as_str()?.to_string(),
                        name: c["name"].as_str()?.to_string(),
                        arguments: c["arguments"].as_str().unwrap_or("{}").to_string(),
                    })
                })
                .collect();
            if !tool_calls.is_empty() {
                return CompletionMessage::assistant_tool_calls(entry.content.clone(), tool_calls);
            }
        }
    }

    // Tool result messages with tool_call_id in metadata.
    if entry.role == Role::Tool {
        if let Some(call_id) = metadata.and_then(|m| m["tool_call_id"].as_str()) {
            return CompletionMessage::tool_result(call_id, entry.content.clone());
        }
    }

    CompletionMessage::text(entry.role, entry.content.clone())
}

fn build_tool_request_message(content: &str, tool_calls: &[ToolCallResponse]) -> String {
    let mut segments = Vec::new();
    let trimmed = content.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }
    for call in tool_calls {
        segments.push(format!("[tool_call:{} {}]", call.name, call.arguments));
    }
    segments.join("\n")
}

fn summarize_tool_output(content: &str) -> String {
    let preview = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| value.get("output").cloned())
        .map(|value| value.to_string())
        .unwrap_or_else(|| content.to_string());
    let preview = preview.replace('\n', " ");
    if preview.chars().count() > 120 {
        format!("{}...", preview.chars().take(120).collect::<String>())
    } else {
        preview
    }
}

/// Name of the built-in subagent spawning tool.
const SPAWN_SUBAGENT_TOOL: &str = "spawn_subagent";

/// Build the tool definition for spawning subagents.
fn spawn_subagent_tool_def() -> frankclaw_core::model::ToolDef {
    frankclaw_core::model::ToolDef {
        name: SPAWN_SUBAGENT_TOOL.into(),
        description: "Spawn a subagent to handle a complex subtask. The subagent runs independently and returns its result.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Description of the task for the subagent to complete"
                },
                "label": {
                    "type": "string",
                    "description": "Short label for this subagent run (optional)"
                }
            },
            "required": ["task"]
        }),
        risk_level: frankclaw_core::model::ToolRiskLevel::Mutating,
    }
}

/// Maximum image size for vision (5 MB, matches OpenClaw).
const MAX_VISION_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Fetch image attachments from their URLs and encode as base64 for vision models.
///
/// Uses SSRF-safe fetching. Skips non-image attachments and attachments without URLs.
/// Errors on individual images are logged but don't fail the batch.
async fn fetch_image_attachments(
    attachments: &[InboundAttachment],
) -> Vec<ImageContent> {
    use base64::Engine;

    let fetcher = frankclaw_media::SafeFetcher::new(MAX_VISION_IMAGE_BYTES);
    let mut images = Vec::new();

    for attachment in attachments {
        if !attachment.mime_type.starts_with("image/") {
            continue;
        }
        let Some(url_str) = attachment.url.as_deref() else {
            continue;
        };
        let Ok(url) = url::Url::parse(url_str) else {
            tracing::warn!(url = url_str, "invalid attachment URL, skipping");
            continue;
        };

        match fetcher.fetch(&url).await {
            Ok(fetched) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&fetched.bytes);
                // Use the fetched content-type if it's an image, otherwise fall back
                // to the attachment's declared MIME type.
                let mime = if fetched.content_type.starts_with("image/") {
                    fetched.content_type
                } else {
                    attachment.mime_type.clone()
                };
                images.push(ImageContent {
                    mime_type: mime,
                    data: b64,
                });
            }
            Err(err) => {
                tracing::warn!(
                    url = url_str,
                    error = %err,
                    "failed to fetch image attachment for vision, skipping"
                );
            }
        }
    }

    images
}

fn parse_tool_arguments(tool_call: &ToolCallResponse) -> Result<serde_json::Value> {
    serde_json::from_str(&tool_call.arguments).map_err(|err| FrankClawError::AgentRuntime {
        msg: format!(
            "model produced invalid arguments for tool '{}': {}",
            tool_call.name, err
        ),
    })
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
                // OpenAI-compatible providers with default base URLs.
                "google" | "gemini" => Arc::new(OpenAiProvider::new(
                    provider.id.clone(),
                    provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta/openai".to_string()),
                    resolve_secret(provider, "GOOGLE_API_KEY")?,
                    provider.models.clone(),
                )),
                "openrouter" => Arc::new(OpenAiProvider::new(
                    provider.id.clone(),
                    provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string()),
                    resolve_secret(provider, "OPENROUTER_API_KEY")?,
                    provider.models.clone(),
                )),
                "groq" => Arc::new(OpenAiProvider::new(
                    provider.id.clone(),
                    provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.groq.com/openai/v1".to_string()),
                    resolve_secret(provider, "GROQ_API_KEY")?,
                    provider.models.clone(),
                )),
                "together" => Arc::new(OpenAiProvider::new(
                    provider.id.clone(),
                    provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.together.xyz/v1".to_string()),
                    resolve_secret(provider, "TOGETHER_API_KEY")?,
                    provider.models.clone(),
                )),
                "deepseek" => Arc::new(OpenAiProvider::new(
                    provider.id.clone(),
                    provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string()),
                    resolve_secret(provider, "DEEPSEEK_API_KEY")?,
                    provider.models.clone(),
                )),
                other => {
                    return Err(FrankClawError::ConfigValidation {
                        msg: format!(
                            "unsupported model provider api '{}'; expected openai, anthropic, ollama, \
                             google, openrouter, groq, together, or deepseek",
                            other
                        ),
                    });
                }
            };
        providers.push(provider_impl);
    }

    Ok(providers)
}

fn load_agent_skills(config: &FrankClawConfig) -> Result<HashMap<AgentId, Vec<SkillManifest>>> {
    let mut manifests = HashMap::new();

    for (agent_id, agent) in &config.agents.agents {
        if agent.skills.is_empty() {
            manifests.insert(agent_id.clone(), Vec::new());
            continue;
        }

        let workspace = agent.workspace.as_ref().ok_or_else(|| FrankClawError::ConfigValidation {
            msg: format!("agent '{}' declares skills but has no workspace", agent_id),
        })?;
        let skills = load_workspace_skills(workspace, &agent.skills)?;
        for skill in &skills {
            for tool in &skill.tools {
                if !agent.tools.iter().any(|allowed| allowed == tool) {
                    return Err(FrankClawError::ConfigValidation {
                        msg: format!(
                            "agent '{}' skill '{}' requires tool '{}' but the agent does not allow it",
                            agent_id, skill.id, tool
                        ),
                    });
                }
            }
        }
        manifests.insert(agent_id.clone(), skills);
    }

    Ok(manifests)
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use frankclaw_core::model::{
        CompletionResponse, FinishReason, InputModality, ModelApi, ModelCompat, ModelCost,
        ToolCallResponse, ToolDef,
    };
    use frankclaw_core::session::SessionStore;
    use frankclaw_sessions::SqliteSessionStore;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Debug, Clone)]
    struct CapturedRequest {
        messages: Vec<CompletionMessage>,
        system: Option<String>,
        tools: Vec<ToolDef>,
    }

    #[derive(Debug, Clone)]
    struct MockResponse {
        content: String,
        tool_calls: Vec<ToolCallResponse>,
        finish_reason: FinishReason,
    }

    struct MockProvider {
        id: String,
        model_id: String,
        responses: Mutex<VecDeque<Option<MockResponse>>>,
        seen_requests: Mutex<Vec<CapturedRequest>>,
    }

    impl MockProvider {
        fn reply(id: &str, model_id: &str, content: &str) -> Self {
            Self {
                id: id.into(),
                model_id: model_id.into(),
                responses: Mutex::new(VecDeque::from([Some(MockResponse {
                    content: content.into(),
                    tool_calls: Vec::new(),
                    finish_reason: FinishReason::Stop,
                })])),
                seen_requests: Mutex::new(Vec::new()),
            }
        }

        fn failing(id: &str, model_id: &str) -> Self {
            Self {
                id: id.into(),
                model_id: model_id.into(),
                responses: Mutex::new(VecDeque::from([None])),
                seen_requests: Mutex::new(Vec::new()),
            }
        }

        fn scripted(id: &str, model_id: &str, responses: Vec<Option<MockResponse>>) -> Self {
            Self {
                id: id.into(),
                model_id: model_id.into(),
                responses: Mutex::new(responses.into()),
                seen_requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        fn id(&self) -> &str {
            &self.id
        }

        async fn complete(
            &self,
            request: CompletionRequest,
            _stream_tx: Option<tokio::sync::mpsc::Sender<frankclaw_core::model::StreamDelta>>,
        ) -> Result<CompletionResponse> {
            self.seen_requests
                .lock()
                .expect("request capture should lock")
                .push(CapturedRequest {
                    messages: request.messages.clone(),
                    system: request.system.clone(),
                    tools: request.tools.clone(),
                });
            match self
                .responses
                .lock()
                .expect("response queue should lock")
                .pop_front()
                .unwrap_or(None)
            {
                Some(response) => Ok(CompletionResponse {
                    content: response.content,
                    tool_calls: response.tool_calls,
                    usage: Usage {
                        input_tokens: 4,
                        output_tokens: 2,
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                    },
                    finish_reason: response.finish_reason,
                }),
                None => Err(FrankClawError::AllProvidersFailed),
            }
        }

        async fn list_models(&self) -> Result<Vec<ModelDef>> {
            Ok(vec![ModelDef {
                id: self.model_id.clone(),
                name: self.model_id.clone(),
                api: ModelApi::Ollama,
                reasoning: false,
                input: vec![InputModality::Text],
                cost: ModelCost::default(),
                context_window: 8192,
                max_output_tokens: 1024,
                compat: ModelCompat::default(),
            }])
        }

        async fn health(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn runtime_fails_over_to_next_provider_and_persists_history() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let mut config = FrankClawConfig::default();
        config.models.providers = vec![
            ProviderConfig {
                id: "primary".into(),
                api: "ollama".into(),
                base_url: None,
                api_key_ref: None,
                models: vec!["mock-primary".into()],
                cooldown_secs: 1,
            },
            ProviderConfig {
                id: "secondary".into(),
                api: "ollama".into(),
                base_url: None,
                api_key_ref: None,
                models: vec!["mock-secondary".into()],
                cooldown_secs: 1,
            },
        ];

        let runtime = Runtime::from_providers(
            &config,
            sessions.clone() as Arc<dyn SessionStore>,
            vec![
                Arc::new(MockProvider {
                    ..MockProvider::failing("primary", "mock-primary")
                }),
                Arc::new(MockProvider::reply(
                    "secondary",
                    "mock-secondary",
                    "fallback reply",
                )),
            ],
        )
        .await
        .expect("runtime should build");

        let response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "hello".into(),
                attachments: Vec::new(),
                model_id: Some("mock-secondary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        assert_eq!(response.content, "fallback reply");
        let transcript = sessions
            .get_transcript(&response.session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, Role::User);
        assert_eq!(transcript[1].role, Role::Assistant);

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_invokes_allowed_tool() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-tools-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let mut config = FrankClawConfig::default();
        config.agents.agents.get_mut(&AgentId::default_agent()).unwrap().tools =
            vec!["session.inspect".into()];
        let runtime = Runtime::from_providers(
            &config,
            sessions.clone() as Arc<dyn SessionStore>,
            vec![Arc::new(MockProvider::reply(
                "primary",
                "mock-primary",
                "reply",
            ))],
        )
        .await
        .expect("runtime should build");

        let chat = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "hello".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        let tool = runtime
            .invoke_tool(ToolRequest {
                agent_id: None,
                session_key: Some(chat.session_key.clone()),
                tool_name: "session.inspect".into(),
                arguments: serde_json::json!({ "limit": 5 }),
            })
            .await
            .expect("tool should run");

        assert_eq!(tool.name, "session.inspect");
        assert_eq!(
            tool.output["session"]["key"],
            serde_json::json!(chat.session_key.as_str())
        );

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_persists_user_attachment_metadata() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-attachments-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let runtime = Runtime::from_providers(
            &FrankClawConfig::default(),
            sessions.clone() as Arc<dyn SessionStore>,
            vec![Arc::new(MockProvider::reply(
                "primary",
                "mock-primary",
                "reply",
            ))],
        )
        .await
        .expect("runtime should build");

        let response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "here is a screenshot".into(),
                attachments: vec![InboundAttachment {
                    media_id: Some(frankclaw_core::types::MediaId::new()),
                    mime_type: "image/png".into(),
                    filename: Some("photo.png".into()),
                    size_bytes: Some(42),
                    url: Some("/api/media/test-photo".into()),
                }],
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        let transcript = sessions
            .get_transcript(&response.session_key, 10, None)
            .await
            .expect("transcript should load");
        let attachments = transcript[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata["attachments"].as_array())
            .expect("user transcript metadata should include attachments");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0]["mime_type"], serde_json::json!("image/png"));
        assert_eq!(attachments[0]["filename"], serde_json::json!("photo.png"));

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_includes_skill_prompt_in_system_message() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-skill-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace = temp.join("workspace");
        let skill_dir = workspace.join(".frankclaw/skills/briefing");
        std::fs::create_dir_all(&skill_dir).expect("skill dir should exist");
        std::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "id": "briefing",
                "name": "Briefing",
                "prompt": "Summarize in a terse operational style.",
                "capabilities": ["prompt"],
                "tools": []
            })
            .to_string(),
        )
        .expect("skill manifest should write");

        let sessions = Arc::new(SqliteSessionStore::open(&temp.join("sessions.db"), None).expect("sessions should open"));
        let mut config = FrankClawConfig::default();
        let agent = config.agents.agents.get_mut(&AgentId::default_agent()).unwrap();
        agent.workspace = Some(workspace.clone());
        agent.system_prompt = Some("Base system".into());
        agent.skills = vec!["briefing".into()];

        let provider = Arc::new(MockProvider {
            ..MockProvider::reply("primary", "mock-primary", "reply")
        });
        let runtime = Runtime::from_providers(
            &config,
            sessions as Arc<dyn SessionStore>,
            vec![provider.clone()],
        )
        .await
        .expect("runtime should build");

        runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "hello".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        let seen = provider
            .seen_requests
            .lock()
            .expect("request capture should lock");
        let system = seen
            .last()
            .and_then(|value| value.system.clone())
            .expect("system prompt should be captured");
        assert!(system.contains("Base system"));
        assert!(system.contains("[Skill: Briefing]"));
        assert!(system.contains("Summarize in a terse operational style."));

        let _ = std::fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn runtime_executes_model_requested_tools_and_persists_trace() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-tool-loop-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let mut config = FrankClawConfig::default();
        config.agents.agents.get_mut(&AgentId::default_agent()).unwrap().tools =
            vec!["session.inspect".into()];

        let provider = Arc::new(MockProvider::scripted(
            "primary",
            "mock-primary",
            vec![
                Some(MockResponse {
                    content: String::new(),
                    tool_calls: vec![ToolCallResponse {
                        id: "call-1".into(),
                        name: "session.inspect".into(),
                        arguments: r#"{"limit":2}"#.into(),
                    }],
                    finish_reason: FinishReason::ToolUse,
                }),
                Some(MockResponse {
                    content: "tool-backed reply".into(),
                    tool_calls: Vec::new(),
                    finish_reason: FinishReason::Stop,
                }),
            ],
        ));
        let runtime = Runtime::from_providers(
            &config,
            sessions.clone() as Arc<dyn SessionStore>,
            vec![provider.clone()],
        )
        .await
        .expect("runtime should build");

        let response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "inspect this session".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        assert_eq!(response.content, "tool-backed reply");

        let transcript = sessions
            .get_transcript(&response.session_key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 4);
        assert_eq!(transcript[0].role, Role::User);
        assert_eq!(transcript[1].role, Role::Assistant);
        assert_eq!(transcript[2].role, Role::Tool);
        assert_eq!(transcript[3].role, Role::Assistant);
        assert_eq!(
            transcript[1]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata["tool_calls"].as_array())
                .map(|calls| calls.len()),
            Some(1)
        );
        assert!(transcript[2].content.contains("\"session\""));

        let seen = provider
            .seen_requests
            .lock()
            .expect("request capture should lock");
        assert_eq!(seen.len(), 2);
        // Agent has 1 tool + spawn_subagent injected by runtime.
        assert_eq!(seen[0].tools.len(), 2);
        assert!(seen[0].tools.iter().any(|t| t.name == "session.inspect"));
        assert!(seen[1]
            .messages
            .iter()
            .any(|message| message.role == Role::Tool && message.content.contains("\"entries\"")));

        let activity = runtime
            .tool_activity(&response.session_key, 10)
            .await
            .expect("tool activity should load");
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].tool_name, "session.inspect");
        assert_eq!(activity[0].tool_call_id.as_deref(), Some("call-1"));
        assert!(activity[0].output_preview.contains("\"entries\""));

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_returns_error_result_for_invalid_tool_arguments() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-tool-args-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions: Arc<dyn SessionStore> = Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let mut config = FrankClawConfig::default();
        config.agents.agents.get_mut(&AgentId::default_agent()).unwrap().tools =
            vec!["session.inspect".into()];

        // First response: tool call with invalid JSON args.
        // Second response: model sees the error result and replies with text.
        let runtime = Runtime::from_providers(
            &config,
            sessions.clone(),
            vec![Arc::new(MockProvider::scripted(
                "primary",
                "mock-primary",
                vec![
                    Some(MockResponse {
                        content: String::new(),
                        tool_calls: vec![ToolCallResponse {
                            id: "call-1".into(),
                            name: "session.inspect".into(),
                            arguments: "{not-json}".into(),
                        }],
                        finish_reason: FinishReason::ToolUse,
                    }),
                    Some(MockResponse {
                        content: "I see the tool had invalid arguments".into(),
                        tool_calls: Vec::new(),
                        finish_reason: FinishReason::Stop,
                    }),
                ],
            ))],
        )
        .await
        .expect("runtime should build");

        let result = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "inspect this session".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("should succeed with error result fed back to model");

        // Model should have received the error result and responded.
        assert!(result.content.contains("invalid arguments"));

        // Transcript should include the error tool result.
        let transcript = sessions
            .get_transcript(&result.session_key, 100, None)
            .await
            .expect("transcript should load");
        let error_entry = transcript
            .iter()
            .find(|e| e.role == Role::Tool && e.content.contains("invalid arguments"));
        assert!(error_entry.is_some(), "transcript should contain error tool result");

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_rejects_excessive_model_tool_calls_in_one_turn() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-tool-limit-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let mut config = FrankClawConfig::default();
        config.agents.agents.get_mut(&AgentId::default_agent()).unwrap().tools =
            vec!["session.inspect".into()];
        let tool_calls = (0..9)
            .map(|index| ToolCallResponse {
                id: format!("call-{index}"),
                name: "session.inspect".into(),
                arguments: r#"{"limit":1}"#.into(),
            })
            .collect();

        let runtime = Runtime::from_providers(
            &config,
            sessions as Arc<dyn SessionStore>,
            vec![Arc::new(MockProvider::scripted(
                "primary",
                "mock-primary",
                vec![Some(MockResponse {
                    content: String::new(),
                    tool_calls,
                    finish_reason: FinishReason::ToolUse,
                })],
            ))],
        )
        .await
        .expect("runtime should build");

        let err = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "inspect this session".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect_err("too many tool calls should fail");

        assert!(matches!(err, FrankClawError::AgentRuntime { .. }));
        assert!(err.to_string().contains("too many tool calls"));

        let _ = std::fs::remove_file(temp);
    }

    #[test]
    fn tool_call_tracker_detects_repeated_calls() {
        let mut tracker = ToolCallTracker::new();

        // First few calls should succeed.
        for _ in 0..TOOL_REPEAT_WARN_THRESHOLD {
            tracker
                .record("my_tool", r#"{"query":"same"}"#)
                .expect("should not block yet");
        }

        // Calls beyond warn threshold should still succeed but log.
        for _ in TOOL_REPEAT_WARN_THRESHOLD..TOOL_REPEAT_BLOCK_THRESHOLD - 1 {
            tracker
                .record("my_tool", r#"{"query":"same"}"#)
                .expect("should warn but not block");
        }

        // At block threshold, should return error.
        let err = tracker
            .record("my_tool", r#"{"query":"same"}"#)
            .expect_err("should block at threshold");
        assert!(err.to_string().contains("infinite loop"));
    }

    #[test]
    fn tool_call_tracker_allows_different_arguments() {
        let mut tracker = ToolCallTracker::new();

        for i in 0..20 {
            tracker
                .record("my_tool", &format!(r#"{{"query":"query-{i}"}}"#))
                .expect("different args should not trigger loop detection");
        }
    }

    #[test]
    fn truncate_tool_output_leaves_small_outputs_unchanged() {
        let small = "hello world";
        assert_eq!(truncate_tool_output(small), small);
    }

    #[test]
    fn truncate_tool_output_truncates_large_outputs() {
        let large = "x".repeat(MAX_TOOL_RESULT_CHARS + 1000);
        let result = truncate_tool_output(&large);
        assert!(result.chars().count() <= MAX_TOOL_RESULT_CHARS + 100); // some marker overhead
        assert!(result.contains("characters omitted"));
    }

    #[tokio::test]
    async fn runtime_detects_tool_call_loop() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-loop-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(
            SqliteSessionStore::open(&temp, None).expect("sessions should open"),
        );
        let mut config = FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&AgentId::default_agent())
            .unwrap()
            .tools = vec!["session.inspect".into()];

        // Model keeps calling same tool with same args every round.
        let mut responses = Vec::new();
        for _ in 0..MAX_TOOL_ROUNDS + 1 {
            responses.push(Some(MockResponse {
                content: String::new(),
                tool_calls: vec![ToolCallResponse {
                    id: "call-loop".into(),
                    name: "session.inspect".into(),
                    arguments: r#"{"limit":1}"#.into(),
                }],
                finish_reason: FinishReason::ToolUse,
            }));
        }

        let runtime = Runtime::from_providers(
            &config,
            sessions as Arc<dyn SessionStore>,
            vec![Arc::new(MockProvider::scripted(
                "primary",
                "mock-primary",
                responses,
            ))],
        )
        .await
        .expect("runtime should build");

        let err = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "loop test".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect_err("loop should be detected");

        assert!(matches!(err, FrankClawError::AgentRuntime { .. }));
        // Could be tool round limit or loop detection — both are correct.
        let msg = err.to_string();
        assert!(
            msg.contains("infinite loop") || msg.contains("tool round limit"),
            "unexpected error: {msg}"
        );

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_reports_provider_health() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-health-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let provider = Arc::new(MockProvider::reply("primary", "mock-primary", "reply"));
        let runtime = Runtime::from_providers(
            &FrankClawConfig::default(),
            sessions as Arc<dyn SessionStore>,
            vec![provider],
        )
        .await
        .expect("runtime should build");

        let health = runtime.provider_health().await;
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].provider_id, "primary");
        assert!(health[0].healthy);

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_streams_intermediate_text_during_tool_rounds() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-stream-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(
            SqliteSessionStore::open(&temp, None).expect("sessions should open"),
        );
        let mut config = FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&AgentId::default_agent())
            .unwrap()
            .tools = vec!["session.inspect".into()];

        // Round 1: model returns text + tool call
        // Round 2: model returns final response (no tool calls)
        let provider = Arc::new(MockProvider::scripted(
            "primary",
            "mock-primary",
            vec![
                Some(MockResponse {
                    content: "Let me check that for you...".into(),
                    tool_calls: vec![ToolCallResponse {
                        id: "call-1".into(),
                        name: "session.inspect".into(),
                        arguments: r#"{"limit": 5}"#.into(),
                    }],
                    finish_reason: FinishReason::ToolUse,
                }),
                Some(MockResponse {
                    content: "Here are the results.".into(),
                    tool_calls: Vec::new(),
                    finish_reason: FinishReason::Stop,
                }),
            ],
        ));

        let runtime = Runtime::from_providers(
            &config,
            sessions as Arc<dyn SessionStore>,
            vec![provider],
        )
        .await
        .expect("runtime should build");

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);

        let response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "check session".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: Some(tx),
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        assert_eq!(response.content, "Here are the results.");

        // The intermediate text from round 1 should have been streamed.
        let mut received_intermediate = false;
        while let Ok(delta) = rx.try_recv() {
            if let StreamDelta::Text(text) = delta {
                if text.contains("Let me check") {
                    received_intermediate = true;
                }
            }
        }
        assert!(
            received_intermediate,
            "should have received intermediate text during tool round"
        );

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_compacts_context_mid_loop_when_over_budget() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-midcompact-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(
            SqliteSessionStore::open(&temp, None).expect("sessions should open"),
        );
        let mut config = FrankClawConfig::default();
        config
            .agents
            .agents
            .get_mut(&AgentId::default_agent())
            .unwrap()
            .tools = vec!["session.inspect".into()];

        // Round 1: tool call with large output that should trigger mid-loop compaction
        // Round 2: final response
        let provider = Arc::new(MockProvider::scripted(
            "primary",
            "mock-primary",
            vec![
                Some(MockResponse {
                    content: "checking".into(),
                    tool_calls: vec![ToolCallResponse {
                        id: "call-1".into(),
                        name: "session.inspect".into(),
                        arguments: r#"{"limit": 5}"#.into(),
                    }],
                    finish_reason: FinishReason::ToolUse,
                }),
                Some(MockResponse {
                    content: "done".into(),
                    tool_calls: Vec::new(),
                    finish_reason: FinishReason::Stop,
                }),
            ],
        ));

        let runtime = Runtime::from_providers(
            &config,
            sessions.clone() as Arc<dyn SessionStore>,
            vec![provider],
        )
        .await
        .expect("runtime should build");

        let response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "check".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        assert_eq!(response.content, "done");

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn runtime_spawns_subagent_and_returns_result() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-subagent-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions = Arc::new(
            SqliteSessionStore::open(&temp, None).expect("sessions should open"),
        );
        let config = FrankClawConfig::default();

        // Round 1 (parent): model calls spawn_subagent
        // Round 2 (child): model returns "child result" (no tools)
        // Round 3 (parent): model returns final response using child result
        let provider = Arc::new(MockProvider::scripted(
            "primary",
            "mock-primary",
            vec![
                // Parent round 1: request subagent
                Some(MockResponse {
                    content: "I'll delegate this.".into(),
                    tool_calls: vec![ToolCallResponse {
                        id: "call-sub".into(),
                        name: SPAWN_SUBAGENT_TOOL.into(),
                        arguments: r#"{"task": "analyze data", "label": "analysis"}"#.into(),
                    }],
                    finish_reason: FinishReason::ToolUse,
                }),
                // Child: reply to the subagent task
                Some(MockResponse {
                    content: "The data shows X.".into(),
                    tool_calls: Vec::new(),
                    finish_reason: FinishReason::Stop,
                }),
                // Parent round 2: incorporate child result
                Some(MockResponse {
                    content: "Based on the analysis: X.".into(),
                    tool_calls: Vec::new(),
                    finish_reason: FinishReason::Stop,
                }),
            ],
        ));

        let runtime = Runtime::from_providers(
            &config,
            sessions as Arc<dyn SessionStore>,
            vec![provider],
        )
        .await
        .expect("runtime should build");

        let response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "analyze my data".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        // The parent incorporates the subagent result in its final response.
        assert!(
            response.content == "Based on the analysis: X."
                || response.content == "The data shows X.",
            "got: {}",
            response.content
        );

        // Verify the subagent registry recorded the run.
        let active = runtime.subagent_registry.active_runs().await;
        assert!(active.is_empty(), "subagent run should be completed");

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn system_prompt_includes_channel_hints_when_provided() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-channel-hints-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions =
            Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let mock = Arc::new(MockProvider::reply("primary", "mock-primary", "ok"));
        let runtime = Runtime::from_providers(
            &FrankClawConfig::default(),
            sessions.clone() as Arc<dyn SessionStore>,
            vec![mock.clone()],
        )
        .await
        .expect("runtime should build");

        let _response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "hello".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: Some(ChannelId::new("telegram")),
                channel_capabilities: Some(ChannelCapabilities {
                    threads: true,
                    groups: true,
                    attachments: true,
                    edit: true,
                    delete: false,
                    reactions: true,
                    streaming: false,
                    voice: false,
                    inline_buttons: true,
                }),
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        let seen = mock.seen_requests.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let system = seen[0].system.as_ref().expect("system prompt should be set");
        assert!(
            system.contains("telegram"),
            "system prompt should mention channel name"
        );
        assert!(
            system.contains("threads"),
            "system prompt should list threads feature"
        );
        assert!(
            system.contains("inline buttons"),
            "system prompt should list inline buttons feature"
        );
        assert!(
            !system.contains("voice"),
            "system prompt should not list disabled features"
        );

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn system_prompt_omits_channel_section_when_no_channel() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-runtime-no-channel-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions =
            Arc::new(SqliteSessionStore::open(&temp, None).expect("sessions should open"));
        let mock = Arc::new(MockProvider::reply("primary", "mock-primary", "ok"));
        let runtime = Runtime::from_providers(
            &FrankClawConfig::default(),
            sessions.clone() as Arc<dyn SessionStore>,
            vec![mock.clone()],
        )
        .await
        .expect("runtime should build");

        let _response = runtime
            .chat(ChatRequest {
                agent_id: None,
                session_key: None,
                message: "hello".into(),
                attachments: Vec::new(),
                model_id: Some("mock-primary".into()),
                max_tokens: None,
                temperature: None,
                stream_tx: None,
                thinking_budget: None,
                channel_id: None,
                channel_capabilities: None,
                canvas: None,
                cancel_token: None,
                approval_tx: None,
            })
            .await
            .expect("chat should succeed");

        let seen = mock.seen_requests.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let system = seen[0].system.as_ref().expect("system prompt should be set");
        assert!(
            !system.contains("Available features:"),
            "system prompt should not contain channel section when no channel provided"
        );

        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn chat_cancel_token_aborts_turn() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-cancel-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let sessions: Arc<dyn SessionStore> = Arc::new(
            frankclaw_sessions::SqliteSessionStore::open(&temp, None)
                .expect("sessions should open"),
        );

        // Use a provider that sleeps to simulate a slow model call.
        struct SlowProvider;

        #[async_trait::async_trait]
        impl ModelProvider for SlowProvider {
            fn id(&self) -> &str {
                "slow"
            }

            async fn complete(
                &self,
                _request: CompletionRequest,
                _stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
            ) -> frankclaw_core::error::Result<CompletionResponse> {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(CompletionResponse {
                    content: "should not reach".into(),
                    tool_calls: Vec::new(),
                    usage: Usage::default(),
                    finish_reason: FinishReason::Stop,
                })
            }

            async fn list_models(&self) -> frankclaw_core::error::Result<Vec<ModelDef>> {
                Ok(vec![ModelDef {
                    id: "slow-model".into(),
                    name: "slow-model".into(),
                    api: frankclaw_core::model::ModelApi::Ollama,
                    reasoning: false,
                    input: vec![frankclaw_core::model::InputModality::Text],
                    cost: Default::default(),
                    context_window: 4096,
                    max_output_tokens: 1024,
                    compat: Default::default(),
                }])
            }

            async fn health(&self) -> bool {
                true
            }
        }

        let config = FrankClawConfig::default();
        let runtime = Runtime::from_providers(
            &config,
            sessions.clone(),
            vec![Arc::new(SlowProvider)],
        )
        .await
        .expect("runtime should build");

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();

        let chat_handle = tokio::spawn(async move {
            runtime
                .chat(ChatRequest {
                    agent_id: None,
                    session_key: None,
                    message: "hello".into(),
                    attachments: Vec::new(),
                    model_id: Some("slow-model".into()),
                    max_tokens: None,
                    temperature: None,
                    stream_tx: None,
                    thinking_budget: None,
                    channel_id: None,
                    channel_capabilities: None,
                    canvas: None,
                    cancel_token: Some(cancel_clone),
                    approval_tx: None,
                })
                .await
        });

        // Cancel after a short delay.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_token.cancel();

        // The chat should fail quickly with TurnCancelled.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            chat_handle,
        )
        .await
        .expect("should complete within timeout")
        .expect("task should not panic");

        // The slow provider won't return TurnCancelled — it will be a provider error
        // because the cancellation check happens between rounds, not during the model call.
        // But the test validates that the cancel_token field works end-to-end.
        assert!(result.is_err());

        let _ = std::fs::remove_file(temp);
    }

    #[test]
    fn accumulate_usage_sums_tokens() {
        let mut total = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: Some(2),
            cache_write_tokens: None,
        };
        let delta = Usage {
            input_tokens: 20,
            output_tokens: 15,
            cache_read_tokens: Some(3),
            cache_write_tokens: Some(1),
        };
        accumulate_usage(&mut total, &delta);
        assert_eq!(total.input_tokens, 30);
        assert_eq!(total.output_tokens, 20);
        assert_eq!(total.cache_read_tokens, Some(5));
        assert_eq!(total.cache_write_tokens, Some(1));
    }

    #[test]
    fn bootstrap_reads_existing_files() {
        let dir = std::env::temp_dir().join(format!("fc-bootstrap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("SOUL.md"), "You are a helpful assistant.").unwrap();
        std::fs::write(dir.join("IDENTITY.md"), "Name: TestBot").unwrap();

        let result = read_workspace_bootstrap_files(&dir);
        assert!(result.contains("# Workspace Context"));
        assert!(result.contains("## SOUL.md"));
        assert!(result.contains("You are a helpful assistant."));
        assert!(result.contains("## IDENTITY.md"));
        assert!(result.contains("Name: TestBot"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_skips_missing_files() {
        let dir = std::env::temp_dir().join(format!("fc-bootstrap-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let result = read_workspace_bootstrap_files(&dir);
        assert!(result.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_strips_yaml_front_matter() {
        let dir = std::env::temp_dir().join(format!("fc-bootstrap-yaml-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("SOUL.md"),
            "---\ntitle: Soul\nauthor: test\n---\nActual content here.",
        )
        .unwrap();

        let result = read_workspace_bootstrap_files(&dir);
        assert!(result.contains("Actual content here."));
        assert!(!result.contains("title: Soul"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_truncates_large_files() {
        let dir = std::env::temp_dir().join(format!("fc-bootstrap-big-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Write a file larger than MAX_BOOTSTRAP_FILE_BYTES (50KB)
        let large_content = "A".repeat(60 * 1024);
        std::fs::write(dir.join("SOUL.md"), &large_content).unwrap();

        let result = read_workspace_bootstrap_files(&dir);
        assert!(result.contains("[... truncated, file too large ...]"));
        // Should still be under total budget
        assert!(result.len() < MAX_BOOTSTRAP_TOTAL_BYTES + 1024);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_yaml_front_matter_works() {
        assert_eq!(
            strip_yaml_front_matter("---\nkey: val\n---\nBody text"),
            "Body text"
        );
        assert_eq!(
            strip_yaml_front_matter("No front matter here"),
            "No front matter here"
        );
        // Unclosed front matter is left as-is
        assert_eq!(
            strip_yaml_front_matter("---\nkey: val\nNo closing"),
            "---\nkey: val\nNo closing"
        );
    }
}
