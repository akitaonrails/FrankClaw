#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Memory entry category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    Fact,
    Observation,
    UserPreference,
    Goal,
    Other,
}

/// A memory entry stored in the vector database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    pub category: MemoryCategory,
    pub importance: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Search result with relevance score.
#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub entry: MemoryEntry,
    pub score: f32,
}

/// Abstract memory store backend.
///
/// Implementations might use LanceDB, SQLite FTS5, or in-memory stores.
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    /// Store a memory entry with its embedding vector.
    async fn store(
        &self,
        entry: &MemoryEntry,
        embedding: &[f32],
    ) -> frankclaw_core::error::Result<()>;

    /// Search memories by semantic similarity.
    async fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_score: f32,
    ) -> frankclaw_core::error::Result<Vec<MemorySearchResult>>;

    /// Delete a memory by ID.
    async fn delete(&self, id: &str) -> frankclaw_core::error::Result<()>;

    /// List all memories (paginated).
    async fn list(
        &self,
        limit: usize,
        offset: usize,
    ) -> frankclaw_core::error::Result<Vec<MemoryEntry>>;
}

/// Abstract embedding provider.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync + 'static {
    /// Generate an embedding vector for the given text.
    async fn embed(&self, text: &str) -> frankclaw_core::error::Result<Vec<f32>>;

    /// Embedding dimension (e.g., 1536 for OpenAI text-embedding-3-small).
    fn dimension(&self) -> usize;
}
