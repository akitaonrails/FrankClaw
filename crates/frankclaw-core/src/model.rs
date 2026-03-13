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

/// An image content block attached to a message for vision-capable models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    /// MIME type (e.g. "image/jpeg", "image/png").
    pub mime_type: String,
    /// Base64-encoded image data.
    pub data: String,
}

/// A message in a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionMessage {
    pub role: Role,
    pub content: String,
    /// Tool calls requested by the assistant (present when role is Assistant).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallResponse>,
    /// The tool_call_id this message is responding to (present when role is Tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Image content blocks for vision-capable models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_content: Vec<ImageContent>,
}

impl CompletionMessage {
    /// Create a simple text message (no tool calls).
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            image_content: Vec::new(),
        }
    }

    /// Create a user message with text and images for vision-capable models.
    pub fn with_images(content: impl Into<String>, images: Vec<ImageContent>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            image_content: images,
        }
    }

    /// Create an assistant message with tool calls.
    pub fn assistant_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolCallResponse>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
            image_content: Vec::new(),
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            image_content: Vec::new(),
        }
    }

    /// Create a tool result message with attached images (for vision-capable models).
    pub fn tool_result_with_images(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        images: Vec<ImageContent>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            image_content: images,
        }
    }
}

/// Response format for structured outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
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
    /// Extended thinking budget in tokens. When set, the model will use
    /// internal chain-of-thought reasoning before responding.
    /// Supported by Anthropic Claude 3.7+.
    pub thinking_budget: Option<u32>,
    /// Whether to allow the model to call multiple tools in parallel (OpenAI).
    pub parallel_tool_calls: Option<bool>,
    /// Seed for deterministic completions (OpenAI).
    pub seed: Option<u64>,
    /// Response format: text or JSON mode (OpenAI).
    pub response_format: Option<ResponseFormat>,
    /// Reasoning effort for o1/o3 models: "low", "medium", "high" (OpenAI).
    pub reasoning_effort: Option<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
