#![forbid(unsafe_code)]

pub mod chunking;
pub mod embedding;
pub mod store;
pub mod sync;

pub use chunking::chunk_text;
pub use embedding::{EmbeddingProvider, CachedEmbeddingProvider};
pub use store::SqliteMemoryStore;
pub use sync::MemorySyncer;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A chunk of content stored in the memory system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkEntry {
    pub id: String,
    pub source: String,
    pub text: String,
    pub line_start: usize,
    pub line_end: usize,
    pub chunk_index: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Search result with relevance score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub chunk: ChunkEntry,
    pub score: f32,
}

/// Options for hybrid search.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub min_score: f32,
    /// Weight for vector similarity (0.0-1.0). BM25 gets 1.0 - this.
    pub vector_weight: f32,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 10,
            min_score: 0.0,
            vector_weight: 0.6,
        }
    }
}

/// Abstract memory store backend.
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    async fn store_chunk(
        &self,
        chunk: &ChunkEntry,
        embedding: &[f32],
    ) -> frankclaw_core::error::Result<()>;

    async fn search_hybrid(
        &self,
        query: &str,
        query_embedding: &[f32],
        options: &SearchOptions,
    ) -> frankclaw_core::error::Result<Vec<SearchResult>>;

    async fn delete_by_source(&self, source: &str) -> frankclaw_core::error::Result<usize>;

    async fn list_sources(&self) -> frankclaw_core::error::Result<Vec<SourceInfo>>;

    async fn has_source(&self, source: &str) -> frankclaw_core::error::Result<bool>;
}

/// Info about a stored source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub source: String,
    pub chunk_count: usize,
    pub content_hash: String,
}
