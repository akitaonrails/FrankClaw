use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

use crate::error::Result;
use crate::types::MediaId;

/// Classification of media content by broad kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Audio,
    Video,
    Document,
    Unknown,
}

/// Classify a MIME type into a media kind.
pub fn classify_mime(mime: &str) -> MediaKind {
    let normalized = normalize_mime(mime);
    if normalized.starts_with("image/") {
        MediaKind::Image
    } else if normalized.starts_with("audio/") {
        MediaKind::Audio
    } else if normalized.starts_with("video/") {
        MediaKind::Video
    } else if matches!(
        normalized,
        "application/pdf"
            | "application/json"
            | "application/zip"
            | "text/plain"
            | "text/csv"
            | "text/markdown"
            | "text/html"
    ) {
        MediaKind::Document
    } else {
        MediaKind::Unknown
    }
}

/// Classify a file by its extension when MIME is unavailable.
pub fn classify_extension(filename: &str) -> MediaKind {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp" | "tiff" => MediaKind::Image,
        "mp3" | "m4a" | "ogg" | "wav" | "flac" | "weba" | "oga" | "aac" | "opus" => {
            MediaKind::Audio
        }
        "mp4" | "webm" | "mov" | "avi" | "mkv" => MediaKind::Video,
        "pdf" | "json" | "txt" | "csv" | "md" | "html" | "xml" | "zip" => MediaKind::Document,
        _ => MediaKind::Unknown,
    }
}

/// A stored media file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFile {
    pub id: MediaId,
    pub original_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub path: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Map a MIME type to a safe file extension.
pub fn safe_extension_for_mime(mime: &str) -> &'static str {
    match normalize_mime(mime) {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "audio/webm" => "weba",
        "audio/flac" => "flac",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "application/pdf" => "pdf",
        "application/json" => "json",
        "application/zip" => "zip",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "text/markdown" => "md",
        _ => "bin",
    }
}

/// Map a safe extension back to a MIME type.
pub fn mime_for_safe_extension(ext: &str) -> &'static str {
    match ext.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "weba" => "audio/webm",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "zip" => "application/zip",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv",
        "md" => "text/markdown",
        _ => "application/octet-stream",
    }
}

/// Infer a MIME type from a filename or URL path segment.
pub fn infer_mime_from_name(name: &str) -> Option<&'static str> {
    let name = name.trim();
    let ext = name.rsplit('.').next()?.trim();
    let mime = mime_for_safe_extension(ext);
    if mime == "application/octet-stream" {
        None
    } else {
        Some(mime)
    }
}

/// Result of scanning a file for malware/threats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanVerdict {
    /// Whether the file is considered safe.
    pub safe: bool,
    /// Number of engines that flagged the file as malicious.
    pub malicious_count: u32,
    /// Total number of engines that scanned the file.
    pub total_engines: u32,
    /// Human-readable summary of the scan result.
    pub summary: String,
    /// Specific threat names detected (e.g., "Trojan.GenericKD.12345").
    pub threat_names: Vec<String>,
}

/// Trait for file scanning services (VirusTotal, ClamAV, etc.).
///
/// Implementations must be Send + Sync for use in async contexts.
#[async_trait::async_trait]
pub trait FileScanService: Send + Sync {
    /// Scan file bytes for malware. Returns the scan verdict.
    ///
    /// Implementations should handle their own timeouts and retries.
    /// If the service is unavailable or times out, return an error
    /// rather than a false `safe: true` verdict.
    async fn scan(&self, filename: &str, data: &[u8]) -> Result<ScanVerdict>;
}

fn normalize_mime(mime: &str) -> &str {
    mime.split(';').next().unwrap_or(mime).trim()
}

/// SSRF protection: check if an IP address is safe to connect to.
///
/// Blocks all private, reserved, loopback, link-local, and multicast ranges.
/// This is critical for preventing webhook/media fetch SSRF attacks.
pub fn is_safe_ip(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(ipv4) => {
            !ipv4.is_loopback()           // 127.0.0.0/8
                && !ipv4.is_private()     // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                && !ipv4.is_link_local()  // 169.254.0.0/16
                && !ipv4.is_broadcast()   // 255.255.255.255
                && !ipv4.is_multicast()   // 224.0.0.0/4
                && !ipv4.is_unspecified() // 0.0.0.0
                && !is_cgnat_v4(*ipv4)     // 100.64.0.0/10
                && !is_documentation_v4(*ipv4)  // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
                && !is_benchmarking_v4(*ipv4)   // 198.18.0.0/15
        }
        IpAddr::V6(ipv6) => {
            !ipv6.is_loopback()       // ::1
                && !ipv6.is_multicast()
                && !ipv6.is_unspecified() // ::
                // Block IPv4-mapped IPv6 addresses that map to private ranges
                && !is_private_mapped_v6(ipv6)
        }
    }
}

/// CGNAT range: 100.64.0.0/10
fn is_cgnat_v4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (octets[1] & 0xC0) == 64
}

/// Documentation ranges
fn is_documentation_v4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)       // 192.0.2.0/24
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100) // 198.51.100.0/24
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)  // 203.0.113.0/24
}

/// Benchmarking range: 198.18.0.0/15
fn is_benchmarking_v4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 198 && (octets[1] & 0xFE) == 18
}

/// Check if an IPv6 address is a mapped private IPv4.
fn is_private_mapped_v6(ip: &std::net::Ipv6Addr) -> bool {
    ip.to_ipv4_mapped()
        .is_some_and(|ipv4| !is_safe_ip(&IpAddr::V4(ipv4)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn public_ips_are_safe() {
        assert!(is_safe_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_safe_ip(&IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn private_ips_blocked() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
    }

    #[test]
    fn loopback_blocked() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn cgnat_blocked() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(100, 127, 255, 254))));
        // 100.128.0.0 is outside CGNAT
        assert!(is_safe_ip(&IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))));
    }

    #[test]
    fn documentation_blocked() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
    }

    #[test]
    fn safe_extension_handles_additional_media_types() {
        assert_eq!(safe_extension_for_mime("audio/mp4"), "m4a");
        assert_eq!(safe_extension_for_mime("video/quicktime"), "mov");
        assert_eq!(safe_extension_for_mime("text/markdown"), "md");
    }

    #[test]
    fn infer_mime_from_name_recognizes_common_extensions() {
        assert_eq!(infer_mime_from_name("voice-note.m4a"), Some("audio/mp4"));
        assert_eq!(infer_mime_from_name("report.csv"), Some("text/csv"));
        assert_eq!(infer_mime_from_name("unknown.blob"), None);
    }

    #[test]
    fn classify_mime_images() {
        assert_eq!(classify_mime("image/jpeg"), MediaKind::Image);
        assert_eq!(classify_mime("image/png"), MediaKind::Image);
        assert_eq!(classify_mime("image/webp; charset=utf-8"), MediaKind::Image);
    }

    #[test]
    fn classify_mime_audio() {
        assert_eq!(classify_mime("audio/mpeg"), MediaKind::Audio);
        assert_eq!(classify_mime("audio/ogg"), MediaKind::Audio);
    }

    #[test]
    fn classify_mime_video() {
        assert_eq!(classify_mime("video/mp4"), MediaKind::Video);
    }

    #[test]
    fn classify_mime_documents() {
        assert_eq!(classify_mime("application/pdf"), MediaKind::Document);
        assert_eq!(classify_mime("text/plain"), MediaKind::Document);
        assert_eq!(classify_mime("text/csv"), MediaKind::Document);
    }

    #[test]
    fn classify_mime_unknown() {
        assert_eq!(classify_mime("application/octet-stream"), MediaKind::Unknown);
    }

    #[test]
    fn classify_extension_covers_common_types() {
        assert_eq!(classify_extension("photo.jpg"), MediaKind::Image);
        assert_eq!(classify_extension("voice.mp3"), MediaKind::Audio);
        assert_eq!(classify_extension("clip.mp4"), MediaKind::Video);
        assert_eq!(classify_extension("report.pdf"), MediaKind::Document);
        assert_eq!(classify_extension("data.xyz"), MediaKind::Unknown);
    }

    #[test]
    fn classify_extension_case_insensitive() {
        assert_eq!(classify_extension("PHOTO.JPG"), MediaKind::Image);
        assert_eq!(classify_extension("voice.MP3"), MediaKind::Audio);
    }
}
