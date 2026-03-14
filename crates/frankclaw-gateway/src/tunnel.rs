//! Tunnel support for exposing the local gateway to the public internet.
//!
//! Supports Cloudflare Tunnel (`cloudflared`), ngrok, and custom tunnel commands.
//! Each tunnel spawns an external process, extracts the public URL from its
//! output, and manages the process lifecycle.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

#![forbid(unsafe_code)]

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use frankclaw_core::error::{FrankClawError, Result};

/// Tunnel provider configuration.
#[derive(Debug, Clone)]
pub enum TunnelConfig {
    /// Cloudflare Tunnel via `cloudflared`.
    Cloudflare { token: String },
    /// ngrok tunnel.
    Ngrok {
        auth_token: String,
        domain: Option<String>,
    },
    /// Custom tunnel command with `{host}` and `{port}` placeholders.
    Custom {
        command: String,
        url_pattern: Option<String>,
    },
}

/// A running tunnel instance.
pub struct Tunnel {
    name: String,
    public_url: Arc<RwLock<Option<String>>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Tunnel {
    /// Start a tunnel with the given config, returning the public URL.
    pub async fn start(config: &TunnelConfig, host: &str, port: u16) -> Result<Self> {
        match config {
            TunnelConfig::Cloudflare { token } => start_cloudflare(token, host, port).await,
            TunnelConfig::Ngrok { auth_token, domain } => {
                start_ngrok(auth_token, domain.as_deref(), host, port).await
            }
            TunnelConfig::Custom {
                command,
                url_pattern,
            } => start_custom(command, url_pattern.as_deref(), host, port).await,
        }
    }

    /// Get the public URL of this tunnel.
    pub fn public_url(&self) -> Option<String> {
        let g = self.public_url.read().ok()?;
        g.clone()
    }

    /// Provider name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Stop the tunnel and kill the process.
    pub async fn stop(&self) {
        let child = self.child.lock().await.take();
        if let Some(mut child) = child {
            let _ = child.kill().await;
            info!(tunnel = %self.name, "tunnel stopped");
        }
        if let Ok(mut url) = self.public_url.write() {
            *url = None;
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.try_lock()
            && let Some(ref mut child) = *guard {
                let _ = child.start_kill();
            }
    }
}

// ---------------------------------------------------------------------------
// Cloudflare Tunnel
// ---------------------------------------------------------------------------

async fn start_cloudflare(token: &str, host: &str, port: u16) -> Result<Tunnel> {
    let origin = format!("http://{host}:{port}");

    let mut child = Command::new("cloudflared")
        .args([
            "tunnel",
            "--no-autoupdate",
            "run",
            "--token",
            token,
            "--url",
            &origin,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| FrankClawError::Internal {
            msg: format!("failed to spawn cloudflared: {e}"),
        })?;

    let stderr = child.stderr.take().ok_or_else(|| FrankClawError::Internal {
        msg: "failed to capture cloudflared stderr".into(),
    })?;

    // Drain stdout in background.
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        });
    }

    // Scan stderr for the public URL.
    let url = extract_url_from_stream(stderr, "https://", Duration::from_secs(30)).await?;

    info!(tunnel = "cloudflare", url = %url, "tunnel started");

    let public_url = Arc::new(RwLock::new(Some(url)));
    let child_handle = Arc::new(Mutex::new(Some(child)));

    Ok(Tunnel {
        name: "cloudflare".into(),
        public_url,
        child: child_handle,
    })
}

// ---------------------------------------------------------------------------
// ngrok
// ---------------------------------------------------------------------------

async fn start_ngrok(
    auth_token: &str,
    domain: Option<&str>,
    host: &str,
    port: u16,
) -> Result<Tunnel> {
    let target = format!("{host}:{port}");

    let mut cmd = Command::new("ngrok");
    cmd.args(["http", &target])
        .args(["--log", "stdout", "--log-format", "logfmt"])
        .env("NGROK_AUTHTOKEN", auth_token)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    if let Some(domain) = domain {
        cmd.args(["--domain", domain]);
    }

    let mut child = cmd.spawn().map_err(|e| FrankClawError::Internal {
        msg: format!("failed to spawn ngrok: {e}"),
    })?;

    let stdout = child.stdout.take().ok_or_else(|| FrankClawError::Internal {
        msg: "failed to capture ngrok stdout".into(),
    })?;

    // Drain stderr in background.
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(ngrok_stderr = %line);
            }
        });
    }

    // Scan stdout for URL in logfmt output (url=https://...).
    let url = extract_url_from_stream(stdout, "url=https://", Duration::from_secs(15)).await.map_err(|_| FrankClawError::Internal {
                msg: "ngrok did not output a public URL within 15 seconds".into(),
            })?;

    // Clean up "url=" prefix if present.
    let url = url
        .strip_prefix("url=")
        .unwrap_or(&url)
        .to_string();

    info!(tunnel = "ngrok", url = %url, "tunnel started");

    let public_url = Arc::new(RwLock::new(Some(url)));
    let child_handle = Arc::new(Mutex::new(Some(child)));

    Ok(Tunnel {
        name: "ngrok".into(),
        public_url,
        child: child_handle,
    })
}

// ---------------------------------------------------------------------------
// Custom tunnel
// ---------------------------------------------------------------------------

async fn start_custom(
    command: &str,
    url_pattern: Option<&str>,
    host: &str,
    port: u16,
) -> Result<Tunnel> {
    #[expect(clippy::literal_string_with_formatting_args, reason = "template placeholders for user-defined tunnel commands, not format! args")]
    let expanded = command
        .replace("{host}", host)
        .replace("{port}", &port.to_string());

    let parts: Vec<&str> = expanded.split_whitespace().collect();
    if parts.is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: "tunnel command is empty".into(),
        });
    }

    let mut child = Command::new(parts[0])
        .args(&parts[1..])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| FrankClawError::Internal {
            msg: format!("failed to spawn tunnel command '{}': {e}", parts[0]),
        })?;

    // Drain stderr in background.
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(tunnel_stderr = %line);
            }
        });
    }

    let url = if let Some(pattern) = url_pattern {
        // Scan stdout for a URL containing the pattern.
        let stdout = child.stdout.take().ok_or_else(|| FrankClawError::Internal {
            msg: "failed to capture tunnel stdout".into(),
        })?;
        extract_url_from_stream(stdout, pattern, Duration::from_secs(15)).await?
    } else {
        // No pattern — drain stdout and use local URL.
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(_)) = lines.next_line().await {}
            });
        }
        format!("http://{host}:{port}")
    };

    info!(tunnel = "custom", url = %url, "tunnel started");

    let public_url = Arc::new(RwLock::new(Some(url)));
    let child_handle = Arc::new(Mutex::new(Some(child)));

    Ok(Tunnel {
        name: "custom".into(),
        public_url,
        child: child_handle,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Scan an async reader for a line containing a URL-like prefix.
///
/// Returns the extracted URL (from the prefix to the next whitespace).
async fn extract_url_from_stream<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    prefix: &str,
    timeout: Duration,
) -> Result<String> {
    let buf = BufReader::new(reader);
    let mut lines = buf.lines();

    let result = tokio::time::timeout(timeout, async {
        while let Ok(Some(line)) = lines.next_line().await {
            debug!(tunnel_output = %line);
            if let Some(url) = extract_url_from_line(&line, prefix) {
                return Some(url);
            }
        }
        None
    })
    .await;

    match result {
        Ok(Some(url)) => Ok(url),
        Ok(None) => Err(FrankClawError::Internal {
            msg: format!("tunnel output ended without a URL (looking for '{prefix}')"),
        }),
        Err(_) => Err(FrankClawError::Internal {
            msg: format!("tunnel did not output a URL within {}s", timeout.as_secs()),
        }),
    }
}

/// Extract a URL from a line of text, starting from the given prefix.
fn extract_url_from_line(line: &str, prefix: &str) -> Option<String> {
    let idx = line.find(prefix)?;
    // Find the actual URL start (might be "url=https://..." or just "https://...")
    let url_start = if line[idx..].starts_with("url=") {
        idx + 4 // Skip "url="
    } else {
        idx
    };

    // Find URL in the text starting from the URL-like prefix.
    let rest = &line[url_start..];
    let http_idx = rest.find("https://").or_else(|| rest.find("http://"))?;
    let url_part = &rest[http_idx..];
    let end = url_part
        .find(|c: char| c.is_whitespace())
        .unwrap_or(url_part.len());
    Some(url_part[..end].to_string())
}

/// Parse tunnel configuration from environment variables.
///
/// Returns `None` if no tunnel is configured.
pub fn tunnel_config_from_env() -> Option<TunnelConfig> {
    let provider = std::env::var("FRANKCLAW_TUNNEL").ok()?;
    match provider.trim().to_ascii_lowercase().as_str() {
        "cloudflare" | "cloudflared" => {
            let token = std::env::var("FRANKCLAW_TUNNEL_CF_TOKEN").ok()?;
            Some(TunnelConfig::Cloudflare { token })
        }
        "ngrok" => {
            let auth_token = std::env::var("FRANKCLAW_TUNNEL_NGROK_TOKEN").ok()?;
            let domain = std::env::var("FRANKCLAW_TUNNEL_NGROK_DOMAIN").ok();
            Some(TunnelConfig::Ngrok { auth_token, domain })
        }
        "custom" => {
            let command = std::env::var("FRANKCLAW_TUNNEL_COMMAND").ok()?;
            let url_pattern = std::env::var("FRANKCLAW_TUNNEL_URL_PATTERN").ok();
            Some(TunnelConfig::Custom {
                command,
                url_pattern,
            })
        }
        _ => {
            warn!(provider = %provider, "unknown tunnel provider");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_from_line_finds_https() {
        let line = "INFO: tunnel started at https://abc.trycloudflare.com finished";
        let url = extract_url_from_line(line, "https://").unwrap();
        assert_eq!(url, "https://abc.trycloudflare.com");
    }

    #[test]
    fn extract_url_from_line_finds_http() {
        let line = "listening on http://localhost:8080";
        let url = extract_url_from_line(line, "http://").unwrap();
        assert_eq!(url, "http://localhost:8080");
    }

    #[test]
    fn extract_url_from_line_with_logfmt_prefix() {
        let line = "t=2024 lvl=info msg=started url=https://abc.ngrok-free.app addr=localhost:8080";
        let url = extract_url_from_line(line, "url=https://").unwrap();
        assert_eq!(url, "https://abc.ngrok-free.app");
    }

    #[test]
    fn extract_url_from_line_no_match() {
        let line = "no urls here at all";
        assert!(extract_url_from_line(line, "https://").is_none());
    }

    #[test]
    fn extract_url_from_line_url_at_end() {
        let line = "tunnel: https://example.com";
        let url = extract_url_from_line(line, "https://").unwrap();
        assert_eq!(url, "https://example.com");
    }

    #[test]
    fn custom_command_placeholder_expansion() {
        let cmd = "bore local --to bore.pub --port {port}";
        let expanded = cmd
            .replace("{host}", "127.0.0.1")
            .replace("{port}", "8080");
        assert_eq!(expanded, "bore local --to bore.pub --port 8080");
    }

    #[test]
    fn tunnel_public_url_none_before_start() {
        let tunnel = Tunnel {
            name: "test".into(),
            public_url: Arc::new(RwLock::new(None)),
            child: Arc::new(Mutex::new(None)),
        };
        assert!(tunnel.public_url().is_none());
    }

    #[test]
    fn tunnel_public_url_returns_value() {
        let tunnel = Tunnel {
            name: "test".into(),
            public_url: Arc::new(RwLock::new(Some("https://example.com".into()))),
            child: Arc::new(Mutex::new(None)),
        };
        assert_eq!(tunnel.public_url().unwrap(), "https://example.com");
    }

    #[tokio::test]
    async fn tunnel_stop_clears_url() {
        let tunnel = Tunnel {
            name: "test".into(),
            public_url: Arc::new(RwLock::new(Some("https://example.com".into()))),
            child: Arc::new(Mutex::new(None)),
        };
        tunnel.stop().await;
        assert!(tunnel.public_url().is_none());
    }

    #[tokio::test]
    async fn custom_tunnel_empty_command_errors() {
        let result = start_custom("", None, "localhost", 8080).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn extract_url_from_cursor() {
        let data = b"some line\nhttps://tunnel.example.com ready\n";
        let cursor = std::io::Cursor::new(data.to_vec());
        let url =
            extract_url_from_stream(cursor, "https://", Duration::from_secs(5)).await.unwrap();
        assert_eq!(url, "https://tunnel.example.com");
    }

    #[tokio::test]
    async fn extract_url_timeout_on_no_match() {
        let data = b"no urls\njust text\n";
        let cursor = std::io::Cursor::new(data.to_vec());
        let result =
            extract_url_from_stream(cursor, "https://", Duration::from_millis(100)).await;
        assert!(result.is_err());
    }
}
