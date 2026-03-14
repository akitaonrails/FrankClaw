//! VirusTotal file scanning integration.
//!
//! Uses the VirusTotal v3 API to scan files for malware before they are
//! stored or delivered to users. Requires a `VIRUSTOTAL_API_KEY` environment
//! variable. When the key is not set, the scanner is not created and all
//! files pass through without scanning.
//!
//! API docs: https://docs.virustotal.com/reference/files-scan

use std::time::Duration;

use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::{debug, warn};

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::media::{FileScanService, ScanVerdict};

/// Minimum number of engines flagging a file to consider it malicious.
/// A single engine false-positive shouldn't block legitimate files.
const MALICIOUS_THRESHOLD: u32 = 2;

/// Maximum time to wait for VirusTotal analysis to complete.
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(120);

/// Poll interval when waiting for analysis results.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum file size VirusTotal accepts (32 MB for standard endpoint).
const MAX_VT_FILE_SIZE: usize = 32 * 1024 * 1024;

/// VirusTotal scanner using the v3 REST API.
pub struct VirusTotalScanner {
    client: Client,
    api_key: SecretString,
}

impl VirusTotalScanner {
    /// Create a scanner from an explicit API key.
    pub fn new(api_key: SecretString) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| FrankClawError::Internal {
                msg: format!("failed to build HTTP client: {e}"),
            })?;
        Ok(Self { client, api_key })
    }

    /// Try to create a scanner from the `VIRUSTOTAL_API_KEY` env var.
    /// Returns `Ok(None)` if the variable is not set or empty.
    pub fn from_env() -> Result<Option<Self>> {
        let key = match std::env::var("VIRUSTOTAL_API_KEY") {
            Ok(k) => k,
            Err(_) => return Ok(None),
        };
        let trimmed = key.trim().to_string();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self::new(SecretString::from(trimmed))?))
    }

    /// Upload a file and get the analysis ID.
    async fn upload_file(&self, filename: &str, data: &[u8]) -> Result<String> {
        let form = reqwest::multipart::Form::new()
            .part("file", reqwest::multipart::Part::bytes(data.to_vec())
                .file_name(filename.to_string()));

        let response = self.client
            .post("https://www.virustotal.com/api/v3/files")
            .header("x-apikey", self.api_key.expose_secret())
            .multipart(form)
            .send()
            .await
            .map_err(|e| FrankClawError::Internal {
                msg: format!("VirusTotal upload failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(FrankClawError::Internal {
                msg: format!("VirusTotal upload returned {status}: {body}"),
            });
        }

        let upload: VtUploadResponse = response.json().await.map_err(|e| {
            FrankClawError::Internal {
                msg: format!("VirusTotal upload response parse failed: {e}"),
            }
        })?;

        Ok(upload.data.id)
    }

    /// Poll for analysis results until complete or timeout.
    async fn poll_analysis(&self, analysis_id: &str) -> Result<VtAnalysisAttributes> {
        let url = format!("https://www.virustotal.com/api/v3/analyses/{analysis_id}");
        let deadline = tokio::time::Instant::now() + ANALYSIS_TIMEOUT;

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(FrankClawError::Internal {
                    msg: "VirusTotal analysis timed out".into(),
                });
            }

            let response = self.client
                .get(&url)
                .header("x-apikey", self.api_key.expose_secret())
                .send()
                .await
                .map_err(|e| FrankClawError::Internal {
                    msg: format!("VirusTotal poll failed: {e}"),
                })?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(FrankClawError::Internal {
                    msg: format!("VirusTotal analysis returned {status}: {body}"),
                });
            }

            let analysis: VtAnalysisResponse = response.json().await.map_err(|e| {
                FrankClawError::Internal {
                    msg: format!("VirusTotal analysis response parse failed: {e}"),
                }
            })?;

            if analysis.data.attributes.status == "completed" {
                return Ok(analysis.data.attributes);
            }

            debug!(
                analysis_id,
                status = analysis.data.attributes.status,
                "VirusTotal analysis in progress, polling..."
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

#[async_trait::async_trait]
impl FileScanService for VirusTotalScanner {
    async fn scan(&self, filename: &str, data: &[u8]) -> Result<ScanVerdict> {
        if data.len() > MAX_VT_FILE_SIZE {
            warn!(
                filename,
                size = data.len(),
                max = MAX_VT_FILE_SIZE,
                "file exceeds VirusTotal size limit, skipping scan"
            );
            return Ok(ScanVerdict {
                safe: true,
                malicious_count: 0,
                total_engines: 0,
                summary: "file too large for VirusTotal scan, skipped".into(),
                threat_names: Vec::new(),
            });
        }

        debug!(filename, size = data.len(), "uploading file to VirusTotal");
        let analysis_id = self.upload_file(filename, data).await?;
        debug!(filename, analysis_id, "file uploaded, waiting for analysis");

        let attrs = self.poll_analysis(&analysis_id).await?;
        let stats = &attrs.stats;
        let malicious = stats.malicious.unwrap_or(0);
        let suspicious = stats.suspicious.unwrap_or(0);
        let total = stats.malicious.unwrap_or(0)
            + stats.undetected.unwrap_or(0)
            + stats.harmless.unwrap_or(0)
            + stats.suspicious.unwrap_or(0);

        let threat_names: Vec<String> = attrs.results
            .iter()
            .filter_map(|(_, result)| {
                if result.category == "malicious" || result.category == "suspicious" {
                    result.result.clone()
                } else {
                    None
                }
            })
            .collect();

        let flagged = malicious + suspicious;
        let safe = flagged < MALICIOUS_THRESHOLD;

        let summary = if safe {
            format!("{flagged}/{total} engines flagged (below threshold {MALICIOUS_THRESHOLD})")
        } else {
            format!("{flagged}/{total} engines flagged as malicious/suspicious")
        };

        debug!(
            filename,
            malicious,
            suspicious,
            total,
            safe,
            "VirusTotal scan complete"
        );

        Ok(ScanVerdict {
            safe,
            malicious_count: flagged,
            total_engines: total,
            summary,
            threat_names,
        })
    }
}

// ── VirusTotal API response types ──────────────────────────────────

#[derive(Debug, Deserialize)]
struct VtUploadResponse {
    data: VtUploadData,
}

#[derive(Debug, Deserialize)]
struct VtUploadData {
    id: String,
}

#[derive(Debug, Deserialize)]
struct VtAnalysisResponse {
    data: VtAnalysisData,
}

#[derive(Debug, Deserialize)]
struct VtAnalysisData {
    attributes: VtAnalysisAttributes,
}

#[derive(Debug, Deserialize)]
struct VtAnalysisAttributes {
    status: String,
    stats: VtStats,
    #[serde(default)]
    results: std::collections::HashMap<String, VtEngineResult>,
}

#[derive(Debug, Deserialize)]
struct VtStats {
    malicious: Option<u32>,
    suspicious: Option<u32>,
    undetected: Option<u32>,
    harmless: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct VtEngineResult {
    category: String,
    result: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_from_env_returns_none_without_key() {
        // Don't set the env var — from_env should return None.
        // (If the test runner happens to have it set, this test is a no-op.)
        let key_was_set = std::env::var("VIRUSTOTAL_API_KEY").is_ok();
        if !key_was_set {
            assert!(VirusTotalScanner::from_env().expect("should not error").is_none());
        }
    }

    #[test]
    fn malicious_threshold_is_reasonable() {
        // Sanity check: threshold should be > 0 (single engine FP protection)
        // and reasonable (not so high that real malware passes).
        assert!(MALICIOUS_THRESHOLD >= 2);
        assert!(MALICIOUS_THRESHOLD <= 5);
    }

    #[test]
    fn scan_verdict_serializes() {
        let verdict = ScanVerdict {
            safe: false,
            malicious_count: 12,
            total_engines: 72,
            summary: "12/72 engines flagged".into(),
            threat_names: vec!["Trojan.Test".into()],
        };
        let json = serde_json::to_string(&verdict).expect("should serialize");
        assert!(json.contains("\"safe\":false"));
        assert!(json.contains("Trojan.Test"));
    }
}
