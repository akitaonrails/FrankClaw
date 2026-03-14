use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::{AgentId, ChannelId, Role, SessionKey};

/// How sessions are scoped for an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SessionScoping {
    /// One session per sender (default).
    Main,
    /// Separate session per DM peer.
    PerPeer,
    /// Separate per channel + peer combination.
    #[default]
    PerChannelPeer,
    /// Single shared session across all senders.
    Global,
}


impl SessionScoping {
    pub fn resolve_inbound_account_scope(
        &self,
        account_id: &str,
        sender_id: &str,
        thread_id: Option<&str>,
        is_group: bool,
    ) -> String {
        match self {
            Self::Main => {
                if is_group {
                    thread_id.unwrap_or("main").to_string()
                } else {
                    "main".to_string()
                }
            }
            Self::PerPeer => {
                if is_group {
                    thread_id.unwrap_or(sender_id).to_string()
                } else {
                    sender_id.to_string()
                }
            }
            Self::PerChannelPeer => {
                let peer = if is_group {
                    thread_id.unwrap_or(sender_id)
                } else {
                    sender_id
                };
                format!("{account_id}:{peer}")
            }
            Self::Global => "global".to_string(),
        }
    }
}

/// Session reset policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResetPolicy {
    /// Reset daily at this UTC hour (0-23). None = no daily reset.
    pub daily_at_hour: Option<u8>,
    /// Reset after this many seconds of inactivity. None = no idle reset.
    pub idle_timeout_secs: Option<u64>,
    /// Maximum transcript entries before forced reset.
    pub max_entries: Option<usize>,
}

impl Default for SessionResetPolicy {
    fn default() -> Self {
        Self {
            daily_at_hour: None,
            idle_timeout_secs: None,
            max_entries: Some(500),
        }
    }
}

/// Pruning configuration for old sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruningConfig {
    /// Delete sessions older than this many days.
    pub max_age_days: u32,
    /// Maximum number of sessions to keep per agent.
    pub max_sessions_per_agent: usize,
    /// Maximum total disk usage per agent (bytes).
    pub disk_budget_bytes: u64,
}

impl Default for PruningConfig {
    fn default() -> Self {
        Self {
            max_age_days: 30,
            max_sessions_per_agent: 500,
            disk_budget_bytes: 10 * 1024 * 1024, // 10 MB
        }
    }
}

/// A session entry in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub key: SessionKey,
    pub agent_id: AgentId,
    pub channel: ChannelId,
    pub account_id: String,
    pub scoping: SessionScoping,
    pub created_at: DateTime<Utc>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub thread_id: Option<String>,
    pub metadata: serde_json::Value,
}

/// A single entry in a session transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub seq: u64,
    pub role: Role,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: Option<serde_json::Value>,
}

/// Abstract session storage backend.
#[async_trait]
pub trait SessionStore: Send + Sync + 'static {
    /// Get a session by key.
    async fn get(&self, key: &SessionKey) -> Result<Option<SessionEntry>>;

    /// Create or update a session.
    async fn upsert(&self, entry: &SessionEntry) -> Result<()>;

    /// Delete a session and its transcript.
    async fn delete(&self, key: &SessionKey) -> Result<()>;

    /// List sessions for an agent.
    async fn list(&self, agent_id: &AgentId, limit: usize, offset: usize) -> Result<Vec<SessionEntry>>;

    /// Append a transcript entry.
    async fn append_transcript(&self, key: &SessionKey, entry: &TranscriptEntry) -> Result<()>;

    /// Get transcript entries.
    async fn get_transcript(
        &self,
        key: &SessionKey,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<TranscriptEntry>>;

    /// Clear a session's transcript (reset).
    async fn clear_transcript(&self, key: &SessionKey) -> Result<()>;

    /// Run maintenance (pruning, disk budget enforcement).
    async fn maintenance(&self, config: &PruningConfig) -> Result<u64>;
}

#[cfg(test)]
mod tests {
    use super::SessionScoping;

    #[test]
    fn per_channel_peer_scoping_includes_account() {
        let scope = SessionScoping::PerChannelPeer.resolve_inbound_account_scope(
            "default",
            "user-123",
            None,
            false,
        );
        assert_eq!(scope, "default:user-123");
    }

    #[test]
    fn global_scoping_is_stable() {
        let scope = SessionScoping::Global.resolve_inbound_account_scope(
            "default",
            "user-123",
            Some("thread-1"),
            true,
        );
        assert_eq!(scope, "global");
    }

    #[test]
    fn group_scoping_prefers_thread_id() {
        let scope = SessionScoping::PerPeer.resolve_inbound_account_scope(
            "default",
            "user-123",
            Some("thread-1"),
            true,
        );
        assert_eq!(scope, "thread-1");
    }
}
