use std::net::IpAddr;

use reqwest::Client;
use tracing::warn;
use url::Url;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::media::is_safe_ip;

/// HTTP fetcher with SSRF protection.
///
/// Resolves DNS before connecting and blocks requests to private IP ranges.
/// Prevents attackers from using media fetch URLs to probe internal networks.
pub struct SafeFetcher {
    client: Client,
    max_bytes: u64,
}

impl SafeFetcher {
    pub fn new(max_bytes: u64) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .expect("failed to build HTTP client");

        Self { client, max_bytes }
    }

    /// Fetch a URL with SSRF protection.
    ///
    /// 1. Parse URL and extract hostname.
    /// 2. Resolve hostname to IP addresses.
    /// 3. Verify ALL resolved IPs are safe (public, non-reserved).
    /// 4. Fetch with size limit.
    pub async fn fetch(&self, url: &Url) -> Result<FetchedContent> {
        // Step 1: Validate scheme.
        match url.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(FrankClawError::MediaFetchBlocked {
                    reason: format!("unsupported scheme: {scheme}"),
                });
            }
        }

        // Step 2: Resolve DNS and check all IPs.
        let host = url.host_str().ok_or_else(|| FrankClawError::MediaFetchBlocked {
            reason: "no host in URL".into(),
        })?;

        let port = url.port_or_known_default().unwrap_or(443);
        let addrs: Vec<IpAddr> = tokio::net::lookup_host(format!("{host}:{port}"))
            .await
            .map_err(|e| FrankClawError::MediaFetchBlocked {
                reason: format!("DNS resolution failed: {e}"),
            })?
            .map(|addr| addr.ip())
            .collect();

        if addrs.is_empty() {
            return Err(FrankClawError::MediaFetchBlocked {
                reason: "DNS resolved to no addresses".into(),
            });
        }

        // Check ALL resolved IPs, not just the first.
        // This prevents DNS rebinding attacks where one A record is public
        // and another points to an internal address.
        for addr in &addrs {
            if !is_safe_ip(addr) {
                warn!(%url, %addr, "SSRF blocked: URL resolved to private IP");
                return Err(FrankClawError::MediaFetchBlocked {
                    reason: format!("URL resolves to blocked IP range: {addr}"),
                });
            }
        }

        // Step 3: Fetch with size limit.
        let response = self
            .client
            .get(url.as_str())
            .send()
            .await
            .map_err(|e| FrankClawError::MediaFetchBlocked {
                reason: format!("fetch failed: {e}"),
            })?;

        // Check Content-Length header before downloading body.
        if let Some(content_length) = response.content_length() {
            if content_length > self.max_bytes {
                return Err(FrankClawError::MediaTooLarge {
                    max_bytes: self.max_bytes,
                });
            }
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let bytes = response.bytes().await.map_err(|e| {
            FrankClawError::MediaFetchBlocked {
                reason: format!("body read failed: {e}"),
            }
        })?;

        if bytes.len() as u64 > self.max_bytes {
            return Err(FrankClawError::MediaTooLarge {
                max_bytes: self.max_bytes,
            });
        }

        Ok(FetchedContent {
            bytes: bytes.to_vec(),
            content_type,
        })
    }
}

/// Successfully fetched content.
pub struct FetchedContent {
    pub bytes: Vec<u8>,
    pub content_type: String,
}
