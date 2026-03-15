//! PDF text extraction tool.

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};

use crate::{Tool, ToolContext};
use crate::file::validate_workspace_path;

/// Maximum PDF file size (10 MB).
const MAX_PDF_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum pages to extract (0 = all).
const DEFAULT_MAX_PAGES: usize = 20;

/// Maximum output characters.
const MAX_OUTPUT_CHARS: usize = 200_000;


// --------------------------------------------------------------------------
// pdf.read
// --------------------------------------------------------------------------

pub struct PdfReadTool;

#[async_trait]
impl Tool for PdfReadTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "pdf_read".into(),
            description: "Extract text content from a PDF file in the workspace. \
                Returns the extracted text with page markers."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to a PDF file within the workspace."
                    },
                    "pages": {
                        "type": "string",
                        "description": "Page range to extract (e.g. '1-5', '1,3,7'). Default: first 20 pages."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let workspace = ctx.require_workspace()?;
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "pdf.read requires a path".into(),
            })?;

        let resolved = validate_workspace_path(workspace, path_str)?;

        // Verify the file exists and is a PDF.
        let metadata = tokio::fs::metadata(&resolved).await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to read '{}': {e}", path_str),
            }
        })?;

        if metadata.len() > MAX_PDF_BYTES {
            return Err(FrankClawError::InvalidRequest {
                msg: format!(
                    "PDF file exceeds {} MB limit",
                    MAX_PDF_BYTES / (1024 * 1024)
                ),
            });
        }

        // Parse page ranges.
        let page_filter = args
            .get("pages")
            .and_then(|v| v.as_str())
            .map(parse_page_ranges)
            .transpose()?;

        let max_pages = page_filter
            .as_ref()
            .map(|pages| pages.len())
            .unwrap_or(DEFAULT_MAX_PAGES);

        // Read the PDF file.
        let pdf_bytes = tokio::fs::read(&resolved).await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to read PDF '{}': {e}", path_str),
            }
        })?;

        // Extract text (blocking operation, offload to thread pool).
        let text = tokio::task::spawn_blocking(move || {
            pdf_extract::extract_text_from_mem(&pdf_bytes).map_err(|e| {
                FrankClawError::AgentRuntime {
                    msg: format!("failed to extract text from PDF: {e}"),
                }
            })
        })
        .await
        .map_err(|e| FrankClawError::Internal {
            msg: format!("PDF extraction task failed: {e}"),
        })??;

        // Split into pages (pdf-extract separates pages with form-feed chars).
        let pages: Vec<&str> = text.split('\u{0C}').collect();
        let total_pages = pages.len();

        // Apply page filter or default limit.
        let selected_pages: Vec<(usize, &str)> = if let Some(ref indices) = page_filter {
            indices
                .iter()
                .filter(|&&i| i <= total_pages && i > 0)
                .map(|&i| (i, pages[i - 1]))
                .collect()
        } else {
            pages
                .iter()
                .enumerate()
                .take(max_pages)
                .map(|(i, p)| (i + 1, *p))
                .collect()
        };

        // Build output with page markers.
        let mut output = String::new();
        for (page_num, page_text) in &selected_pages {
            let trimmed = page_text.trim();
            if !trimmed.is_empty() {
                output.push_str(&format!("--- Page {} ---\n{}\n\n", page_num, trimmed));
            }
        }

        let truncated = output.len() > MAX_OUTPUT_CHARS;
        if truncated {
            output.truncate(MAX_OUTPUT_CHARS);
            output.push_str("\n... [truncated]");
        }

        Ok(serde_json::json!({
            "path": path_str,
            "total_pages": total_pages,
            "pages_extracted": selected_pages.len(),
            "text": output,
            "truncated": truncated,
        }))
    }
}

/// Parse page range strings like "1-5", "1,3,7-9" into a sorted list of page numbers.
fn parse_page_ranges(input: &str) -> Result<Vec<usize>> {
    let mut pages = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: usize = start.trim().parse().map_err(|_| FrankClawError::InvalidRequest {
                msg: format!("invalid page range: '{}'", input),
            })?;
            let end: usize = end.trim().parse().map_err(|_| FrankClawError::InvalidRequest {
                msg: format!("invalid page range: '{}'", input),
            })?;
            if start == 0 || end == 0 || start > end || end > 10000 {
                return Err(FrankClawError::InvalidRequest {
                    msg: format!("invalid page range: '{}'", part),
                });
            }
            pages.extend(start..=end);
        } else {
            let page: usize = part.parse().map_err(|_| FrankClawError::InvalidRequest {
                msg: format!("invalid page number: '{}'", part),
            })?;
            if page == 0 || page > 10000 {
                return Err(FrankClawError::InvalidRequest {
                    msg: format!("invalid page number: '{}'", part),
                });
            }
            pages.push(page);
        }
    }
    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_pages() {
        let pages = parse_page_ranges("1,3,7").unwrap();
        assert_eq!(pages, vec![1, 3, 7]);
    }

    #[test]
    fn parse_page_range() {
        let pages = parse_page_ranges("1-5").unwrap();
        assert_eq!(pages, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn parse_mixed_ranges() {
        let pages = parse_page_ranges("1-3,7,10-12").unwrap();
        assert_eq!(pages, vec![1, 2, 3, 7, 10, 11, 12]);
    }

    #[test]
    fn parse_deduplicates() {
        let pages = parse_page_ranges("1-3,2-4").unwrap();
        assert_eq!(pages, vec![1, 2, 3, 4]);
    }

    #[test]
    fn parse_rejects_zero_page() {
        assert!(parse_page_ranges("0").is_err());
    }

    #[test]
    fn parse_rejects_invalid_range() {
        assert!(parse_page_ranges("5-3").is_err());
    }

    #[test]
    fn pdf_read_definition_is_valid() {
        let tool = PdfReadTool;
        let def = tool.definition();
        assert_eq!(def.name, "pdf_read");
        assert_eq!(def.risk_level, ToolRiskLevel::ReadOnly);
    }
}
