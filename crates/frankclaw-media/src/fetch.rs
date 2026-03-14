use std::net::IpAddr;

use reqwest::Client;
use tracing::warn;
use url::Url;

use frankclaw_core::error::{Internal, MediaFetchBlocked, MediaTooLarge, Result};
use frankclaw_core::media::is_safe_ip;

/// Maximum number of redirects to follow before aborting.
const MAX_REDIRECTS: u8 = 5;

/// HTTP fetcher with SSRF protection.
///
/// Resolves DNS before connecting and blocks requests to private IP ranges.
/// Validates EACH redirect URL through the SSRF checker — not just the original.
/// Prevents attackers from using media fetch URLs to probe internal networks.
pub struct SafeFetcher {
    client: Client,
    max_bytes: u64,
}

impl SafeFetcher {
    pub fn new(max_bytes: u64) -> Result<Self> {
        // Disable automatic redirects — we follow them manually so we can
        // validate each intermediate URL through the SSRF checker.
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Internal {
                msg: format!("failed to build HTTP client: {e}"),
            }.build())?;

        Ok(Self { client, max_bytes })
    }

    /// Fetch a URL with SSRF protection.
    ///
    /// 1. Validate scheme and resolve DNS for each URL in the redirect chain.
    /// 2. Verify ALL resolved IPs are safe (public, non-reserved) at each hop.
    /// 3. Follow redirects manually (up to MAX_REDIRECTS).
    /// 4. Fetch body with dual size enforcement (Content-Length + actual bytes).
    pub async fn fetch(&self, url: &Url) -> Result<FetchedContent> {
        let mut current_url = url.clone();

        for redirect_count in 0..=MAX_REDIRECTS {
            validate_url_ssrf(&current_url).await?;

            let response = self
                .client
                .get(current_url.as_str())
                .send()
                .await
                .map_err(|e| MediaFetchBlocked {
                    reason: format!("fetch failed: {e}"),
                }.build())?;

            // Handle redirects: validate the target URL before following.
            if response.status().is_redirection() {
                if redirect_count >= MAX_REDIRECTS {
                    return MediaFetchBlocked {
                        reason: format!("too many redirects ({MAX_REDIRECTS})"),
                    }.fail();
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| MediaFetchBlocked {
                        reason: "redirect without Location header",
                    }.build())?;
                // Resolve relative URLs against the current URL.
                current_url = current_url.join(location).map_err(|e| {
                    MediaFetchBlocked {
                        reason: format!("invalid redirect URL: {e}"),
                    }.build()
                })?;
                continue;
            }

            if !response.status().is_success() {
                return MediaFetchBlocked {
                    reason: format!("HTTP {}", response.status()),
                }.fail();
            }

            // Check Content-Length header before downloading body.
            if let Some(content_length) = response.content_length()
                && content_length > self.max_bytes {
                    return MediaTooLarge {
                        max_bytes: self.max_bytes,
                    }.fail();
                }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();

            let bytes = response.bytes().await.map_err(|e| {
                MediaFetchBlocked {
                    reason: format!("body read failed: {e}"),
                }.build()
            })?;

            // Enforce actual size limit (Content-Length can lie).
            if bytes.len() as u64 > self.max_bytes {
                return MediaTooLarge {
                    max_bytes: self.max_bytes,
                }.fail();
            }

            return Ok(FetchedContent {
                bytes: bytes.to_vec(),
                content_type,
            });
        }

        MediaFetchBlocked {
            reason: format!("too many redirects ({MAX_REDIRECTS})"),
        }.fail()
    }
}

/// Validate a URL for SSRF protection: check scheme and resolve DNS to ensure
/// all resolved IPs are in public ranges.
async fn validate_url_ssrf(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return MediaFetchBlocked {
                reason: format!("unsupported scheme: {scheme}"),
            }.fail();
        }
    }

    let host = url
        .host_str()
        .ok_or_else(|| MediaFetchBlocked {
            reason: "no host in URL",
        }.build())?;

    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<IpAddr> = tokio::net::lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|e| MediaFetchBlocked {
            reason: format!("DNS resolution failed: {e}"),
        }.build())?
        .map(|addr| addr.ip())
        .collect();

    if addrs.is_empty() {
        return MediaFetchBlocked {
            reason: "DNS resolved to no addresses",
        }.fail();
    }

    // Check ALL resolved IPs, not just the first.
    // This prevents DNS rebinding attacks where one A record is public
    // and another points to an internal address.
    for addr in &addrs {
        if !is_safe_ip(addr) {
            warn!(%url, %addr, "SSRF blocked: URL resolved to private IP");
            return MediaFetchBlocked {
                reason: format!("URL resolves to blocked IP range: {addr}"),
            }.fail();
        }
    }

    Ok(())
}

/// Successfully fetched content.
pub struct FetchedContent {
    pub bytes: Vec<u8>,
    pub content_type: String,
}
