//! Image analysis tool — loads images from workspace and attaches them to the
//! tool result so vision-capable models can describe/analyze them.

use std::path::Path;

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ImageContent, ToolDef, ToolRiskLevel};

use crate::file::validate_workspace_path;
use crate::{Tool, ToolContext};

/// Maximum image file size (20 MB).
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Maximum number of images per invocation.
const MAX_IMAGES: usize = 10;

/// Allowed image MIME types inferred from extension.
const SUPPORTED_EXTENSIONS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
];


fn mime_from_extension(path: &Path) -> Result<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    SUPPORTED_EXTENSIONS
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, mime)| *mime)
        .ok_or_else(|| FrankClawError::InvalidRequest {
            msg: format!(
                "unsupported image format '.{}'. Supported: {}",
                ext,
                SUPPORTED_EXTENSIONS
                    .iter()
                    .map(|(e, _)| format!(".{e}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
}

// --------------------------------------------------------------------------
// image.describe
// --------------------------------------------------------------------------

pub struct ImageDescribeTool;

#[async_trait]
impl Tool for ImageDescribeTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "image_describe".into(),
            description: "Load one or more images from the workspace for visual analysis. \
                The images are sent to the model so it can describe or analyze their contents. \
                Use this when you need to understand what an image shows."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["paths"],
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Relative paths to image files within the workspace (max 10)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "What to analyze about the images. Default: 'Describe the image(s).'"
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let workspace = ctx.require_workspace()?;

        // Parse paths array.
        let paths: Vec<String> = match args.get("paths") {
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
            _ => {
                return Err(FrankClawError::InvalidRequest {
                    msg: "image.describe requires a 'paths' array".into(),
                });
            }
        };

        if paths.is_empty() {
            return Err(FrankClawError::InvalidRequest {
                msg: "image.describe requires at least one path".into(),
            });
        }
        if paths.len() > MAX_IMAGES {
            return Err(FrankClawError::InvalidRequest {
                msg: format!("image.describe accepts at most {} images", MAX_IMAGES),
            });
        }

        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("Describe the image(s).");

        let mut images = Vec::with_capacity(paths.len());
        let mut loaded_paths = Vec::with_capacity(paths.len());

        for path_str in &paths {
            let resolved = validate_workspace_path(workspace, path_str)?;
            let mime = mime_from_extension(&resolved)?;

            let metadata = tokio::fs::metadata(&resolved).await.map_err(|e| {
                FrankClawError::AgentRuntime {
                    msg: format!("failed to read '{}': {e}", path_str),
                }
            })?;

            if metadata.len() > MAX_IMAGE_BYTES {
                return Err(FrankClawError::InvalidRequest {
                    msg: format!(
                        "image '{}' exceeds {} MB limit",
                        path_str,
                        MAX_IMAGE_BYTES / (1024 * 1024)
                    ),
                });
            }

            let bytes = tokio::fs::read(&resolved).await.map_err(|e| {
                FrankClawError::AgentRuntime {
                    msg: format!("failed to read image '{}': {e}", path_str),
                }
            })?;

            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

            images.push(ImageContent {
                mime_type: mime.to_string(),
                data: b64,
            });
            loaded_paths.push(path_str.clone());
        }

        // Return JSON metadata. The actual image data is attached via ToolOutput.image_content
        // in the registry's invoke method — we store images in a thread-local for pickup.
        // For now, we use a convention: include _image_content in the output JSON which the
        // registry will extract.
        Ok(serde_json::json!({
            "prompt": prompt,
            "images_loaded": loaded_paths,
            "count": loaded_paths.len(),
            "_image_content": images.iter().map(|img| {
                serde_json::json!({
                    "mime_type": img.mime_type,
                    "data": img.data,
                })
            }).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_detection_jpg() {
        let path = Path::new("photo.jpg");
        assert_eq!(mime_from_extension(path).unwrap(), "image/jpeg");
    }

    #[test]
    fn mime_detection_png() {
        let path = Path::new("screenshot.PNG");
        assert_eq!(mime_from_extension(path).unwrap(), "image/png");
    }

    #[test]
    fn mime_detection_unsupported() {
        let path = Path::new("doc.pdf");
        assert!(mime_from_extension(path).is_err());
    }

    #[test]
    fn image_describe_definition_is_valid() {
        let tool = ImageDescribeTool;
        let def = tool.definition();
        assert_eq!(def.name, "image_describe");
        assert_eq!(def.risk_level, ToolRiskLevel::ReadOnly);
    }
}
