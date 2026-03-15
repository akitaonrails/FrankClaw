use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

/// How the gateway authenticates incoming connections.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AuthMode {
    /// No authentication. Only safe when bound to loopback.
    #[default]
    None,

    /// Bearer token (constant-time comparison).
    Token {
        /// The token is stored encrypted; this is the config reference.
        #[serde(
            serialize_with = "serialize_optional_secret_string",
            deserialize_with = "deserialize_optional_secret_string"
        )]
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

impl AuthMode {
    pub fn validate(&self) -> crate::error::Result<()> {
        match self {
            Self::Token { token } => {
                if token
                    .as_ref()
                    .map(|token| token.expose_secret().trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(crate::error::FrankClawError::ConfigValidation {
                        msg: "gateway.auth.mode=token requires a non-empty token".into(),
                    });
                }
            }
            Self::Password { hash } => {
                if hash.trim().is_empty() {
                    return Err(crate::error::FrankClawError::ConfigValidation {
                        msg: "gateway.auth.mode=password requires a non-empty hash".into(),
                    });
                }
            }
            Self::TrustedProxy { identity_header } => {
                if identity_header.trim().is_empty() {
                    return Err(crate::error::FrankClawError::ConfigValidation {
                        msg: "trusted_proxy auth requires an identity_header".into(),
                    });
                }
            }
            Self::None | Self::Tailscale => {}
        }

        Ok(())
    }
}

fn serialize_optional_secret_string<S>(
    value: &Option<SecretString>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(secret) => serializer.serialize_some(secret.expose_secret()),
        None => serializer.serialize_none(),
    }
}

fn deserialize_optional_secret_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<SecretString>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
        .map(|value| value.map(SecretString::from))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_mode_roundtrips_through_serde() {
        let auth = AuthMode::Token {
            token: Some(SecretString::from("secret-token")),
        };

        let json = serde_json::to_string(&auth).expect("token mode should serialize");
        let decoded: AuthMode =
            serde_json::from_str(&json).expect("token mode should deserialize");

        match decoded {
            AuthMode::Token { token } => {
                assert_eq!(
                    token.expect("token should be present").expose_secret(),
                    "secret-token"
                );
            }
            other => panic!("unexpected auth mode: {other:?}"),
        }
    }

    #[test]
    fn token_mode_requires_a_value() {
        let auth = AuthMode::Token { token: None };
        assert!(auth.validate().is_err());
    }
}
