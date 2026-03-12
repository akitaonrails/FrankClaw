//! File system tools: read, write, and edit files within a workspace directory.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};

use crate::{Tool, ToolContext};

/// Maximum file read output (200 KB).
const MAX_READ_BYTES: usize = 200 * 1024;

/// Maximum write content (1 MB).
const MAX_WRITE_BYTES: usize = 1024 * 1024;

/// Validate and resolve a requested path within the workspace.
///
/// - Rejects absolute paths
/// - Rejects `..` components
/// - Resolves to an absolute path inside the workspace
/// - Rejects symlinks that escape the workspace
fn validate_workspace_path(workspace: &Path, requested: &str) -> Result<PathBuf> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(FrankClawError::InvalidRequest {
            msg: "file path must not be empty".into(),
        });
    }

    // Reject absolute paths.
    if requested.starts_with('/') || requested.starts_with('\\') {
        return Err(FrankClawError::InvalidRequest {
            msg: "file path must be relative to the workspace directory".into(),
        });
    }

    // Reject .. traversal.
    for component in Path::new(requested).components() {
        if let std::path::Component::ParentDir = component {
            return Err(FrankClawError::InvalidRequest {
                msg: "file path must not contain '..' components".into(),
            });
        }
    }

    let resolved = workspace.join(requested);

    // If the path exists, canonicalize and verify it's inside workspace.
    if resolved.exists() {
        let canonical = resolved.canonicalize().map_err(|e| FrankClawError::AgentRuntime {
            msg: format!("failed to resolve path: {e}"),
        })?;
        let workspace_canonical = workspace.canonicalize().map_err(|e| FrankClawError::AgentRuntime {
            msg: format!("failed to resolve workspace: {e}"),
        })?;
        if !canonical.starts_with(&workspace_canonical) {
            return Err(FrankClawError::InvalidRequest {
                msg: "file path escapes the workspace directory (symlink?)".into(),
            });
        }
        Ok(canonical)
    } else {
        // For new files, verify the parent exists and is inside workspace.
        if let Some(parent) = resolved.parent().filter(|p| p.exists()) {
            let parent_canonical =
                parent.canonicalize().map_err(|e| FrankClawError::AgentRuntime {
                    msg: format!("failed to resolve parent directory: {e}"),
                })?;
            let workspace_canonical =
                workspace.canonicalize().map_err(|e| FrankClawError::AgentRuntime {
                    msg: format!("failed to resolve workspace: {e}"),
                })?;
            if !parent_canonical.starts_with(&workspace_canonical) {
                return Err(FrankClawError::InvalidRequest {
                    msg: "file path escapes the workspace directory".into(),
                });
            }
        }
        Ok(resolved)
    }
}

fn get_workspace(ctx: &ToolContext) -> Result<&Path> {
    ctx.workspace.as_deref().ok_or_else(|| FrankClawError::AgentRuntime {
        msg: "file tools are not available: no workspace directory configured".into(),
    })
}

// --------------------------------------------------------------------------
// file.read
// --------------------------------------------------------------------------

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "file.read".into(),
            description: "Read a file from the workspace directory. \
                Returns the file content with line numbers."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path within the workspace."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Starting line number (0-based). Default: 0."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10000,
                        "description": "Maximum lines to return. Default: 2000."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let workspace = get_workspace(&ctx)?;
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "file.read requires a path".into(),
            })?;
        let resolved = validate_workspace_path(workspace, path_str)?;
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000)
            .clamp(1, 10000) as usize;

        let content = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to read file '{}': {e}", path_str),
            }
        })?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let selected: Vec<String> = lines
            .into_iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", offset + i + 1, line))
            .collect();

        let output = selected.join("\n");
        let truncated = output.len() > MAX_READ_BYTES;
        let final_output = if truncated {
            output[..MAX_READ_BYTES].to_string()
        } else {
            output
        };

        Ok(serde_json::json!({
            "path": path_str,
            "content": final_output,
            "total_lines": total_lines,
            "offset": offset,
            "lines_returned": selected.len().min(if truncated { limit } else { selected.len() }),
            "truncated": truncated,
        }))
    }
}

// --------------------------------------------------------------------------
// file.write
// --------------------------------------------------------------------------

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "file.write".into(),
            description: "Create or overwrite a file in the workspace directory."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path within the workspace."
                    },
                    "content": {
                        "type": "string",
                        "description": "File content to write (max 1MB)."
                    }
                }
            }),
            risk_level: ToolRiskLevel::Mutating,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let workspace = get_workspace(&ctx)?;
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "file.write requires a path".into(),
            })?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "file.write requires content".into(),
            })?;

        if content.len() > MAX_WRITE_BYTES {
            return Err(FrankClawError::InvalidRequest {
                msg: format!(
                    "file.write content exceeds {} byte limit",
                    MAX_WRITE_BYTES
                ),
            });
        }

        let resolved = validate_workspace_path(workspace, path_str)?;

        // Create parent directories.
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                FrankClawError::AgentRuntime {
                    msg: format!("failed to create directories for '{}': {e}", path_str),
                }
            })?;
        }

        tokio::fs::write(&resolved, content).await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to write file '{}': {e}", path_str),
            }
        })?;

        // Set file permissions to owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::set_permissions(&resolved, std::fs::Permissions::from_mode(0o600)).await;
        }

        Ok(serde_json::json!({
            "path": path_str,
            "bytes_written": content.len(),
            "status": "ok",
        }))
    }
}

// --------------------------------------------------------------------------
// file.edit
// --------------------------------------------------------------------------

pub struct FileEditTool;

#[async_trait]
impl Tool for FileEditTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "file.edit".into(),
            description: "Search and replace text in a file. \
                The old_text must match exactly once in the file."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["path", "old_text", "new_text"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path within the workspace."
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Exact text to find and replace. Must match exactly once."
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text."
                    }
                }
            }),
            risk_level: ToolRiskLevel::Mutating,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let workspace = get_workspace(&ctx)?;
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "file.edit requires a path".into(),
            })?;
        let old_text = args
            .get("old_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "file.edit requires old_text".into(),
            })?;
        let new_text = args
            .get("new_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "file.edit requires new_text".into(),
            })?;

        if old_text.is_empty() {
            return Err(FrankClawError::InvalidRequest {
                msg: "file.edit old_text must not be empty".into(),
            });
        }

        let resolved = validate_workspace_path(workspace, path_str)?;
        let content = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to read file '{}': {e}", path_str),
            }
        })?;

        let match_count = content.matches(old_text).count();
        if match_count == 0 {
            return Err(FrankClawError::AgentRuntime {
                msg: format!("file.edit: old_text not found in '{}'", path_str),
            });
        }
        if match_count > 1 {
            return Err(FrankClawError::AgentRuntime {
                msg: format!(
                    "file.edit: old_text matches {} times in '{}' (must match exactly once)",
                    match_count, path_str
                ),
            });
        }

        let new_content = content.replacen(old_text, new_text, 1);

        if new_content.len() > MAX_WRITE_BYTES {
            return Err(FrankClawError::InvalidRequest {
                msg: format!(
                    "file.edit result would exceed {} byte limit",
                    MAX_WRITE_BYTES
                ),
            });
        }

        tokio::fs::write(&resolved, &new_content).await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to write file '{}': {e}", path_str),
            }
        })?;

        Ok(serde_json::json!({
            "path": path_str,
            "status": "ok",
            "bytes_written": new_content.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn validates_relative_paths() {
        let workspace = PathBuf::from("/tmp/test-workspace");
        std::fs::create_dir_all(&workspace).ok();

        // Absolute paths rejected.
        assert!(validate_workspace_path(&workspace, "/etc/passwd").is_err());

        // Parent traversal rejected.
        assert!(validate_workspace_path(&workspace, "../etc/passwd").is_err());
        assert!(validate_workspace_path(&workspace, "foo/../../etc/passwd").is_err());

        // Empty path rejected.
        assert!(validate_workspace_path(&workspace, "").is_err());
    }

    #[test]
    fn validates_normal_relative_path() {
        let workspace = std::env::temp_dir().join("frankclaw-file-test");
        std::fs::create_dir_all(&workspace).ok();

        // Normal relative path should work.
        let result = validate_workspace_path(&workspace, "hello.txt");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn file_edit_rejects_ambiguous_match() {
        let workspace = std::env::temp_dir().join("frankclaw-edit-test");
        std::fs::create_dir_all(&workspace).ok();
        let test_file = workspace.join("test.txt");
        std::fs::write(&test_file, "aaa bbb aaa").ok();

        let tool = FileEditTool;
        let ctx = crate::test_tool_context(Some(workspace.clone()));

        let result = tool
            .invoke(
                serde_json::json!({
                    "path": "test.txt",
                    "old_text": "aaa",
                    "new_text": "ccc"
                }),
                ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("matches 2 times"));

        // Cleanup.
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[tokio::test]
    async fn file_read_write_roundtrip() {
        let workspace = std::env::temp_dir().join("frankclaw-rw-test");
        std::fs::create_dir_all(&workspace).ok();

        let ctx = crate::test_tool_context(Some(workspace.clone()));

        // Write.
        let write_tool = FileWriteTool;
        let result = write_tool
            .invoke(
                serde_json::json!({
                    "path": "hello.txt",
                    "content": "line1\nline2\nline3"
                }),
                ctx.clone(),
            )
            .await
            .expect("write should succeed");
        assert_eq!(result["status"], "ok");

        // Read.
        let read_tool = FileReadTool;
        let result = read_tool
            .invoke(
                serde_json::json!({ "path": "hello.txt" }),
                ctx,
            )
            .await
            .expect("read should succeed");
        assert_eq!(result["total_lines"], 3);
        let content = result["content"].as_str().unwrap();
        assert!(content.contains("line1"));
        assert!(content.contains("line3"));

        // Cleanup.
        std::fs::remove_dir_all(&workspace).ok();
    }
}
