use secrecy::SecretString;
use serde::{Deserialize, Serialize};

/// How the gateway authenticates incoming connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AuthMode {
    /// No authentication. Only safe when bound to loopback.
    None,

    /// Bearer token (constant-time comparison).
    Token {
        /// The token is stored encrypted; this is the config reference.
        #[serde(skip)]
        token: Option<SecretString>,
    },

    /// Password verified with Argon2id.
    Password {
        /// PHC-format Argon2id hash string.
        hash: String,
    },

    /// Trust identity from reverse proxy headers.
    TrustedProxy {
        /// Header containing the authenticated identity.
        identity_header: String,
    },

    /// Tailscale-verified identity (via whois API).
    Tailscale,
}

impl Default for AuthMode {
    fn default() -> Self {
        Self::None
    }
}

/// Authorization role for a connected client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthRole {
    /// View-only access (read sessions, config, logs).
    Viewer,
    /// Can send messages and manage sessions.
    Editor,
    /// Full administrative access.
    Admin,
    /// AI-only: can execute agent turns but not manage config.
    AiOnly,
    /// Device node: mobile/desktop app with limited scope.
    Node,
}

/// Rate limiter configuration for auth attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum failed attempts before lockout.
    pub max_attempts: u32,
    /// Window duration in seconds.
    pub window_secs: u64,
    /// Lockout duration in seconds after max attempts.
    pub lockout_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            window_secs: 60,
            lockout_secs: 300,
        }
    }
}
