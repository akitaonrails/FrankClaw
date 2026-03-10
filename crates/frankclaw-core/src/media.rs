use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

use crate::types::MediaId;

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
                && !is_cgnat_v4(ipv4)     // 100.64.0.0/10
                && !is_documentation_v4(ipv4)  // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
                && !is_benchmarking_v4(ipv4)   // 198.18.0.0/15
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
fn is_cgnat_v4(ip: &std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (octets[1] & 0xC0) == 64
}

/// Documentation ranges
fn is_documentation_v4(ip: &std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)       // 192.0.2.0/24
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100) // 198.51.100.0/24
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)  // 203.0.113.0/24
}

/// Benchmarking range: 198.18.0.0/15
fn is_benchmarking_v4(ip: &std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 198 && (octets[1] & 0xFE) == 18
}

/// Check if an IPv6 address is a mapped private IPv4.
fn is_private_mapped_v6(ip: &std::net::Ipv6Addr) -> bool {
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        !is_safe_ip(&IpAddr::V4(ipv4))
    } else {
        false
    }
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
}
