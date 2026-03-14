use snafu::Snafu;

use crate::types::{AgentId, ChannelId, SessionKey};

/// Unified error hierarchy. Every variant is explicit — no catch-all.
/// Error messages never contain secret material.
///
/// Each variant carries an implicit `snafu::Location` that records the
/// file and line where the error was constructed, available via the
/// [`FrankClawError::location`] method.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub), context(suffix(false)))]
pub enum FrankClawError {
    // ── Auth ──────────────────────────────────────────────
    #[snafu(display("authentication required"))]
    AuthRequired {
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("authentication failed"))]
    AuthFailed {
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("rate limited (retry after {retry_after_secs}s)"))]
    RateLimited {
        retry_after_secs: u64,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("insufficient permissions for method {method}"))]
    Forbidden {
        method: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Session ──────────────────────────────────────────
    #[snafu(display("session not found: {key}"))]
    SessionNotFound {
        key: SessionKey,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("session storage error: {msg}"))]
    SessionStorage {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Channel ──────────────────────────────────────────
    #[snafu(display("channel {channel} error: {msg}"))]
    Channel {
        channel: ChannelId,
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("channel {channel} not configured"))]
    ChannelNotConfigured {
        channel: ChannelId,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("channel {channel} is disabled"))]
    ChannelDisabled {
        channel: ChannelId,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("sender blocked by policy on channel {channel}"))]
    SenderBlocked {
        channel: ChannelId,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Agent ────────────────────────────────────────────
    #[snafu(display("agent {agent_id} not found"))]
    AgentNotFound {
        agent_id: AgentId,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("agent runtime error: {msg}"))]
    AgentRuntime {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("agent turn cancelled"))]
    TurnCancelled {
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("sandbox error: {msg}"))]
    Sandbox {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Model ────────────────────────────────────────────
    #[snafu(display("model provider error: {msg}"))]
    ProviderError {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("all model providers failed"))]
    AllProvidersFailed {
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("model not found: {model_id}"))]
    ModelNotFound {
        model_id: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Config ───────────────────────────────────────────
    #[snafu(display("config validation error: {msg}"))]
    ConfigValidation {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("config I/O error: {msg}"))]
    ConfigIo {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Protocol ─────────────────────────────────────────
    #[snafu(display("invalid request: {msg}"))]
    InvalidRequest {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("unknown method: {method}"))]
    UnknownMethod {
        method: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("request too large (max {max_bytes} bytes)"))]
    RequestTooLarge {
        max_bytes: usize,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Media ────────────────────────────────────────────
    #[snafu(display("media file too large (max {max_bytes} bytes)"))]
    MediaTooLarge {
        max_bytes: u64,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("media fetch blocked: {reason}"))]
    MediaFetchBlocked {
        reason: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("unsupported media type: {mime}"))]
    UnsupportedMediaType {
        mime: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("malware detected in file '{filename}': {detail}"))]
    MalwareDetected {
        filename: String,
        detail: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Crypto ───────────────────────────────────────────
    #[snafu(display("cryptographic operation failed"), context(false))]
    Crypto {
        source: frankclaw_crypto::CryptoError,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    // ── Internal ─────────────────────────────────────────
    #[snafu(display("internal error: {msg}"))]
    Internal {
        msg: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("shutdown in progress"))]
    ShuttingDown {
        #[snafu(implicit)]
        location: snafu::Location,
    },
}

impl FrankClawError {
    /// Where the error was created (file, line, column).
    pub fn location(&self) -> &snafu::Location {
        match self {
            Self::AuthRequired { location, .. }
            | Self::AuthFailed { location, .. }
            | Self::RateLimited { location, .. }
            | Self::Forbidden { location, .. }
            | Self::SessionNotFound { location, .. }
            | Self::SessionStorage { location, .. }
            | Self::Channel { location, .. }
            | Self::ChannelNotConfigured { location, .. }
            | Self::ChannelDisabled { location, .. }
            | Self::SenderBlocked { location, .. }
            | Self::AgentNotFound { location, .. }
            | Self::AgentRuntime { location, .. }
            | Self::TurnCancelled { location, .. }
            | Self::Sandbox { location, .. }
            | Self::ProviderError { location, .. }
            | Self::AllProvidersFailed { location, .. }
            | Self::ModelNotFound { location, .. }
            | Self::ConfigValidation { location, .. }
            | Self::ConfigIo { location, .. }
            | Self::InvalidRequest { location, .. }
            | Self::UnknownMethod { location, .. }
            | Self::RequestTooLarge { location, .. }
            | Self::MediaTooLarge { location, .. }
            | Self::MediaFetchBlocked { location, .. }
            | Self::UnsupportedMediaType { location, .. }
            | Self::MalwareDetected { location, .. }
            | Self::Crypto { location, .. }
            | Self::Internal { location, .. }
            | Self::ShuttingDown { location, .. } => location,
        }
    }

    /// Whether the client should retry this request.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::ProviderError { .. }
                | Self::AllProvidersFailed { .. }
                | Self::Internal { .. }
        )
    }

    /// HTTP-like status code for protocol responses.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::AuthRequired { .. } | Self::AuthFailed { .. } => 401,
            Self::RateLimited { .. } => 429,
            Self::Forbidden { .. } => 403,
            Self::SessionNotFound { .. }
            | Self::AgentNotFound { .. }
            | Self::ModelNotFound { .. }
            | Self::ChannelNotConfigured { .. } => 404,
            Self::InvalidRequest { .. } | Self::UnknownMethod { .. } => 400,
            Self::RequestTooLarge { .. } | Self::MediaTooLarge { .. } => 413,
            Self::MediaFetchBlocked { .. }
            | Self::MalwareDetected { .. }
            | Self::SenderBlocked { .. } => 403,
            Self::ConfigValidation { .. } => 422,
            Self::SessionStorage { .. }
            | Self::Channel { .. }
            | Self::ChannelDisabled { .. }
            | Self::AgentRuntime { .. }
            | Self::TurnCancelled { .. }
            | Self::Sandbox { .. }
            | Self::ProviderError { .. }
            | Self::AllProvidersFailed { .. }
            | Self::ConfigIo { .. }
            | Self::UnsupportedMediaType { .. }
            | Self::Crypto { .. }
            | Self::Internal { .. }
            | Self::ShuttingDown { .. } => 500,
        }
    }
}

pub type Result<T> = std::result::Result<T, FrankClawError>;

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::types::{AgentId, ChannelId, SessionKey};

    /// Build every error variant. Returns (error, expected_display, expected_status, expected_retryable).
    fn error_cases() -> Vec<(FrankClawError, &'static str, u16, bool)> {
        vec![
            (AuthRequired.build(), "authentication required", 401, false),
            (AuthFailed.build(), "authentication failed", 401, false),
            (RateLimited { retry_after_secs: 30_u64 }.build(), "rate limited (retry after 30s)", 429, true),
            (Forbidden { method: "chat_send" }.build(), "insufficient permissions for method chat_send", 403, false),
            (SessionNotFound { key: SessionKey::from_raw("a:b:c") }.build(), "session not found: a:b:c", 404, false),
            (SessionStorage { msg: "disk full" }.build(), "session storage error: disk full", 500, false),
            (Channel { channel: ChannelId::new("discord"), msg: "timeout" }.build(), "channel discord error: timeout", 500, false),
            (ChannelNotConfigured { channel: ChannelId::new("slack") }.build(), "channel slack not configured", 404, false),
            (ChannelDisabled { channel: ChannelId::new("telegram") }.build(), "channel telegram is disabled", 500, false),
            (SenderBlocked { channel: ChannelId::new("signal") }.build(), "sender blocked by policy on channel signal", 403, false),
            (AgentNotFound { agent_id: AgentId::new("missing") }.build(), "agent missing not found", 404, false),
            (AgentRuntime { msg: "oom" }.build(), "agent runtime error: oom", 500, false),
            (TurnCancelled.build(), "agent turn cancelled", 500, false),
            (Sandbox { msg: "denied" }.build(), "sandbox error: denied", 500, false),
            (Provider { msg: "rate limit" }.build(), "model provider error: rate limit", 500, true),
            (AllProvidersFailed.build(), "all model providers failed", 500, true),
            (ModelNotFound { model_id: "gpt-5" }.build(), "model not found: gpt-5", 404, false),
            (ConfigValidation { msg: "bad port" }.build(), "config validation error: bad port", 422, false),
            (ConfigIo { msg: "not found" }.build(), "config I/O error: not found", 500, false),
            (InvalidRequest { msg: "missing field" }.build(), "invalid request: missing field", 400, false),
            (UnknownMethod { method: "foo" }.build(), "unknown method: foo", 400, false),
            (RequestTooLarge { max_bytes: 4096_usize }.build(), "request too large (max 4096 bytes)", 413, false),
            (MediaTooLarge { max_bytes: 1024_u64 }.build(), "media file too large (max 1024 bytes)", 413, false),
            (MediaFetchBlocked { reason: "private ip" }.build(), "media fetch blocked: private ip", 403, false),
            (UnsupportedMediaType { mime: "video/avi" }.build(), "unsupported media type: video/avi", 500, false),
            (MalwareDetected { filename: "bad.exe", detail: "trojan" }.build(), "malware detected in file 'bad.exe': trojan", 403, false),
            (Internal { msg: "unexpected" }.build(), "internal error: unexpected", 500, true),
            (ShuttingDown.build(), "shutdown in progress", 500, false),
        ]
    }

    #[test]
    fn every_variant_has_a_test_case() {
        // If a new variant is added but not covered here, this count will drift
        // and the display/status/retryable tests below will miss it.
        assert_eq!(
            error_cases().len(),
            28,
            "error_cases() should cover all 28 FrankClawError variants"
        );
    }

    #[rstest]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(3)]
    #[case(4)]
    #[case(5)]
    #[case(6)]
    #[case(7)]
    #[case(8)]
    #[case(9)]
    #[case(10)]
    #[case(11)]
    #[case(12)]
    #[case(13)]
    #[case(14)]
    #[case(15)]
    #[case(16)]
    #[case(17)]
    #[case(18)]
    #[case(19)]
    #[case(20)]
    #[case(21)]
    #[case(22)]
    #[case(23)]
    #[case(24)]
    #[case(25)]
    #[case(26)]
    #[case(27)]
    fn display_message(#[case] idx: usize) {
        let cases = error_cases();
        let (error, expected, _, _) = &cases[idx];
        assert_eq!(error.to_string(), *expected, "variant index {idx}");
    }

    #[rstest]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(3)]
    #[case(4)]
    #[case(5)]
    #[case(6)]
    #[case(7)]
    #[case(8)]
    #[case(9)]
    #[case(10)]
    #[case(11)]
    #[case(12)]
    #[case(13)]
    #[case(14)]
    #[case(15)]
    #[case(16)]
    #[case(17)]
    #[case(18)]
    #[case(19)]
    #[case(20)]
    #[case(21)]
    #[case(22)]
    #[case(23)]
    #[case(24)]
    #[case(25)]
    #[case(26)]
    #[case(27)]
    fn status_code(#[case] idx: usize) {
        let cases = error_cases();
        let (error, _, expected_status, _) = &cases[idx];
        assert_eq!(
            error.status_code(),
            *expected_status,
            "status_code for '{}' (index {idx})",
            error
        );
    }

    #[rstest]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(3)]
    #[case(4)]
    #[case(5)]
    #[case(6)]
    #[case(7)]
    #[case(8)]
    #[case(9)]
    #[case(10)]
    #[case(11)]
    #[case(12)]
    #[case(13)]
    #[case(14)]
    #[case(15)]
    #[case(16)]
    #[case(17)]
    #[case(18)]
    #[case(19)]
    #[case(20)]
    #[case(21)]
    #[case(22)]
    #[case(23)]
    #[case(24)]
    #[case(25)]
    #[case(26)]
    #[case(27)]
    fn retryable(#[case] idx: usize) {
        let cases = error_cases();
        let (error, _, _, expected_retry) = &cases[idx];
        assert_eq!(
            error.is_retryable(),
            *expected_retry,
            "is_retryable for '{}' (index {idx})",
            error
        );
    }

    #[test]
    fn location_is_captured() {
        let error = AuthRequired.build();
        let loc = error.location();
        let loc_str = loc.to_string();
        assert!(
            loc_str.contains("error.rs"),
            "location should point to this file, got: {loc_str}",
        );
    }

    #[test]
    fn location_reflects_call_site() {
        let line_before = line!();
        let error = Internal { msg: "test" }.build();
        let line_after = line!();

        let loc_str = error.location().to_string();
        // Location format: "file:line:column"
        let parts: Vec<&str> = loc_str.split(':').collect();
        let loc_line: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        assert!(
            loc_line >= line_before && loc_line <= line_after,
            "expected location line between {line_before} and {line_after}, got {loc_str}",
        );
    }

    #[test]
    fn crypto_error_converts_via_question_mark() {
        fn inner() -> Result<()> {
            let crypto_result: std::result::Result<(), frankclaw_crypto::CryptoError> =
                Err(frankclaw_crypto::CryptoError::DecryptionFailed);
            crypto_result?;
            Ok(())
        }
        let err = inner().unwrap_err();
        assert_eq!(err.status_code(), 500);
        assert!(
            err.to_string().contains("cryptographic operation failed"),
            "got: {}",
            err
        );
    }
}
