use crate::types::RequestId;
use serde::{Deserialize, Serialize};

/// WebSocket protocol frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Request(RequestFrame),
    Response(ResponseFrame),
    Event(EventFrame),
}

/// Client → Server RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFrame {
    pub id: RequestId,
    pub method: Method,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Server → Client RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFrame {
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

/// Server → Client unsolicited event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFrame {
    pub event: EventType,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// All known RPC methods. Exhaustive matching ensures no method is forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    // Agent
    AgentIdentity,
    AgentList,

    // Chat
    ChatSend,
    ChatHistory,
    ChatCancel,

    // Channels
    ChannelsStatus,
    ChannelsList,

    // Config
    ConfigGet,
    ConfigPatch,
    ConfigApply,
    ConfigSchema,

    // Cron
    CronList,
    CronAdd,
    CronUpdate,
    CronRemove,
    CronRun,

    // Sessions
    SessionsList,
    SessionsGet,
    SessionsPatch,
    SessionsReset,
    SessionsDelete,

    // Models
    ModelsList,

    // Canvas
    CanvasGet,
    CanvasExport,
    CanvasSet,
    CanvasPatch,
    CanvasClear,

    // Webhooks
    WebhooksAdd,
    WebhooksRemove,
    WebhooksTest,

    // Logs
    LogsTail,
    LogsQuery,

    // Nodes (device pairing)
    NodesList,
    NodesPairRequest,
    NodesPairApprove,
    NodesPairReject,

    // Health
    HealthProbe,
}

impl Method {
    /// Minimum role required to call this method.
    pub fn min_role(&self) -> crate::auth::AuthRole {
        use crate::auth::AuthRole;
        match self {
            // Admin-only
            Self::ConfigPatch | Self::ConfigApply => AuthRole::Admin,

            // Editor
            Self::ChatSend
            | Self::ChatCancel
            | Self::SessionsPatch
            | Self::SessionsReset
            | Self::SessionsDelete
            | Self::CronAdd
            | Self::CronUpdate
            | Self::CronRemove
            | Self::CronRun
            | Self::CanvasSet
            | Self::CanvasPatch
            | Self::CanvasClear
            | Self::WebhooksAdd
            | Self::WebhooksRemove
            | Self::WebhooksTest
            | Self::NodesPairApprove
            | Self::NodesPairReject => AuthRole::Editor,

            // Viewer (read-only)
            Self::AgentIdentity
            | Self::AgentList
            | Self::ChatHistory
            | Self::ChannelsStatus
            | Self::ChannelsList
            | Self::ConfigGet
            | Self::ConfigSchema
            | Self::CronList
            | Self::SessionsList
            | Self::SessionsGet
            | Self::ModelsList
            | Self::CanvasGet
            | Self::CanvasExport
            | Self::LogsTail
            | Self::LogsQuery
            | Self::NodesList
            | Self::NodesPairRequest
            | Self::HealthProbe => AuthRole::Viewer,
        }
    }
}

/// All known event types pushed to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    ChatDelta,
    ChatComplete,
    ChatError,
    PresenceUpdate,
    ChannelHealth,
    ConfigChanged,
    SessionUpdated,
    CanvasUpdated,
    CronRun,
    LogEntry,
    NodePairRequest,
}

/// Structured protocol error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: u16,
    pub message: String,
}

impl ResponseFrame {
    pub fn ok(id: RequestId, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: RequestId, code: u16, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(ProtocolError {
                code,
                message: message.into(),
            }),
        }
    }
}
