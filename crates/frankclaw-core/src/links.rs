//! URL extraction from messages with SSRF-safe filtering.
//!
//! Extracts bare HTTP(S) URLs from user messages, deduplicating and filtering
//! out SSRF-unsafe hosts. Skips URLs already inside markdown link syntax.

use crate::media::is_safe_ip;

/// Maximum number of URLs to extract from a single message.
const DEFAULT_MAX_LINKS: usize = 3;

/// Check if a character is valid in a URL (simplified — covers common cases).
fn is_url_char(c: char) -> bool {
    !c.is_whitespace()
        && c != '<'
        && c != '>'
        && c != '"'
        && c != '\''
        && c != '`'
}

/// Strip markdown link syntax `[text](url)` from a message, replacing with spaces.
fn strip_markdown_links(message: &str) -> String {
    let mut result = String::with_capacity(message.len());
    let chars: Vec<char> = message.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '[' {
            // Look for `](http` pattern
            if let Some(close_bracket) = chars[i + 1..].iter().position(|&c| c == ']') {
                let close_idx = i + 1 + close_bracket;
                if close_idx + 1 < chars.len() && chars[close_idx + 1] == '(' {
                    // Check if the paren content starts with http
                    let paren_start = close_idx + 2;
                    let rest: String = chars[paren_start..].iter().collect();
                    if rest.starts_with("http://") || rest.starts_with("https://") {
                        // Find closing paren
                        if let Some(close_paren) = chars[paren_start..].iter().position(|&c| c == ')') {
                            // Skip the entire markdown link
                            i = paren_start + close_paren + 1;
                            result.push(' ');
                            continue;
                        }
                    }
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Extract a bare URL starting at the given position in the text.
fn extract_url_at(text: &str) -> Option<&str> {
    if !text.starts_with("http://") && !text.starts_with("https://") {
        return None;
    }

    let end = text
        .find(|c: char| !is_url_char(c))
        .unwrap_or(text.len());

    if end <= "https://".len() {
        return None;
    }

    let raw = &text[..end];

    // Trim trailing punctuation that's typically not part of URLs.
    let trimmed = raw.trim_end_matches(['.', ',', ')', ']', '>', ';', '!', '?']);

    if trimmed.len() <= "https://".len() {
        return None;
    }

    Some(trimmed)
}

/// Check if a URL is safe to fetch (public IP, HTTP(S) only).
fn is_allowed_url(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    // If host parses as an IP, check SSRF blocklist directly.
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_safe_ip(&ip);
    }
    // For hostnames, block obviously dangerous ones.
    // Full DNS resolution + IP check happens at fetch time via SafeFetcher.
    if host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host == "metadata.google.internal"
    {
        return false;
    }
    true
}

/// Extract unique HTTP(S) URLs from a message.
///
/// - Strips markdown link syntax first (so `[click](url)` is ignored).
/// - Deduplicates URLs.
/// - Filters out SSRF-unsafe URLs.
/// - Caps at `max_links` results.
pub fn extract_links(message: &str, max_links: Option<usize>) -> Vec<String> {
    let message = message.trim();
    if message.is_empty() {
        return Vec::new();
    }

    let max = max_links.unwrap_or(DEFAULT_MAX_LINKS);
    let stripped = strip_markdown_links(message);

    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();

    let mut search_from = 0;
    while search_from < stripped.len() {
        // Find next "http://" or "https://"
        let rest = &stripped[search_from..];
        let http_pos = match (rest.find("http://"), rest.find("https://")) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };

        let Some(pos) = http_pos else {
            break;
        };

        let url_start = search_from + pos;
        if let Some(url) = extract_url_at(&stripped[url_start..]) {
            search_from = url_start + url.len();

            if is_allowed_url(url) && seen.insert(url.to_string()) {
                results.push(url.to_string());
                if results.len() >= max {
                    break;
                }
            }
        } else {
            search_from = url_start + 1;
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bare_urls() {
        let msg = "Check out https://example.com and http://test.org for info";
        let links = extract_links(msg, None);
        assert_eq!(links, vec!["https://example.com", "http://test.org"]);
    }

    #[test]
    fn skip_markdown_links() {
        let msg = "See [docs](https://docs.example.com) and also https://bare.example.com";
        let links = extract_links(msg, None);
        assert_eq!(links, vec!["https://bare.example.com"]);
    }

    #[test]
    fn deduplicate_urls() {
        let msg = "Visit https://example.com then https://example.com again";
        let links = extract_links(msg, None);
        assert_eq!(links, vec!["https://example.com"]);
    }

    #[test]
    fn respect_max_links() {
        let msg = "https://a.com https://b.com https://c.com https://d.com";
        let links = extract_links(msg, Some(2));
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn block_localhost() {
        let msg = "http://localhost:8080/secret and http://127.0.0.1/internal";
        let links = extract_links(msg, None);
        assert!(links.is_empty());
    }

    #[test]
    fn block_metadata_endpoint() {
        let msg = "http://169.254.169.254/latest/meta-data/";
        let links = extract_links(msg, None);
        assert!(links.is_empty());
    }

    #[test]
    fn block_internal_hosts() {
        let msg = "https://service.internal/api https://host.local/data";
        let links = extract_links(msg, None);
        assert!(links.is_empty());
    }

    #[test]
    fn trim_trailing_punctuation() {
        let msg = "Check https://example.com. Also https://test.org, ok?";
        let links = extract_links(msg, None);
        assert_eq!(links, vec!["https://example.com", "https://test.org"]);
    }

    #[test]
    fn empty_message_returns_empty() {
        assert!(extract_links("", None).is_empty());
        assert!(extract_links("   ", None).is_empty());
    }

    #[test]
    fn no_urls_returns_empty() {
        let msg = "No URLs here, just plain text.";
        assert!(extract_links(msg, None).is_empty());
    }

    #[test]
    fn ftp_urls_ignored() {
        let msg = "Download from ftp://files.example.com/data.zip";
        assert!(extract_links(msg, None).is_empty());
    }

    #[test]
    fn urls_with_paths_and_query_strings() {
        let msg = "Visit https://example.com/path/to/page?q=hello&lang=en#section";
        let links = extract_links(msg, None);
        assert_eq!(
            links,
            vec!["https://example.com/path/to/page?q=hello&lang=en#section"]
        );
    }

    #[test]
    fn private_ip_ranges_blocked() {
        let msg = "http://10.0.0.1/admin http://192.168.1.1/config http://172.16.0.1/internal";
        let links = extract_links(msg, None);
        assert!(links.is_empty());
    }

    #[test]
    fn strip_markdown_links_preserves_text() {
        let result = strip_markdown_links("Hello [click here](https://example.com) world");
        assert!(!result.contains("https://example.com"));
        assert!(result.contains("Hello"));
        assert!(result.contains("world"));
    }
}
