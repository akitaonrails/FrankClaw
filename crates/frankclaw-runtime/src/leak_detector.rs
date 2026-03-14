//! Credential leak detection for tool outputs and LLM responses.
//!
//! Scans text for patterns matching API keys, tokens, and other credentials
//! that should never appear in tool output or assistant messages.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

/// Severity of a detected credential leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LeakSeverity {
    Medium,
    High,
    Critical,
}

/// What to do when a leak is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakAction {
    /// Block the content entirely (for critical secrets).
    Block,
    /// Mask the matched text with [REDACTED].
    Redact,
    /// Log a warning but allow pass-through.
    Warn,
}

/// A pattern to detect in text.
struct LeakPattern {
    name: &'static str,
    prefix: &'static str,
    check: fn(&str) -> bool,
    severity: LeakSeverity,
    action: LeakAction,
    /// If true, pass the entire content to `check` instead of individual words.
    full_content_check: bool,
}

/// A single match found by the scanner.
#[derive(Debug, Clone)]
pub struct LeakMatch {
    pub pattern_name: String,
    pub severity: LeakSeverity,
    pub action: LeakAction,
    pub masked_preview: String,
}

/// Result of scanning text for credential leaks.
#[derive(Debug)]
pub struct LeakScanResult {
    pub matches: Vec<LeakMatch>,
    pub should_block: bool,
    pub redacted_content: Option<String>,
}

/// Scan text for credential leaks.
///
/// Returns a `LeakScanResult` with all matches found. If any match has
/// `LeakAction::Block`, `should_block` is `true`. If any match has
/// `LeakAction::Redact`, `redacted_content` contains the text with
/// matched regions replaced by `[REDACTED]`.
pub fn scan_for_leaks(content: &str) -> LeakScanResult {
    let patterns = default_patterns();
    let mut matches = Vec::new();
    let mut should_block = false;
    let mut redacted = content.to_string();
    let mut any_redacted = false;

    for pattern in &patterns {
        // Quick prefix check to skip expensive scanning.
        if !pattern.prefix.is_empty() && !content.contains(pattern.prefix) {
            continue;
        }

        if pattern.full_content_check {
            // Some patterns (PEM keys) need full-content matching, not word-level.
            if (pattern.check)(content) {
                let leak = LeakMatch {
                    pattern_name: pattern.name.to_string(),
                    severity: pattern.severity,
                    action: pattern.action,
                    masked_preview: format!("[{}]", pattern.name),
                };
                match pattern.action {
                    LeakAction::Block => should_block = true,
                    LeakAction::Redact => {
                        // Can't easily redact multi-word patterns; mark as blocked.
                        should_block = true;
                    }
                    LeakAction::Warn => {}
                }
                matches.push(leak);
            }
        } else {
            // Split content into words and check each token.
            for word in content.split_whitespace() {
                if (pattern.check)(word) {
                    let leak = LeakMatch {
                        pattern_name: pattern.name.to_string(),
                        severity: pattern.severity,
                        action: pattern.action,
                        masked_preview: mask_secret(word),
                    };

                    match pattern.action {
                        LeakAction::Block => should_block = true,
                        LeakAction::Redact => {
                            redacted = redacted.replace(word, "[REDACTED]");
                            any_redacted = true;
                        }
                        LeakAction::Warn => {}
                    }

                    matches.push(leak);
                }
            }
        }
    }

    LeakScanResult {
        matches,
        should_block,
        redacted_content: any_redacted.then_some(redacted),
    }
}

/// Mask a secret for safe logging: show first 4 + last 4 chars.
fn mask_secret(secret: &str) -> String {
    let len = secret.len();
    if len <= 8 {
        return "*".repeat(len);
    }
    let prefix: String = secret.chars().take(4).collect();
    let suffix: String = secret.chars().skip(len - 4).collect();
    let middle_len = (len - 8).min(8);
    format!("{}{}{}", prefix, "*".repeat(middle_len), suffix)
}

// ── Pattern definitions ─────────────────────────────────────────────────

#[expect(clippy::too_many_lines, reason = "pattern catalog; each entry is a simple struct literal")]
fn default_patterns() -> Vec<LeakPattern> {
    vec![
        // ── Critical: Block ──────────────────────────────────────────
        LeakPattern {
            name: "openai_api_key",
            prefix: "sk-",
            check: |w| {
                (w.starts_with("sk-proj-") || w.starts_with("sk-"))
                    && w.len() >= 20
                    && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    // Exclude Anthropic keys matched below
                    && !w.starts_with("sk-ant-")
            },
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "anthropic_api_key",
            prefix: "sk-ant-api",
            check: |w| {
                w.starts_with("sk-ant-api")
                    && w.len() >= 90
                    && w.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            },
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "aws_access_key",
            prefix: "AKIA",
            check: |w| {
                w.starts_with("AKIA")
                    && w.len() == 20
                    && w[4..].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            },
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "github_token",
            prefix: "gh",
            check: |w| {
                (w.starts_with("ghp_")
                    || w.starts_with("gho_")
                    || w.starts_with("ghu_")
                    || w.starts_with("ghs_")
                    || w.starts_with("ghr_"))
                    && w.len() >= 40
                    && w[4..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            },
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "github_fine_grained_pat",
            prefix: "github_pat_",
            check: |w| {
                w.starts_with("github_pat_")
                    && w.len() >= 80
                    && w[11..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
            },
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "stripe_api_key",
            prefix: "sk_",
            check: |w| {
                (w.starts_with("sk_live_") || w.starts_with("sk_test_"))
                    && w.len() >= 24
                    && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            },
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "pem_private_key",
            prefix: "-----BEGIN",
            check: |w| w.contains("PRIVATE KEY-----"),
            severity: LeakSeverity::Critical,
            action: LeakAction::Block,
            full_content_check: true,
        },
        // ── High: Block ──────────────────────────────────────────────
        LeakPattern {
            name: "google_api_key",
            prefix: "AIza",
            check: |w| {
                w.starts_with("AIza")
                    && w.len() == 39
                    && w[4..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            },
            severity: LeakSeverity::High,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "slack_token",
            prefix: "xox",
            check: |w| {
                (w.starts_with("xoxb-")
                    || w.starts_with("xoxa-")
                    || w.starts_with("xoxp-")
                    || w.starts_with("xoxr-")
                    || w.starts_with("xoxs-"))
                    && w.len() >= 15
            },
            severity: LeakSeverity::High,
            action: LeakAction::Block,
            full_content_check: false,
        },
        LeakPattern {
            name: "sendgrid_api_key",
            prefix: "SG.",
            check: |w| {
                w.starts_with("SG.")
                    && w.len() >= 50
                    && w[3..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
            },
            severity: LeakSeverity::High,
            action: LeakAction::Block,
            full_content_check: false,
        },
        // ── High: Redact ─────────────────────────────────────────────
        LeakPattern {
            name: "bearer_token",
            prefix: "Bearer",
            check: |w| {
                // This checks individual words, so we look for long tokens
                // that follow "Bearer" — but since we split on whitespace,
                // this catches the token part after "Bearer ".
                w.starts_with("Bearer")
                    && w.len() > 10
                    // Exclude "Bearer" by itself
                    && w != "Bearer"
            },
            severity: LeakSeverity::High,
            action: LeakAction::Redact,
            full_content_check: false,
        },
        // ── Medium: Warn ─────────────────────────────────────────────
        LeakPattern {
            name: "high_entropy_hex_64",
            prefix: "",
            check: |w| {
                w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit())
            },
            severity: LeakSeverity::Medium,
            action: LeakAction::Warn,
            full_content_check: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_openai_key() {
        let content = "Here is my key: sk-proj-abc123456789012345678901234567890123";
        let result = scan_for_leaks(content);
        assert!(result.should_block);
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].pattern_name, "openai_api_key");
        assert_eq!(result.matches[0].severity, LeakSeverity::Critical);
    }

    #[test]
    fn detects_anthropic_key() {
        let key = format!("sk-ant-api{}", "a".repeat(90));
        let content = format!("Key: {key}");
        let result = scan_for_leaks(&content);
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "anthropic_api_key");
    }

    #[test]
    fn detects_aws_access_key() {
        let result = scan_for_leaks("AKIAIOSFODNN7EXAMPLE");
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "aws_access_key");
    }

    #[test]
    fn detects_github_token() {
        let token = format!("ghp_{}", "a".repeat(36));
        let result = scan_for_leaks(&token);
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "github_token");
    }

    #[test]
    fn detects_github_fine_grained_pat() {
        let token = format!("github_pat_{}", "a".repeat(81));
        let result = scan_for_leaks(&token);
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "github_fine_grained_pat");
    }

    #[test]
    fn detects_stripe_key() {
        let key = format!("sk_live_{}", "a".repeat(24));
        let result = scan_for_leaks(&key);
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "stripe_api_key");
    }

    #[test]
    fn detects_google_api_key() {
        let key = format!("AIza{}", "a".repeat(35));
        let result = scan_for_leaks(&key);
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "google_api_key");
    }

    #[test]
    fn detects_slack_token() {
        let result = scan_for_leaks("xoxb-1234567890-abcdefg");
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "slack_token");
    }

    #[test]
    fn detects_pem_private_key() {
        let content = "-----BEGIN PRIVATE KEY----- MIIEvgIBADANBg...";
        let result = scan_for_leaks(content);
        assert!(result.should_block);
    }

    #[test]
    fn warns_on_high_entropy_hex() {
        let hex64 = "a".repeat(64);
        let result = scan_for_leaks(&hex64);
        assert!(!result.should_block);
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].action, LeakAction::Warn);
    }

    #[test]
    fn no_false_positive_on_normal_text() {
        let result = scan_for_leaks("Hello, this is a normal message about the weather.");
        assert!(!result.should_block);
        assert!(result.matches.is_empty());
    }

    #[test]
    fn no_false_positive_on_short_sk_prefix() {
        // "sk-" alone or short strings should not match.
        let result = scan_for_leaks("sk-short");
        assert!(!result.should_block);
    }

    #[test]
    fn mask_secret_short() {
        assert_eq!(mask_secret("abcde"), "*****");
    }

    #[test]
    fn mask_secret_long() {
        let masked = mask_secret("sk-proj-abcdefghijklmnop");
        assert!(masked.starts_with("sk-p"));
        assert!(masked.ends_with("mnop"));
        assert!(masked.contains("*"));
    }

    #[test]
    fn redacts_bearer_token() {
        let content = "Use BearerSomeVeryLongTokenValue123456 for auth";
        let result = scan_for_leaks(content);
        assert!(!result.should_block);
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].action, LeakAction::Redact);
        assert!(result.redacted_content.is_some());
        let redacted = result.redacted_content.unwrap();
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("SomeVeryLongTokenValue123456"));
    }

    #[test]
    fn multiple_leaks_detected() {
        let aws = "AKIAIOSFODNN7EXAMPLE";
        let hex = "a".repeat(64);
        let content = format!("{aws} and {hex}");
        let result = scan_for_leaks(&content);
        assert!(result.should_block); // AWS key triggers block
        assert_eq!(result.matches.len(), 2);
    }

    #[test]
    fn sendgrid_key_detected() {
        let key = format!("SG.{}.{}", "a".repeat(22), "b".repeat(43));
        let result = scan_for_leaks(&key);
        assert!(result.should_block);
        assert_eq!(result.matches[0].pattern_name, "sendgrid_api_key");
    }
}
