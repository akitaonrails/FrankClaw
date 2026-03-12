//! Canvas service trait for structured scratchpad documents.
//!
//! The trait is defined here so both the gateway (which owns the store)
//! and the tools crate (which exposes canvas tools to the LLM) can share it.

use async_trait::async_trait;

use crate::error::Result;

/// A single structured block inside a canvas document.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CanvasBlock {
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// A canvas document — the shared scratchpad between agent and user.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanvasDocument {
    pub id: String,
    pub title: String,
    pub body: String,
    pub session_key: Option<String>,
    #[serde(default)]
    pub blocks: Vec<CanvasBlock>,
    pub revision: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Trait for canvas storage backends. Implemented by the gateway's `CanvasStore`.
#[async_trait]
pub trait CanvasService: Send + Sync + 'static {
    /// Get a canvas document by ID.
    async fn get(&self, canvas_id: &str) -> Option<CanvasDocument>;

    /// Create or fully replace a canvas document.
    async fn set(&self, document: CanvasDocument) -> Result<CanvasDocument>;

    /// Append blocks and/or update title/body on an existing document.
    async fn patch(
        &self,
        canvas_id: &str,
        title: Option<String>,
        body: Option<String>,
        append_blocks: Vec<CanvasBlock>,
    ) -> Result<CanvasDocument>;

    /// Delete a canvas document.
    async fn clear(&self, canvas_id: &str);
}
