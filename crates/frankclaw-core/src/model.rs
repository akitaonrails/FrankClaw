use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::Role;

/// Model API variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelApi {
    OpenaiCompletions,
    OpenaiResponses,
    AnthropicMessages,
    GoogleGenerativeAi,
    Ollama,
    BedrockConverseStream,
}

/// What input modalities a model accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputModality {
    Text,
    Image,
    Audio,
}

/// Model cost per million tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCost {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
}

/// Model compatibility flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCompat {
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_streaming: bool,
    pub supports_json_mode: bool,
    pub supports_system_message: bool,
}

/// Definition of a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDef {
    pub id: String,
    pub name: String,
    pub api: ModelApi,
    pub reasoning: bool,
    pub input: Vec<InputModality>,
    pub cost: ModelCost,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub compat: ModelCompat,
}

/// A message in a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionMessage {
    pub role: Role,
    pub content: String,
    // Tool calls, images, etc. will be added as needed.
}

/// Request to a model provider.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model_id: String,
    pub messages: Vec<CompletionMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system: Option<String>,
    pub tools: Vec<ToolDef>,
}

/// Risk classification for tools. Determines whether operator approval is needed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolRiskLevel {
    /// Read-only operations: always auto-approved.
    #[default]
    ReadOnly,
    /// Mutating operations: require at least `Mutating` approval level.
    Mutating,
    /// Destructive operations: require explicit `Destructive` approval level.
    Destructive,
}

impl std::fmt::Display for ToolRiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "readonly"),
            Self::Mutating => write!(f, "mutating"),
            Self::Destructive => write!(f, "destructive"),
        }
    }
}

/// Tool definition for function calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub risk_level: ToolRiskLevel,
}

/// Streaming delta from a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDelta {
    Text(String),
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, arguments: String },
    ToolCallEnd { id: String },
    Done { usage: Option<Usage> },
    Error(String),
}

/// Token usage stats.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
}

/// Response from a model.
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallResponse>,
    pub usage: Usage,
    pub finish_reason: FinishReason,
}

/// A tool call in a completion response.
#[derive(Debug, Clone)]
pub struct ToolCallResponse {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    MaxTokens,
    ToolUse,
    ContentFilter,
}

/// Trait for model provider backends.
#[async_trait]
pub trait ModelProvider: Send + Sync + 'static {
    /// Unique provider identifier.
    fn id(&self) -> &str;

    /// Run a completion. If `stream_tx` is Some, stream deltas to it.
    async fn complete(
        &self,
        request: CompletionRequest,
        stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse>;

    /// List available models.
    async fn list_models(&self) -> Result<Vec<ModelDef>>;

    /// Check provider health.
    async fn health(&self) -> bool;
}
