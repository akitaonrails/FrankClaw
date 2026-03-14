use std::sync::Mutex;

use rusqlite::params;
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};

use crate::{ChunkEntry, MemoryStore, SearchOptions, SearchResult, SourceInfo};

/// SQLite-backed memory store with FTS5 for text search and in-Rust cosine for vectors.
pub struct SqliteMemoryStore {
    db: Mutex<rusqlite::Connection>,
}

impl SqliteMemoryStore {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = rusqlite::Connection::open(path).map_err(|e| FrankClawError::MemoryStore {
            msg: format!("failed to open memory store: {e}"),
        })?;
        Self::run_migrations(&conn)?;
        Ok(Self {
            db: Mutex::new(conn),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn =
            rusqlite::Connection::open_in_memory().map_err(|e| FrankClawError::MemoryStore {
                msg: format!("failed to open in-memory store: {e}"),
            })?;
        Self::run_migrations(&conn)?;
        Ok(Self {
            db: Mutex::new(conn),
        })
    }

    fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS memory_chunks (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                text TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end INTEGER NOT NULL,
                chunk_index INTEGER NOT NULL,
                content_hash TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_memory_chunks_source
                ON memory_chunks(source);

            CREATE TABLE IF NOT EXISTS memory_embeddings (
                chunk_id TEXT PRIMARY KEY REFERENCES memory_chunks(id) ON DELETE CASCADE,
                embedding BLOB NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memory_source_hashes (
                source TEXT PRIMARY KEY,
                content_hash TEXT NOT NULL
            );
            ",
        )
        .map_err(|e| FrankClawError::MemoryStore {
            msg: format!("migration failed: {e}"),
        })?;

        // Create FTS5 virtual table if not exists.
        let has_fts: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memory_fts'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_fts {
            conn.execute_batch(
                "CREATE VIRTUAL TABLE memory_fts USING fts5(
                    text,
                    content='memory_chunks',
                    content_rowid='rowid'
                );

                CREATE TRIGGER IF NOT EXISTS memory_chunks_ai AFTER INSERT ON memory_chunks BEGIN
                    INSERT INTO memory_fts(rowid, text) VALUES (new.rowid, new.text);
                END;
                CREATE TRIGGER IF NOT EXISTS memory_chunks_ad AFTER DELETE ON memory_chunks BEGIN
                    INSERT INTO memory_fts(memory_fts, rowid, text) VALUES('delete', old.rowid, old.text);
                END;
                CREATE TRIGGER IF NOT EXISTS memory_chunks_au AFTER UPDATE ON memory_chunks BEGIN
                    INSERT INTO memory_fts(memory_fts, rowid, text) VALUES('delete', old.rowid, old.text);
                    INSERT INTO memory_fts(rowid, text) VALUES (new.rowid, new.text);
                END;",
            )
            .map_err(|e| FrankClawError::MemoryStore {
                msg: format!("FTS5 table creation failed: {e}"),
            })?;
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl MemoryStore for SqliteMemoryStore {
    async fn store_chunk(&self, chunk: &ChunkEntry, embedding: &[f32]) -> Result<()> {
        let db = self.db.lock().map_err(|_| FrankClawError::MemoryStore {
            msg: "lock poisoned".into(),
        })?;

        db.execute(
            "INSERT OR REPLACE INTO memory_chunks (id, source, text, line_start, line_end, chunk_index, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                chunk.id,
                chunk.source,
                chunk.text,
                chunk.line_start as i64,
                chunk.line_end as i64,
                chunk.chunk_index as i64,
                chunk.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| FrankClawError::MemoryStore {
            msg: format!("failed to store chunk: {e}"),
        })?;

        let blob = f32_to_bytes(embedding);
        db.execute(
            "INSERT OR REPLACE INTO memory_embeddings (chunk_id, embedding) VALUES (?1, ?2)",
            params![chunk.id, blob],
        )
        .map_err(|e| FrankClawError::MemoryStore {
            msg: format!("failed to store embedding: {e}"),
        })?;

        Ok(())
    }

    async fn search_hybrid(
        &self,
        query: &str,
        query_embedding: &[f32],
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let db = self.db.lock().map_err(|_| FrankClawError::MemoryStore {
            msg: "lock poisoned".into(),
        })?;

        // 1. BM25 text search.
        let mut bm25_scores: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        {
            // Escape FTS5 special characters in query.
            let safe_query = query
                .replace('"', "\"\"")
                .split_whitespace()
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" ");

            if !safe_query.is_empty() {
                let fts_query = format!("\"{safe_query}\"");
                let mut stmt = db
                    .prepare(
                        "SELECT mc.id, bm25(memory_fts) as score
                         FROM memory_fts
                         JOIN memory_chunks mc ON mc.rowid = memory_fts.rowid
                         WHERE memory_fts MATCH ?1
                         ORDER BY score
                         LIMIT ?2",
                    )
                    .map_err(|e| FrankClawError::MemoryStore {
                        msg: format!("FTS query failed: {e}"),
                    })?;

                let rows = stmt
                    .query_map(params![fts_query, options.limit as i64 * 3], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                    })
                    .map_err(|e| FrankClawError::MemoryStore {
                        msg: format!("FTS query failed: {e}"),
                    })?;

                for row in rows {
                    if let Ok((id, score)) = row {
                        // bm25() returns negative scores (lower = better). Normalize.
                        bm25_scores.insert(id, (-score as f32).max(0.0));
                    }
                }
            }
        }

        // Normalize BM25 scores to 0..1.
        let bm25_max = bm25_scores.values().copied().fold(0.0f32, f32::max);
        if bm25_max > 0.0 {
            for score in bm25_scores.values_mut() {
                *score /= bm25_max;
            }
        }

        // 2. Vector similarity (cosine) — load all embeddings.
        let mut vec_scores: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        if !query_embedding.is_empty() {
            let mut stmt = db
                .prepare(
                    "SELECT e.chunk_id, e.embedding
                     FROM memory_embeddings e",
                )
                .map_err(|e| FrankClawError::MemoryStore {
                    msg: format!("embedding query failed: {e}"),
                })?;

            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(|e| FrankClawError::MemoryStore {
                    msg: format!("embedding query failed: {e}"),
                })?;

            for row in rows {
                if let Ok((id, blob)) = row {
                    let emb = bytes_to_f32(&blob);
                    let sim = cosine_similarity(query_embedding, &emb);
                    if sim > 0.0 {
                        vec_scores.insert(id, sim);
                    }
                }
            }
        }

        // 3. Merge scores with weighted combination.
        let mut all_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        all_ids.extend(bm25_scores.keys().cloned());
        all_ids.extend(vec_scores.keys().cloned());

        let vw = options.vector_weight;
        let bw = 1.0 - vw;

        let mut scored: Vec<(String, f32)> = all_ids
            .into_iter()
            .map(|id| {
                let vs = vec_scores.get(&id).copied().unwrap_or(0.0);
                let bs = bm25_scores.get(&id).copied().unwrap_or(0.0);
                let combined = vs * vw + bs * bw;
                (id, combined)
            })
            .filter(|(_, score)| *score >= options.min_score)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(options.limit);

        // 4. Load chunk data for top results.
        let mut results = Vec::with_capacity(scored.len());
        for (id, score) in scored {
            let chunk = db
                .query_row(
                    "SELECT id, source, text, line_start, line_end, chunk_index, created_at
                     FROM memory_chunks WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok(ChunkEntry {
                            id: row.get(0)?,
                            source: row.get(1)?,
                            text: row.get(2)?,
                            line_start: row.get::<_, i64>(3)? as usize,
                            line_end: row.get::<_, i64>(4)? as usize,
                            chunk_index: row.get::<_, i64>(5)? as usize,
                            created_at: row
                                .get::<_, String>(6)?
                                .parse()
                                .unwrap_or_else(|_| chrono::Utc::now()),
                        })
                    },
                )
                .map_err(|e| FrankClawError::MemoryStore {
                    msg: format!("failed to load chunk {id}: {e}"),
                })?;
            results.push(SearchResult { chunk, score });
        }

        debug!(results = results.len(), "hybrid search complete");
        Ok(results)
    }

    async fn delete_by_source(&self, source: &str) -> Result<usize> {
        let db = self.db.lock().map_err(|_| FrankClawError::MemoryStore {
            msg: "lock poisoned".into(),
        })?;

        // Delete embeddings first (CASCADE would handle it but be explicit).
        db.execute(
            "DELETE FROM memory_embeddings WHERE chunk_id IN (SELECT id FROM memory_chunks WHERE source = ?1)",
            params![source],
        )
        .map_err(|e| FrankClawError::MemoryStore {
            msg: format!("failed to delete embeddings: {e}"),
        })?;

        let deleted = db
            .execute(
                "DELETE FROM memory_chunks WHERE source = ?1",
                params![source],
            )
            .map_err(|e| FrankClawError::MemoryStore {
                msg: format!("failed to delete chunks: {e}"),
            })?;

        db.execute(
            "DELETE FROM memory_source_hashes WHERE source = ?1",
            params![source],
        )
        .map_err(|e| FrankClawError::MemoryStore {
            msg: format!("failed to delete source hash: {e}"),
        })?;

        Ok(deleted)
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>> {
        let db = self.db.lock().map_err(|_| FrankClawError::MemoryStore {
            msg: "lock poisoned".into(),
        })?;

        let mut stmt = db
            .prepare(
                "SELECT mc.source, COUNT(*) as cnt, COALESCE(sh.content_hash, '') as hash
                 FROM memory_chunks mc
                 LEFT JOIN memory_source_hashes sh ON sh.source = mc.source
                 GROUP BY mc.source
                 ORDER BY mc.source",
            )
            .map_err(|e| FrankClawError::MemoryStore {
                msg: format!("list sources failed: {e}"),
            })?;

        let sources = stmt
            .query_map([], |row| {
                Ok(SourceInfo {
                    source: row.get(0)?,
                    chunk_count: row.get::<_, i64>(1)? as usize,
                    content_hash: row.get(2)?,
                })
            })
            .map_err(|e| FrankClawError::MemoryStore {
                msg: format!("list sources failed: {e}"),
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(sources)
    }

    async fn has_source(&self, source: &str) -> Result<bool> {
        let db = self.db.lock().map_err(|_| FrankClawError::MemoryStore {
            msg: "lock poisoned".into(),
        })?;

        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM memory_chunks WHERE source = ?1",
                params![source],
                |row| row.get(0),
            )
            .map_err(|e| FrankClawError::MemoryStore {
                msg: format!("has_source query failed: {e}"),
            })?;

        Ok(count > 0)
    }
}

/// Store a source content hash for change detection.
pub fn set_source_hash(
    db: &rusqlite::Connection,
    source: &str,
    hash: &str,
) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO memory_source_hashes (source, content_hash) VALUES (?1, ?2)",
        params![source, hash],
    )
    .map_err(|e| FrankClawError::MemoryStore {
        msg: format!("set source hash failed: {e}"),
    })?;
    Ok(())
}

/// Get stored hash for a source.
pub fn get_source_hash(db: &rusqlite::Connection, source: &str) -> Option<String> {
    db.query_row(
        "SELECT content_hash FROM memory_source_hashes WHERE source = ?1",
        params![source],
        |row| row.get(0),
    )
    .ok()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-10 {
        0.0
    } else {
        (dot / denom).max(0.0)
    }
}

fn f32_to_bytes(data: &[f32]) -> Vec<u8> {
    data.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_f32(data: &[u8]) -> Vec<f32> {
    data.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn store_and_search() {
        let store = SqliteMemoryStore::in_memory().unwrap();
        let chunk = ChunkEntry {
            id: "c1".into(),
            source: "test.md".into(),
            text: "Rust is a systems programming language".into(),
            line_start: 1,
            line_end: 1,
            chunk_index: 0,
            created_at: chrono::Utc::now(),
        };
        let embedding = vec![1.0, 0.0, 0.0];
        store.store_chunk(&chunk, &embedding).await.unwrap();

        let results = store
            .search_hybrid("rust programming", &[1.0, 0.0, 0.0], &SearchOptions::default())
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].chunk.id, "c1");
    }

    #[tokio::test]
    async fn delete_by_source_removes_chunks() {
        let store = SqliteMemoryStore::in_memory().unwrap();
        let chunk = ChunkEntry {
            id: "c1".into(),
            source: "file.md".into(),
            text: "Hello world".into(),
            line_start: 1,
            line_end: 1,
            chunk_index: 0,
            created_at: chrono::Utc::now(),
        };
        store.store_chunk(&chunk, &[1.0]).await.unwrap();
        assert!(store.has_source("file.md").await.unwrap());

        let deleted = store.delete_by_source("file.md").await.unwrap();
        assert_eq!(deleted, 1);
        assert!(!store.has_source("file.md").await.unwrap());
    }

    #[tokio::test]
    async fn list_sources() {
        let store = SqliteMemoryStore::in_memory().unwrap();
        for i in 0..3 {
            let chunk = ChunkEntry {
                id: format!("c{i}"),
                source: "src.md".into(),
                text: format!("Chunk {i}"),
                line_start: i + 1,
                line_end: i + 1,
                chunk_index: i,
                created_at: chrono::Utc::now(),
            };
            store.store_chunk(&chunk, &[1.0]).await.unwrap();
        }

        let sources = store.list_sources().await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source, "src.md");
        assert_eq!(sources[0].chunk_count, 3);
    }

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-5);
    }
}
