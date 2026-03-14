use std::net::SocketAddr;

use frankclaw_core::auth::{AuthMode, AuthRole};
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_crypto::{verify_password, verify_token_eq};
use secrecy::{ExposeSecret, SecretString};

use crate::rate_limit::AuthRateLimiter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExposureSurface {
    Loopback,
    Lan,
    PrivateAddress(String),
    PublicAddress(String),
}

#[derive(Debug, Clone)]
pub struct ExposureReport {
    pub surface: ExposureSurface,
    pub auth_mode: &'static str,
    pub remote_ready: bool,
    pub public_ready: bool,
    pub summary: String,
    pub warnings: Vec<String>,
}

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
    if let Some(addr) = remote_addr
        && let Some(remaining) = rate_limiter.is_locked(&addr.ip()) {
            return Err(FrankClawError::RateLimited {
                retry_after_secs: remaining.as_secs(),
            });
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

pub fn assess_exposure(config: &frankclaw_core::config::FrankClawConfig) -> Result<ExposureReport> {
    validate_bind_auth(&config.gateway.bind, &config.gateway.auth)?;

    let surface = classify_surface(&config.gateway.bind)?;
    let auth_mode = auth_mode_name(&config.gateway.auth);
    let mut warnings = Vec::new();
    let mut remote_ready = !matches!(surface, ExposureSurface::Loopback);
    let mut public_ready = !matches!(
        surface,
        ExposureSurface::Loopback | ExposureSurface::Lan | ExposureSurface::PrivateAddress(_)
    );

    match &config.gateway.auth {
        AuthMode::None => {
            remote_ready = false;
            public_ready = false;
        }
        AuthMode::Token { .. } | AuthMode::Password { .. } => {
            if config.gateway.tls.is_none() && !matches!(surface, ExposureSurface::Loopback) {
                warnings.push(
                    "network-exposed direct auth is running without TLS; keep it tailnet-only or terminate TLS upstream"
                        .into(),
                );
                public_ready = false;
            }
        }
        AuthMode::TrustedProxy { .. } => {
            warnings.push(
                "trusted_proxy mode assumes a header-scrubbing reverse proxy; do not expose it directly"
                    .into(),
            );
            public_ready = false;
        }
        AuthMode::Tailscale => {
            warnings.push(
                "tailscale auth expects identity headers from a trusted Tailscale-aware proxy path"
                    .into(),
            );
            public_ready = false;
        }
    }

    if matches!(surface, ExposureSurface::Loopback) {
        warnings.push("gateway is loopback-only; remote access is disabled".into());
    }

    let summary = match (&surface, &config.gateway.auth) {
        (ExposureSurface::Loopback, _) => "local-only gateway".into(),
        (_, AuthMode::Token { .. }) | (_, AuthMode::Password { .. }) => {
            "network-exposed gateway with direct auth".into()
        }
        (_, AuthMode::TrustedProxy { .. }) => "reverse-proxy mediated gateway".into(),
        (_, AuthMode::Tailscale) => "tailnet-mediated gateway".into(),
        (_, AuthMode::None) => "misconfigured network exposure".into(),
    };

    Ok(ExposureReport {
        surface,
        auth_mode,
        remote_ready,
        public_ready,
        summary,
        warnings,
    })
}

fn auth_mode_name(mode: &AuthMode) -> &'static str {
    match mode {
        AuthMode::None => "none",
        AuthMode::Token { .. } => "token",
        AuthMode::Password { .. } => "password",
        AuthMode::TrustedProxy { .. } => "trusted_proxy",
        AuthMode::Tailscale => "tailscale",
    }
}

fn classify_surface(bind: &frankclaw_core::config::BindMode) -> Result<ExposureSurface> {
    match bind {
        frankclaw_core::config::BindMode::Loopback => Ok(ExposureSurface::Loopback),
        frankclaw_core::config::BindMode::Lan => Ok(ExposureSurface::Lan),
        frankclaw_core::config::BindMode::Address(address) => {
            let ip: std::net::IpAddr = address.parse().map_err(|_| FrankClawError::ConfigValidation {
                msg: format!("gateway.bind address '{address}' is not a valid IP address"),
            })?;
            let is_private = match ip {
                std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
                std::net::IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
            };
            if is_private {
                Ok(ExposureSurface::PrivateAddress(address.clone()))
            } else {
                Ok(ExposureSurface::PublicAddress(address.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::config::BindMode;
    use secrecy::SecretString;

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

    #[test]
    fn assess_exposure_marks_loopback_as_local_only() {
        let report = assess_exposure(&frankclaw_core::config::FrankClawConfig::default())
            .expect("assessment should succeed");
        assert_eq!(report.surface, ExposureSurface::Loopback);
        assert!(!report.remote_ready);
        assert!(report.warnings.iter().any(|warning| warning.contains("loopback-only")));
    }

    #[test]
    fn assess_exposure_warns_on_public_direct_auth_without_tls() {
        let mut config = frankclaw_core::config::FrankClawConfig::default();
        config.gateway.bind = BindMode::Address("203.0.113.10".into());
        config.gateway.auth = AuthMode::Token {
            token: Some(SecretString::from("secret")),
        };

        let report = assess_exposure(&config).expect("assessment should succeed");
        assert!(report.remote_ready);
        assert!(!report.public_ready);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("without TLS")));
    }
}
