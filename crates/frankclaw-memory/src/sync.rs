use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use frankclaw_core::error::{FrankClawError, Result};

use crate::chunking::chunk_text;
use crate::embedding::EmbeddingProvider;
use crate::{ChunkEntry, MemoryStore};

/// Syncs files from a directory into the memory store,
/// re-chunking and re-embedding only changed files.
pub struct MemorySyncer {
    store: Arc<dyn MemoryStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    memory_dir: PathBuf,
    chunk_size: usize,
}

impl MemorySyncer {
    pub fn new(
        store: Arc<dyn MemoryStore>,
        embedder: Arc<dyn EmbeddingProvider>,
        memory_dir: PathBuf,
        chunk_size: usize,
    ) -> Self {
        Self {
            store,
            embedder,
            memory_dir,
            chunk_size,
        }
    }

    /// Scan the memory directory and sync changed files.
    pub async fn sync_once(&self) -> Result<SyncReport> {
        let mut report = SyncReport::default();

        if !self.memory_dir.exists() {
            debug!(dir = %self.memory_dir.display(), "memory directory does not exist, skipping sync");
            return Ok(report);
        }

        let files = scan_files(&self.memory_dir)?;
        let existing_sources = self.store.list_sources().await?;
        let existing_map: std::collections::HashMap<String, String> = existing_sources
            .into_iter()
            .map(|s| (s.source, s.content_hash))
            .collect();

        for file_path in &files {
            let source = file_source(&self.memory_dir, file_path);
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(e) => {
                    warn!(file = %file_path.display(), error = %e, "failed to read memory file");
                    report.errors += 1;
                    continue;
                }
            };

            let hash = content_hash(&content);

            if let Some(existing_hash) = existing_map.get(&source) {
                if *existing_hash == hash {
                    report.skipped += 1;
                    continue;
                }
            }

            // File is new or changed — re-index.
            info!(source = %source, "re-indexing memory file");

            // Delete old chunks.
            self.store.delete_by_source(&source).await?;

            // Chunk the content.
            let chunks = chunk_text(&content, self.chunk_size);
            if chunks.is_empty() {
                report.skipped += 1;
                continue;
            }

            // Embed all chunks.
            let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
            let embeddings = self.embedder.embed_batch(&texts).await?;

            // Store chunks.
            for (i, chunk) in chunks.iter().enumerate() {
                let id = format!("{source}:{}", chunk.index);
                let entry = ChunkEntry {
                    id,
                    source: source.clone(),
                    text: chunk.text.clone(),
                    line_start: chunk.line_start,
                    line_end: chunk.line_end,
                    chunk_index: chunk.index,
                    created_at: chrono::Utc::now(),
                };
                let embedding = embeddings.get(i).map(|e| e.as_slice()).unwrap_or(&[]);
                self.store.store_chunk(&entry, embedding).await?;
            }

            report.indexed += 1;
        }

        // Find removed sources (in store but not on disk).
        let disk_sources: std::collections::HashSet<String> = files
            .iter()
            .map(|f| file_source(&self.memory_dir, f))
            .collect();
        for (source, _) in &existing_map {
            if !disk_sources.contains(source) {
                info!(source = %source, "removing deleted memory source");
                self.store.delete_by_source(source).await?;
                report.removed += 1;
            }
        }

        info!(
            indexed = report.indexed,
            skipped = report.skipped,
            removed = report.removed,
            errors = report.errors,
            "memory sync complete"
        );

        Ok(report)
    }
}

/// Report of a sync operation.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub indexed: usize,
    pub skipped: usize,
    pub removed: usize,
    pub errors: usize,
}

fn scan_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| FrankClawError::MemoryStore {
        msg: format!("failed to read memory directory: {e}"),
    })?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_dir() {
            // Recurse into subdirectories.
            files.extend(scan_files(&path)?);
        } else if is_text_file(&path) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn is_text_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "md" | "txt" | "rs" | "py" | "js" | "ts" | "json" | "yaml" | "yml" | "toml" | "cfg"
            | "ini" | "csv" | "html" | "xml" | "sh" | "bash" | "zsh"
    )
}

fn file_source(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_deterministic() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn content_hash_different() {
        let h1 = content_hash("hello");
        let h2 = content_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn is_text_file_recognizes_common_extensions() {
        assert!(is_text_file(Path::new("notes.md")));
        assert!(is_text_file(Path::new("main.rs")));
        assert!(is_text_file(Path::new("config.toml")));
        assert!(!is_text_file(Path::new("image.png")));
        assert!(!is_text_file(Path::new("binary.exe")));
    }

    #[test]
    fn file_source_strips_base() {
        let base = Path::new("/home/user/memory");
        let path = Path::new("/home/user/memory/notes/daily.md");
        assert_eq!(file_source(base, path), "notes/daily.md");
    }
}
