use std::net::SocketAddr;

use frankclaw_core::auth::{AuthMode, AuthRole};
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_crypto::{verify_password, verify_token_eq};
use secrecy::{ExposeSecret, SecretString};

use crate::rate_limit::AuthRateLimiter;

/// Authenticate an incoming connection.
///
/// Returns the authenticated role on success.
/// Records failures in the rate limiter to prevent brute force.
pub fn authenticate(
    mode: &AuthMode,
    provided: &AuthCredential,
    remote_addr: Option<&SocketAddr>,
    rate_limiter: &AuthRateLimiter,
) -> Result<AuthRole> {
    // Check rate limit first.
    if let Some(addr) = remote_addr {
        if let Some(remaining) = rate_limiter.is_locked(&addr.ip()) {
            return Err(FrankClawError::RateLimited {
                retry_after_secs: remaining.as_secs(),
            });
        }
    }

    let result = match (mode, provided) {
        // No auth required — only safe on loopback.
        (AuthMode::None, _) => Ok(AuthRole::Admin),

        // Token-based auth.
        (AuthMode::Token { token: Some(expected) }, AuthCredential::BearerToken(provided)) => {
            if verify_token_eq(provided.expose_secret(), expected.expose_secret()) {
                Ok(AuthRole::Admin)
            } else {
                Err(FrankClawError::AuthFailed)
            }
        }

        // Password-based auth.
        (AuthMode::Password { hash }, AuthCredential::Password(provided)) => {
            let stored = frankclaw_crypto::PasswordHash::from_stored(hash.clone());
            match verify_password(provided, &stored) {
                Ok(true) => Ok(AuthRole::Admin),
                Ok(false) => Err(FrankClawError::AuthFailed),
                Err(_) => Err(FrankClawError::AuthFailed),
            }
        }

        // Trusted proxy: identity from header.
        (AuthMode::TrustedProxy { .. }, AuthCredential::ProxyIdentity(identity)) => {
            if identity.is_empty() {
                Err(FrankClawError::AuthFailed)
            } else {
                // Trusted proxy provides identity; we accept it.
                // The proxy is responsible for authentication.
                Ok(AuthRole::Editor)
            }
        }

        // Tailscale: verified identity.
        (AuthMode::Tailscale, AuthCredential::TailscaleIdentity(identity)) => {
            if identity.is_empty() {
                Err(FrankClawError::AuthFailed)
            } else {
                Ok(AuthRole::Admin)
            }
        }

        // Token mode but no token configured.
        (AuthMode::Token { token: None }, _) => {
            tracing::error!("token auth mode configured but no token set");
            Err(FrankClawError::Internal {
                msg: "auth misconfigured".into(),
            })
        }

        // Mismatched credential type.
        _ => Err(FrankClawError::AuthRequired),
    };

    // Record success/failure for rate limiting.
    if let Some(addr) = remote_addr {
        match &result {
            Ok(_) => rate_limiter.record_success(&addr.ip()),
            Err(FrankClawError::AuthFailed) => rate_limiter.record_failure(&addr.ip()),
            _ => {}
        }
    }

    result
}

/// Credential presented by a connecting client.
pub enum AuthCredential {
    BearerToken(SecretString),
    Password(SecretString),
    ProxyIdentity(String),
    TailscaleIdentity(String),
    None,
}

/// Check that the bind mode is safe for the auth mode.
///
/// HARD REQUIREMENT: network-accessible bind modes MUST have auth enabled.
/// This prevents accidentally exposing an unauthenticated gateway to the network.
pub fn validate_bind_auth(
    bind: &frankclaw_core::config::BindMode,
    auth: &AuthMode,
) -> Result<()> {
    match (bind, auth) {
        // Loopback is safe without auth.
        (frankclaw_core::config::BindMode::Loopback, _) => Ok(()),

        // LAN or specific address: auth is REQUIRED.
        (_, AuthMode::None) => {
            tracing::error!("refusing to bind to network without authentication");
            Err(FrankClawError::ConfigValidation {
                msg: "gateway.auth must be configured when bind mode is 'lan' or a specific address. \
                      Set gateway.auth.mode to 'token' or 'password'."
                    .into(),
            })
        }

        // Network bind with auth is fine.
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::config::BindMode;

    #[test]
    fn loopback_allows_no_auth() {
        assert!(validate_bind_auth(&BindMode::Loopback, &AuthMode::None).is_ok());
    }

    #[test]
    fn lan_requires_auth() {
        assert!(validate_bind_auth(&BindMode::Lan, &AuthMode::None).is_err());
    }

    #[test]
    fn lan_with_token_is_ok() {
        let mode = AuthMode::Token {
            token: Some(SecretString::from("test")),
        };
        assert!(validate_bind_auth(&BindMode::Lan, &mode).is_ok());
    }
}
