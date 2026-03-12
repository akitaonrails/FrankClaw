use serde::{Deserialize, Serialize};
use std::fmt;

/// Maximum length for identifiers (agent, channel, account, session key).
/// Prevents memory exhaustion from maliciously long strings.
const MAX_ID_LEN: usize = 255;

/// Truncate a string to the maximum identifier length.
fn clamp_id(s: String) -> String {
    if s.len() <= MAX_ID_LEN {
        s
    } else {
        s[..MAX_ID_LEN].to_string()
    }
}

/// Strongly-typed channel identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelId(String);

impl ChannelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(clamp_id(id.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Strongly-typed agent identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(clamp_id(id.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn default_agent() -> Self {
        Self("default".to_string())
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Session key: `{agent_id}:{channel}:{account_id}`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionKey(String);

impl SessionKey {
    pub fn new(agent_id: &AgentId, channel: &ChannelId, account_id: &str) -> Self {
        Self(format!("{}:{}:{}", agent_id, channel, account_id))
    }

    /// Create a session key from a raw string.
    ///
    /// The key is clamped to a maximum of 800 bytes (3 components × 255 + separators).
    pub fn from_raw(key: impl Into<String>) -> Self {
        let k = key.into();
        if k.len() > 800 {
            Self(k[..800].to_string())
        } else {
            Self(k)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse back into components. Returns None if format is invalid.
    pub fn parse(&self) -> Option<(AgentId, ChannelId, String)> {
        let mut parts = self.0.splitn(3, ':');
        let agent = parts.next()?;
        let channel = parts.next()?;
        let account = parts.next()?;
        Some((
            AgentId::new(agent),
            ChannelId::new(channel),
            account.to_string(),
        ))
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Unique identifier for a WebSocket connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId(pub u64);

impl fmt::Display for ConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conn-{}", self.0)
    }
}

/// Request identifier for RPC correlation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    Text(String),
}

/// Role of a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Media identifier (UUID v4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaId(uuid::Uuid);

impl MediaId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn parse(value: &str) -> Option<Self> {
        uuid::Uuid::parse_str(value.trim()).ok().map(Self)
    }
}

impl Default for MediaId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MediaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_clamps_long_input() {
        let long = "x".repeat(1000);
        let id = AgentId::new(long);
        assert_eq!(id.as_str().len(), MAX_ID_LEN);
    }

    #[test]
    fn agent_id_preserves_normal_input() {
        let id = AgentId::new("default");
        assert_eq!(id.as_str(), "default");
    }

    #[test]
    fn channel_id_clamps_long_input() {
        let long = "c".repeat(1000);
        let id = ChannelId::new(long);
        assert_eq!(id.as_str().len(), MAX_ID_LEN);
    }

    #[test]
    fn session_key_from_raw_clamps_long_input() {
        let long = "k".repeat(5000);
        let key = SessionKey::from_raw(long);
        assert_eq!(key.as_str().len(), 800);
    }

    #[test]
    fn session_key_from_raw_preserves_normal_input() {
        let key = SessionKey::from_raw("agent:web:user123");
        assert_eq!(key.as_str(), "agent:web:user123");
    }

    #[test]
    fn session_key_parse_round_trips() {
        let agent = AgentId::new("a1");
        let channel = ChannelId::new("web");
        let key = SessionKey::new(&agent, &channel, "user42");
        let (a, c, acct) = key.parse().unwrap();
        assert_eq!(a.as_str(), "a1");
        assert_eq!(c.as_str(), "web");
        assert_eq!(acct, "user42");
    }

    #[test]
    fn session_key_parse_rejects_malformed() {
        let key = SessionKey::from_raw("no-colons");
        assert!(key.parse().is_none());
    }

    #[test]
    fn media_id_parse_rejects_invalid() {
        assert!(MediaId::parse("not-a-uuid").is_none());
        assert!(MediaId::parse("").is_none());
    }

    #[test]
    fn media_id_parse_accepts_valid_uuid() {
        assert!(MediaId::parse("550e8400-e29b-41d4-a716-446655440000").is_some());
    }
}
