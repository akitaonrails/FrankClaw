use async_trait::async_trait;
use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use tracing::{debug, warn};

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::session::{
    PruningConfig, SessionEntry, SessionStore, TranscriptEntry,
};
use frankclaw_core::types::{AgentId, SessionKey};
use frankclaw_crypto::{decrypt, derive_subkey, encrypt, MasterKey};

use crate::migrations;

/// Maximum size of a single transcript entry content (1 MB).
const MAX_TRANSCRIPT_ENTRY_BYTES: usize = 1_024 * 1_024;

/// SQLite-backed session store with optional encryption at rest.
///
/// All transcript content is encrypted with a session-derived key when
/// `encryption_key` is Some. Session metadata (keys, timestamps, agent IDs)
/// is NOT encrypted — it's needed for indexed lookups.
pub struct SqliteSessionStore {
    pool: Pool<SqliteConnectionManager>,
    encryption_key: Option<[u8; 32]>,
}

impl SqliteSessionStore {
    /// Open or create the session database.
    ///
    /// - `path`: SQLite file path. Created if it doesn't exist.
    /// - `master_key`: If provided, derives a session encryption subkey.
    pub fn open(
        path: &std::path::Path,
        master_key: Option<&MasterKey>,
    ) -> Result<Self> {
        // Ensure parent directory exists with restricted permissions.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FrankClawError::SessionStorage {
                msg: format!("failed to create session directory: {e}"),
            })?;

            // Set directory permissions to owner-only (Unix).
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o700);
                let _ = std::fs::set_permissions(parent, perms);
            }
        }

        let manager = SqliteConnectionManager::file(path);
        let pool = Pool::builder()
            .max_size(8)
            .connection_timeout(std::time::Duration::from_secs(5))
            .build(manager)
            .map_err(|e| FrankClawError::SessionStorage {
                msg: format!("connection pool error: {e}"),
            })?;

        // Run migrations on a fresh connection.
        {
            let conn = pool.get().map_err(|e| FrankClawError::SessionStorage {
                msg: format!("migration connection error: {e}"),
            })?;
            migrations::run_migrations(&conn).map_err(|e| FrankClawError::SessionStorage {
                msg: format!("migration error: {e}"),
            })?;

            // Set file permissions to owner-only.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                let _ = std::fs::set_permissions(path, perms);
            }
        }

        let encryption_key = master_key
            .map(|mk| derive_subkey(mk, "session"))
            .transpose()
            .map_err(FrankClawError::Crypto)?;

        Ok(Self {
            pool,
            encryption_key,
        })
    }

    /// Encrypt content if encryption is enabled, otherwise return raw bytes.
    fn encrypt_content(&self, content: &str) -> Result<Vec<u8>> {
        match &self.encryption_key {
            Some(key) => {
                let blob = encrypt(key, content.as_bytes())
                    .map_err(FrankClawError::Crypto)?;
                serde_json::to_vec(&blob).map_err(|e| FrankClawError::SessionStorage {
                    msg: format!("encryption serialization error: {e}"),
                })
            }
            None => Ok(content.as_bytes().to_vec()),
        }
    }

    /// Decrypt content if encryption is enabled, otherwise return raw string.
    fn decrypt_content(
        encryption_key: Option<&[u8; 32]>,
        data: &[u8],
    ) -> Result<String> {
        match encryption_key {
            Some(key) => {
                let blob: frankclaw_crypto::EncryptedBlob =
                    serde_json::from_slice(data).map_err(|e| FrankClawError::SessionStorage {
                        msg: format!(
                            "transcript data is not valid encrypted blob (was it written without encryption?): {e}"
                        ),
                    })?;
                let plaintext = decrypt(key, &blob).map_err(|e| {
                    warn!("transcript decryption failed — if the master key was rotated, existing encrypted transcripts cannot be read with the new key");
                    FrankClawError::SessionStorage {
                        msg: format!(
                            "transcript decryption failed (possible key rotation): {e}"
                        ),
                    }
                })?;
                String::from_utf8(plaintext).map_err(|e| FrankClawError::SessionStorage {
                    msg: format!("invalid UTF-8 in transcript: {e}"),
                })
            }
            None => String::from_utf8(data.to_vec()).map_err(|e| FrankClawError::SessionStorage {
                msg: format!("invalid UTF-8 in transcript: {e}"),
            }),
        }
    }

    fn get_conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>> {
        self.pool.get().map_err(|e| FrankClawError::SessionStorage {
            msg: format!("pool error: {e}"),
        })
    }

    pub async fn rewrite_last_assistant_message(
        &self,
        key: &SessionKey,
        content: &str,
    ) -> Result<bool> {
        let conn = self.get_conn()?;
        let key_str = key.as_str().to_string();
        let encrypted_content = self.encrypt_content(content)?;
        let role = serde_json::to_string(&frankclaw_core::types::Role::Assistant)
            .unwrap_or_else(|_| "\"assistant\"".to_string());

        tokio::task::spawn_blocking(move || {
            let seq = conn
                .query_row(
                    "SELECT seq FROM transcript
                     WHERE session_key = ?1 AND role = ?2
                     ORDER BY seq DESC
                     LIMIT 1",
                    params![key_str, role],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            let Some(seq) = seq else {
                return Ok(false);
            };

            conn.execute(
                "UPDATE transcript SET content = ?1, timestamp = ?2
                 WHERE session_key = ?3 AND seq = ?4",
                params![
                    encrypted_content,
                    Utc::now().to_rfc3339(),
                    key_str,
                    seq,
                ],
            )
            .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            Ok(true)
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn get(&self, key: &SessionKey) -> Result<Option<SessionEntry>> {
        let conn = self.get_conn()?;
        let key_str = key.as_str().to_string();

        tokio::task::spawn_blocking(move || {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT key, agent_id, channel, account_id, scoping, thread_id,
                            metadata, created_at, last_message_at
                     FROM sessions WHERE key = ?1",
                )
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            let result = stmt
                .query_row(params![key_str], |row| {
                    Ok(SessionEntry {
                        key: SessionKey::from_raw(row.get::<_, String>(0)?),
                        agent_id: AgentId::new(row.get::<_, String>(1)?),
                        channel: frankclaw_core::types::ChannelId::new(row.get::<_, String>(2)?),
                        account_id: row.get(3)?,
                        scoping: serde_json::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or_default(),
                        thread_id: row.get(5)?,
                        metadata: serde_json::from_str(&row.get::<_, String>(6)?)
                            .unwrap_or_default(),
                        created_at: row
                            .get::<_, String>(7)?
                            .parse()
                            .unwrap_or_else(|_| Utc::now()),
                        last_message_at: row
                            .get::<_, Option<String>>(8)?
                            .and_then(|s| s.parse().ok()),
                    })
                })
                .optional()
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            Ok(result)
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }

    async fn upsert(&self, entry: &SessionEntry) -> Result<()> {
        let conn = self.get_conn()?;
        let entry = entry.clone();

        tokio::task::spawn_blocking(move || {
            conn.execute(
                "INSERT INTO sessions (key, agent_id, channel, account_id, scoping,
                                       thread_id, metadata, created_at, last_message_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(key) DO UPDATE SET
                    last_message_at = excluded.last_message_at,
                    metadata = excluded.metadata,
                    thread_id = excluded.thread_id",
                params![
                    entry.key.as_str(),
                    entry.agent_id.as_str(),
                    entry.channel.as_str(),
                    entry.account_id,
                    serde_json::to_string(&entry.scoping).unwrap_or_default(),
                    entry.thread_id,
                    serde_json::to_string(&entry.metadata).unwrap_or_default(),
                    entry.created_at.to_rfc3339(),
                    entry.last_message_at.map(|t| t.to_rfc3339()),
                ],
            )
            .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            Ok(())
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }

    async fn delete(&self, key: &SessionKey) -> Result<()> {
        let conn = self.get_conn()?;
        let key_str = key.as_str().to_string();

        tokio::task::spawn_blocking(move || {
            // CASCADE deletes transcript entries.
            conn.execute("DELETE FROM sessions WHERE key = ?1", params![key_str])
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;
            Ok(())
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }

    async fn list(
        &self,
        agent_id: &AgentId,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SessionEntry>> {
        let conn = self.get_conn()?;
        let agent = agent_id.as_str().to_string();

        tokio::task::spawn_blocking(move || {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT key, agent_id, channel, account_id, scoping, thread_id,
                            metadata, created_at, last_message_at
                     FROM sessions
                     WHERE agent_id = ?1
                     ORDER BY last_message_at DESC NULLS LAST
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            let rows = stmt
                .query_map(params![agent, limit as i64, offset as i64], |row| {
                    Ok(SessionEntry {
                        key: SessionKey::from_raw(row.get::<_, String>(0)?),
                        agent_id: AgentId::new(row.get::<_, String>(1)?),
                        channel: frankclaw_core::types::ChannelId::new(row.get::<_, String>(2)?),
                        account_id: row.get(3)?,
                        scoping: serde_json::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or_default(),
                        thread_id: row.get(5)?,
                        metadata: serde_json::from_str(&row.get::<_, String>(6)?)
                            .unwrap_or_default(),
                        created_at: row
                            .get::<_, String>(7)?
                            .parse()
                            .unwrap_or_else(|_| Utc::now()),
                        last_message_at: row
                            .get::<_, Option<String>>(8)?
                            .and_then(|s| s.parse().ok()),
                    })
                })
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            let mut entries = Vec::new();
            for row in rows {
                entries
                    .push(row.map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?);
            }
            Ok(entries)
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }

    async fn append_transcript(&self, key: &SessionKey, entry: &TranscriptEntry) -> Result<()> {
        if entry.content.len() > MAX_TRANSCRIPT_ENTRY_BYTES {
            return Err(FrankClawError::InvalidRequest {
                msg: format!(
                    "transcript entry exceeds maximum size ({} bytes > {} limit)",
                    entry.content.len(),
                    MAX_TRANSCRIPT_ENTRY_BYTES,
                ),
            });
        }
        let conn = self.get_conn()?;
        let key_str = key.as_str().to_string();
        let encrypted_content = self.encrypt_content(&entry.content)?;
        let seq = entry.seq;
        let role = serde_json::to_string(&entry.role).unwrap_or_default();
        let metadata = entry.metadata.as_ref().map(|m| serde_json::to_string(m).unwrap_or_default());
        let timestamp = entry.timestamp.to_rfc3339();

        tokio::task::spawn_blocking(move || {
            conn.execute(
                "INSERT INTO transcript (session_key, seq, role, content, metadata, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![key_str, seq as i64, role, encrypted_content, metadata, timestamp],
            )
            .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            // Update session's last_message_at.
            conn.execute(
                "UPDATE sessions SET last_message_at = ?1 WHERE key = ?2",
                params![timestamp, key_str],
            )
            .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            Ok(())
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }

    async fn get_transcript(
        &self,
        key: &SessionKey,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<TranscriptEntry>> {
        let conn = self.get_conn()?;
        let key_str = key.as_str().to_string();
        let encryption_key = self.encryption_key;

        tokio::task::spawn_blocking(move || {
            let (sql, seq_val) = match before_seq {
                Some(seq) => (
                    "SELECT seq, role, content, metadata, timestamp
                     FROM transcript
                     WHERE session_key = ?1 AND seq < ?2
                     ORDER BY seq DESC
                     LIMIT ?3",
                    Some(seq as i64),
                ),
                None => (
                    "SELECT seq, role, content, metadata, timestamp
                     FROM transcript
                     WHERE session_key = ?1
                     ORDER BY seq DESC
                     LIMIT ?2",
                    None,
                ),
            };

            let mut stmt = conn
                .prepare_cached(sql)
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            type RowTuple = (i64, String, Vec<u8>, Option<String>, String);

            let extract_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<RowTuple> {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            };

            let raw_rows: Vec<RowTuple> = if let Some(seq) = seq_val {
                let mapped = stmt.query_map(params![key_str, seq, limit as i64], extract_row)
                    .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;
                mapped.collect::<rusqlite::Result<Vec<_>>>()
            } else {
                let mapped = stmt.query_map(params![key_str, limit as i64], extract_row)
                    .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;
                mapped.collect::<rusqlite::Result<Vec<_>>>()
            }
            .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            let mut entries = Vec::new();
            for (seq, role_str, content_bytes, metadata_str, timestamp_str) in raw_rows {
                let content = Self::decrypt_content(encryption_key.as_ref(), &content_bytes)?;

                entries.push(TranscriptEntry {
                    seq: seq as u64,
                    role: serde_json::from_str::<frankclaw_core::types::Role>(&role_str)
                        .unwrap_or(frankclaw_core::types::Role::User),
                    content,
                    metadata: metadata_str.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                    timestamp: timestamp_str.parse().unwrap_or_else(|_| Utc::now()),
                });
            }

            // Reverse so they're in ascending order.
            entries.reverse();
            Ok(entries)
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }

    async fn clear_transcript(&self, key: &SessionKey) -> Result<()> {
        let conn = self.get_conn()?;
        let key_str = key.as_str().to_string();

        tokio::task::spawn_blocking(move || {
            conn.execute(
                "DELETE FROM transcript WHERE session_key = ?1",
                params![key_str],
            )
            .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;
            Ok(())
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }

    async fn maintenance(&self, config: &PruningConfig) -> Result<u64> {
        let conn = self.get_conn()?;
        let max_age_days = config.max_age_days;
        let max_sessions = config.max_sessions_per_agent;

        tokio::task::spawn_blocking(move || {
            let cutoff = Utc::now() - chrono::Duration::days(max_age_days as i64);
            let cutoff_str = cutoff.to_rfc3339();

            // Delete sessions older than max_age_days.
            let deleted = conn
                .execute(
                    "DELETE FROM sessions WHERE last_message_at < ?1 OR
                     (last_message_at IS NULL AND created_at < ?1)",
                    params![cutoff_str],
                )
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            if deleted > 0 {
                debug!(deleted, "pruned old sessions");
            }

            // Enforce per-agent session limit: keep most recent N.
            let overflow = conn
                .execute(
                    "DELETE FROM sessions WHERE key IN (
                        SELECT s.key FROM sessions s
                        WHERE (
                            SELECT COUNT(*) FROM sessions s2
                            WHERE s2.agent_id = s.agent_id
                              AND COALESCE(s2.last_message_at, s2.created_at) >=
                                  COALESCE(s.last_message_at, s.created_at)
                        ) > ?1
                    )",
                    params![max_sessions as i64],
                )
                .map_err(|e| FrankClawError::SessionStorage { msg: e.to_string() })?;

            if overflow > 0 {
                warn!(overflow, max_sessions, "pruned sessions exceeding per-agent limit");
            }

            Ok((deleted + overflow) as u64)
        })
        .await
        .map_err(|e| FrankClawError::SessionStorage {
            msg: format!("task join error: {e}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::session::{SessionEntry, SessionScoping, SessionStore, TranscriptEntry};
    use frankclaw_core::types::{AgentId, ChannelId, Role, SessionKey};

    #[tokio::test]
    async fn rewrite_last_assistant_message_updates_latest_assistant_turn() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");
        let path = temp_dir.join("sessions.db");
        let store = SqliteSessionStore::open(&path, None).expect("store should open");
        let key = SessionKey::from_raw("agent:main:test");

        store
            .upsert(&SessionEntry {
                key: key.clone(),
                agent_id: AgentId::default_agent(),
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                scoping: SessionScoping::PerChannelPeer,
                created_at: Utc::now(),
                last_message_at: Some(Utc::now()),
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("session should upsert");
        store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: "hello".into(),
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("user transcript should append");
        store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 2,
                    role: Role::Assistant,
                    content: "old".into(),
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("assistant transcript should append");

        let updated = store
            .rewrite_last_assistant_message(&key, "new")
            .await
            .expect("assistant rewrite should succeed");
        assert!(updated);

        let transcript = store
            .get_transcript(&key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[1].content, "new");

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn rewrite_last_assistant_message_returns_false_when_session_has_no_assistant_turn() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-test-no-assistant-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");
        let path = temp_dir.join("sessions.db");
        let store = SqliteSessionStore::open(&path, None).expect("store should open");
        let key = SessionKey::from_raw("agent:main:test-no-assistant");

        store
            .upsert(&SessionEntry {
                key: key.clone(),
                agent_id: AgentId::default_agent(),
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                scoping: SessionScoping::PerChannelPeer,
                created_at: Utc::now(),
                last_message_at: Some(Utc::now()),
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("session should upsert");
        store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: "hello".into(),
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("user transcript should append");

        let updated = store
            .rewrite_last_assistant_message(&key, "new")
            .await
            .expect("rewrite should succeed");
        assert!(!updated);

        let transcript = store
            .get_transcript(&key, 10, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].content, "hello");

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn concurrent_transcript_appends_are_serialized() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-concurrent-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");
        let path = temp_dir.join("sessions.db");
        let store = std::sync::Arc::new(
            SqliteSessionStore::open(&path, None).expect("store should open"),
        );
        let key = SessionKey::from_raw("agent:main:concurrent");

        store
            .upsert(&SessionEntry {
                key: key.clone(),
                agent_id: AgentId::default_agent(),
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                scoping: SessionScoping::PerChannelPeer,
                created_at: Utc::now(),
                last_message_at: Some(Utc::now()),
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("session should upsert");

        // Spawn 20 concurrent appends.
        let mut handles = Vec::new();
        for i in 0..20u64 {
            let store = store.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                store
                    .append_transcript(
                        &key,
                        &TranscriptEntry {
                            seq: i + 1,
                            role: Role::User,
                            content: format!("message-{i}"),
                            timestamp: Utc::now(),
                            metadata: None,
                        },
                    )
                    .await
                    .expect("append should succeed");
            }));
        }
        for handle in handles {
            handle.await.expect("task should complete");
        }

        let transcript = store
            .get_transcript(&key, 100, None)
            .await
            .expect("transcript should load");
        assert_eq!(transcript.len(), 20, "all 20 appends should be persisted");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn transcript_entry_size_limit_rejects_oversized_content() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-size-limit-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");
        let path = temp_dir.join("sessions.db");
        let store = SqliteSessionStore::open(&path, None).expect("store should open");
        let key = SessionKey::from_raw("agent:main:size-limit");

        store
            .upsert(&SessionEntry {
                key: key.clone(),
                agent_id: AgentId::default_agent(),
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                scoping: SessionScoping::PerChannelPeer,
                created_at: Utc::now(),
                last_message_at: Some(Utc::now()),
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("session should upsert");

        // Content just over 1MB should be rejected.
        let oversized = "x".repeat(MAX_TRANSCRIPT_ENTRY_BYTES + 1);
        let err = store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: oversized,
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("maximum size"));

        // Content at exactly 1MB should succeed.
        let just_right = "x".repeat(MAX_TRANSCRIPT_ENTRY_BYTES);
        store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: just_right,
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("1MB content should be accepted");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn encrypted_transcript_with_wrong_key_gives_clear_error() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-keyrot-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");
        let path = temp_dir.join("sessions.db");

        let key1 = MasterKey::from_bytes([1u8; 32]);
        let store1 = SqliteSessionStore::open(&path, Some(&key1)).expect("store should open");
        let session_key = SessionKey::from_raw("agent:main:keyrot");

        store1
            .upsert(&SessionEntry {
                key: session_key.clone(),
                agent_id: AgentId::default_agent(),
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                scoping: SessionScoping::PerChannelPeer,
                created_at: Utc::now(),
                last_message_at: Some(Utc::now()),
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("session should upsert");
        store1
            .append_transcript(
                &session_key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: "secret".into(),
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("encrypted append should work");

        // Open with a different key — reading should fail with clear error.
        let key2 = MasterKey::from_bytes([2u8; 32]);
        let store2 = SqliteSessionStore::open(&path, Some(&key2)).expect("store should open");
        let err = store2
            .get_transcript(&session_key, 10, None)
            .await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("key rotation") || msg.contains("decryption failed"),
            "error should mention key rotation: {msg}"
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    /// Helper to create a temp store for tests.
    fn temp_store(suffix: &str) -> (std::path::PathBuf, SqliteSessionStore) {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-{suffix}-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        let path = temp_dir.join("sessions.db");
        let store = SqliteSessionStore::open(&path, None).expect("store should open");
        (temp_dir, store)
    }

    fn test_session(key: &str) -> SessionEntry {
        SessionEntry {
            key: SessionKey::from_raw(key),
            agent_id: AgentId::default_agent(),
            channel: ChannelId::new("web"),
            account_id: "default".into(),
            scoping: SessionScoping::PerChannelPeer,
            created_at: Utc::now(),
            last_message_at: Some(Utc::now()),
            thread_id: None,
            metadata: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_session() {
        let (dir, store) = temp_store("get-none");
        let result = store
            .get(&SessionKey::from_raw("nonexistent"))
            .await
            .expect("get should succeed");
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn upsert_then_get_roundtrip() {
        let (dir, store) = temp_store("upsert-get");
        let entry = test_session("agent:main:roundtrip");
        store.upsert(&entry).await.expect("upsert");
        let loaded = store
            .get(&entry.key)
            .await
            .expect("get")
            .expect("should exist");
        assert_eq!(loaded.key.as_str(), entry.key.as_str());
        assert_eq!(loaded.agent_id.as_str(), entry.agent_id.as_str());
        assert_eq!(loaded.channel.as_str(), "web");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn upsert_updates_existing_session() {
        let (dir, store) = temp_store("upsert-update");
        let mut entry = test_session("agent:main:update");
        store.upsert(&entry).await.expect("initial upsert");

        entry.thread_id = Some("thread-1".into());
        entry.metadata = serde_json::json!({"updated": true});
        store.upsert(&entry).await.expect("update upsert");

        let loaded = store.get(&entry.key).await.expect("get").expect("exists");
        assert_eq!(loaded.thread_id.as_deref(), Some("thread-1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn delete_removes_session_and_transcript() {
        let (dir, store) = temp_store("delete");
        let key = SessionKey::from_raw("agent:main:delete");
        store.upsert(&test_session("agent:main:delete")).await.expect("upsert");
        store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: "hello".into(),
                    timestamp: Utc::now(),
                    metadata: None,
                },
            )
            .await
            .expect("append");

        store.delete(&key).await.expect("delete");

        assert!(store.get(&key).await.expect("get").is_none());
        let transcript = store.get_transcript(&key, 100, None).await.expect("transcript");
        assert!(transcript.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn list_returns_sessions_for_agent() {
        let (dir, store) = temp_store("list");
        store.upsert(&test_session("agent:main:a")).await.expect("upsert a");
        store.upsert(&test_session("agent:main:b")).await.expect("upsert b");
        store.upsert(&test_session("agent:main:c")).await.expect("upsert c");

        let all = store
            .list(&AgentId::default_agent(), 10, 0)
            .await
            .expect("list");
        assert_eq!(all.len(), 3);

        // Test pagination
        let page = store
            .list(&AgentId::default_agent(), 2, 0)
            .await
            .expect("page 1");
        assert_eq!(page.len(), 2);

        let page2 = store
            .list(&AgentId::default_agent(), 2, 2)
            .await
            .expect("page 2");
        assert_eq!(page2.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn clear_transcript_removes_entries() {
        let (dir, store) = temp_store("clear");
        let key = SessionKey::from_raw("agent:main:clear");
        store.upsert(&test_session("agent:main:clear")).await.expect("upsert");
        for i in 1..=5 {
            store
                .append_transcript(
                    &key,
                    &TranscriptEntry {
                        seq: i,
                        role: Role::User,
                        content: format!("msg-{i}"),
                        timestamp: Utc::now(),
                        metadata: None,
                    },
                )
                .await
                .expect("append");
        }

        store.clear_transcript(&key).await.expect("clear");
        let transcript = store.get_transcript(&key, 100, None).await.expect("get");
        assert!(transcript.is_empty());

        // Session itself should still exist
        assert!(store.get(&key).await.expect("get").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn get_transcript_pagination_with_before_seq() {
        let (dir, store) = temp_store("pagination");
        let key = SessionKey::from_raw("agent:main:paginate");
        store.upsert(&test_session("agent:main:paginate")).await.expect("upsert");
        for i in 1..=10 {
            store
                .append_transcript(
                    &key,
                    &TranscriptEntry {
                        seq: i,
                        role: if i % 2 == 1 { Role::User } else { Role::Assistant },
                        content: format!("msg-{i}"),
                        timestamp: Utc::now(),
                        metadata: None,
                    },
                )
                .await
                .expect("append");
        }

        // Get entries before seq 6 (should return 1-5)
        let page = store.get_transcript(&key, 100, Some(6)).await.expect("page");
        assert_eq!(page.len(), 5);
        assert_eq!(page[0].seq, 1);
        assert_eq!(page[4].seq, 5);

        // Get last 3 entries before seq 6
        let page = store.get_transcript(&key, 3, Some(6)).await.expect("page");
        assert_eq!(page.len(), 3);
        assert_eq!(page[0].seq, 3);
        assert_eq!(page[2].seq, 5);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn encrypted_roundtrip_with_same_key() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-enc-roundtrip-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        let path = temp_dir.join("sessions.db");
        let master = MasterKey::from_bytes([42u8; 32]);
        let store = SqliteSessionStore::open(&path, Some(&master)).expect("open");
        let key = SessionKey::from_raw("agent:main:encrypted");

        store.upsert(&test_session("agent:main:encrypted")).await.expect("upsert");
        store
            .append_transcript(
                &key,
                &TranscriptEntry {
                    seq: 1,
                    role: Role::User,
                    content: "encrypted content 🔐".into(),
                    timestamp: Utc::now(),
                    metadata: Some(serde_json::json!({"tool": "bash"})),
                },
            )
            .await
            .expect("append");

        let transcript = store.get_transcript(&key, 10, None).await.expect("get");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].content, "encrypted content 🔐");
        assert_eq!(transcript[0].metadata.as_ref().unwrap()["tool"], "bash");
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn maintenance_prunes_old_sessions() {
        let (dir, store) = temp_store("maintenance");
        let old_time = Utc::now() - chrono::Duration::days(60);
        let mut old_entry = test_session("agent:main:old");
        old_entry.created_at = old_time;
        old_entry.last_message_at = Some(old_time);
        store.upsert(&old_entry).await.expect("upsert old");

        let new_entry = test_session("agent:main:new");
        store.upsert(&new_entry).await.expect("upsert new");

        let pruned = store
            .maintenance(&PruningConfig {
                max_age_days: 30,
                max_sessions_per_agent: 1000,
                disk_budget_bytes: 100_000_000,
            })
            .await
            .expect("maintenance");

        assert!(pruned >= 1, "should prune at least the old session");

        // Old session should be gone, new should remain
        assert!(store.get(&old_entry.key).await.expect("get old").is_none());
        assert!(store.get(&new_entry.key).await.expect("get new").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn empty_transcript_retrieval() {
        let (dir, store) = temp_store("empty-transcript");
        let key = SessionKey::from_raw("agent:main:empty");
        store.upsert(&test_session("agent:main:empty")).await.expect("upsert");

        let transcript = store.get_transcript(&key, 100, None).await.expect("get");
        assert!(transcript.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn migration_is_idempotent() {
        let temp_dir = std::env::temp_dir().join(format!(
            "frankclaw-sessions-idempotent-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");
        let path = temp_dir.join("sessions.db");

        // Open twice — second open runs migrations again.
        let _store1 = SqliteSessionStore::open(&path, None).expect("first open should succeed");
        let store2 = SqliteSessionStore::open(&path, None).expect("second open should succeed");

        // Verify it works by doing a write.
        let key = SessionKey::from_raw("agent:main:idempotent");
        store2
            .upsert(&SessionEntry {
                key: key.clone(),
                agent_id: AgentId::default_agent(),
                channel: ChannelId::new("web"),
                account_id: "default".into(),
                scoping: SessionScoping::PerChannelPeer,
                created_at: Utc::now(),
                last_message_at: Some(Utc::now()),
                thread_id: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("upsert should work after re-migration");

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}

/// Extension trait for `rusqlite::Result<Option<T>>`.
trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
