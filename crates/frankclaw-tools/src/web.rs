//! Web tools: URL fetching and web search.

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};
use frankclaw_core::sanitize::{sanitize_for_prompt, wrap_external_content};

use crate::{Tool, ToolContext};

/// Maximum output chars for web.fetch (hard ceiling).
const MAX_FETCH_CHARS: usize = 200_000;

/// Maximum search query length.
const MAX_SEARCH_QUERY: usize = 400;

// --------------------------------------------------------------------------
// web.fetch
// --------------------------------------------------------------------------

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "web.fetch".into(),
            description: "Fetch a URL and return its content as text or markdown. \
                Useful for reading web pages, API responses, or downloading text content."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "HTTP or HTTPS URL to fetch."
                    },
                    "extract_mode": {
                        "type": "string",
                        "enum": ["markdown", "text"],
                        "description": "How to extract content from HTML. Default: markdown."
                    },
                    "max_chars": {
                        "type": "integer",
                        "minimum": 100,
                        "maximum": 200000,
                        "description": "Maximum characters to return. Default: 50000."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "web.fetch requires a non-empty url".into(),
            })?;

        // Validate scheme.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(FrankClawError::InvalidRequest {
                msg: "web.fetch only supports http and https URLs".into(),
            });
        }

        let extract_mode = args
            .get("extract_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(50_000)
            .clamp(100, MAX_FETCH_CHARS as u64) as usize;

        let fetcher = ctx.fetcher.as_ref().ok_or_else(|| FrankClawError::AgentRuntime {
            msg: "web.fetch is not available: no fetcher service configured".into(),
        })?;

        let content = fetcher.fetch(url).await?;
        let content_type = content.content_type.clone();
        let final_url = content.final_url.clone();

        // Convert bytes to text.
        let raw_text = String::from_utf8_lossy(&content.bytes);

        let (text, title) = if content_type.contains("html") {
            let extracted = extract_html(&raw_text, extract_mode);
            let title = extract_html_title(&raw_text);
            (extracted, title)
        } else {
            (raw_text.into_owned(), None)
        };

        // Sanitize for prompt injection.
        let sanitized = sanitize_for_prompt(&text);
        let truncated = sanitized.len() > max_chars;
        let output_text = if truncated {
            sanitized.chars().take(max_chars).collect::<String>()
        } else {
            sanitized
        };

        // Wrap in external content boundary.
        let wrapped = wrap_external_content(url, &output_text);

        Ok(serde_json::json!({
            "url": url,
            "final_url": final_url,
            "content_type": content_type,
            "title": title,
            "text": wrapped,
            "truncated": truncated,
            "length": output_text.len(),
        }))
    }
}

fn extract_html(html: &str, _mode: &str) -> String {
    // Both "text" and "markdown" modes use html2text conversion.
    html2text::from_read(html.as_bytes(), 120)
        .unwrap_or_else(|_| html.to_string())
}

fn extract_html_title(html: &str) -> Option<String> {
    // Simple regex-free title extraction.
    let lower = html.to_lowercase();
    let start = lower.find("<title")?;
    let tag_end = lower[start..].find('>')?;
    let content_start = start + tag_end + 1;
    let end = lower[content_start..].find("</title>")?;
    let title = html[content_start..content_start + end].trim().to_string();
    if title.is_empty() { None } else { Some(title) }
}

// --------------------------------------------------------------------------
// web.search
// --------------------------------------------------------------------------

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "web.search".into(),
            description: "Search the web using the Brave Search API. \
                Returns titles, URLs, and descriptions of matching web pages."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (max 400 chars)."
                    },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10,
                        "description": "Number of results to return. Default: 5."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, _ctx: ToolContext) -> Result<serde_json::Value> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "web.search requires a non-empty query".into(),
            })?;

        if query.len() > MAX_SEARCH_QUERY {
            return Err(FrankClawError::InvalidRequest {
                msg: format!("web.search query exceeds {} char limit", MAX_SEARCH_QUERY),
            });
        }

        let count = args
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 10);

        let api_key = std::env::var("BRAVE_API_KEY").map_err(|_| FrankClawError::AgentRuntime {
            msg: "web.search requires BRAVE_API_KEY environment variable. \
                Get a free key at https://api.search.brave.com/"
                .into(),
        })?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| FrankClawError::Internal {
                msg: format!("failed to build HTTP client: {e}"),
            })?;

        let response = client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", &api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &count.to_string())])
            .send()
            .await
            .map_err(|e| FrankClawError::AgentRuntime {
                msg: format!("Brave Search API request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(FrankClawError::AgentRuntime {
                msg: format!("Brave Search API returned HTTP {status}: {body}"),
            });
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to parse Brave Search response: {e}"),
            }
        })?;

        let results = parse_brave_results(&body);

        Ok(serde_json::json!({
            "results": results,
            "count": results.len(),
        }))
    }
}

fn parse_brave_results(body: &serde_json::Value) -> Vec<serde_json::Value> {
    let empty = vec![];
    let web_results = body["web"]["results"].as_array().unwrap_or(&empty);

    web_results
        .iter()
        .map(|r| {
            serde_json::json!({
                "title": sanitize_for_prompt(r["title"].as_str().unwrap_or("")),
                "url": r["url"].as_str().unwrap_or(""),
                "description": sanitize_for_prompt(r["description"].as_str().unwrap_or("")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_html_title_basic() {
        let html = r#"<html><head><title>Hello World</title></head><body></body></html>"#;
        assert_eq!(extract_html_title(html), Some("Hello World".into()));
    }

    #[test]
    fn extract_html_title_missing() {
        assert_eq!(extract_html_title("<html><body></body></html>"), None);
    }

    #[test]
    fn extract_html_title_empty() {
        assert_eq!(extract_html_title("<title></title>"), None);
    }

    #[test]
    fn extract_html_converts_to_text() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = extract_html(html, "markdown");
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
    }

    #[test]
    fn parse_brave_results_basic() {
        let body = serde_json::json!({
            "web": {
                "results": [
                    {
                        "title": "Example",
                        "url": "https://example.com",
                        "description": "An example page"
                    }
                ]
            }
        });
        let results = parse_brave_results(&body);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Example");
        assert_eq!(results[0]["url"], "https://example.com");
    }

    #[test]
    fn parse_brave_results_empty() {
        let body = serde_json::json!({});
        let results = parse_brave_results(&body);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_brave_results_sanitizes_text() {
        let body = serde_json::json!({
            "web": {
                "results": [
                    {
                        "title": "Test\u{200B}Title",
                        "url": "https://example.com",
                        "description": "Desc\u{202E}ription"
                    }
                ]
            }
        });
        let results = parse_brave_results(&body);
        assert_eq!(results[0]["title"], "TestTitle");
        assert_eq!(results[0]["description"], "Description");
    }
}
