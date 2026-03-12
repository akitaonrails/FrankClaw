//! Memory tools: read files from the agent's memory directory.

use std::path::Path;

use async_trait::async_trait;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{ToolDef, ToolRiskLevel};

use crate::{Tool, ToolContext};

/// Maximum output size (100 KB).
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Default memory subdirectory name within workspace.
const MEMORY_DIR: &str = "memory";

// --------------------------------------------------------------------------
// memory.get
// --------------------------------------------------------------------------

pub struct MemoryGetTool;

#[async_trait]
impl Tool for MemoryGetTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "memory.get".into(),
            description: "Read a file from the agent's memory directory. \
                Use this to retrieve stored notes, context, or reference data."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path within the memory directory."
                    },
                    "from": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Starting line number (0-based). Default: 0."
                    },
                    "lines": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 5000,
                        "description": "Maximum lines to return. Default: 500."
                    }
                }
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: ToolContext) -> Result<serde_json::Value> {
        let workspace = ctx.workspace.as_deref().ok_or_else(|| FrankClawError::AgentRuntime {
            msg: "memory.get is not available: no workspace directory configured".into(),
        })?;

        let memory_dir = workspace.join(MEMORY_DIR);
        if !memory_dir.exists() {
            return Err(FrankClawError::AgentRuntime {
                msg: "memory directory does not exist".into(),
            });
        }

        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FrankClawError::InvalidRequest {
                msg: "memory.get requires a non-empty path".into(),
            })?;

        // Security: validate the path doesn't escape memory directory.
        validate_memory_path(&memory_dir, path_str)?;

        let resolved = memory_dir.join(path_str);
        let from = args
            .get("from")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let max_lines = args
            .get("lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(500)
            .clamp(1, 5000) as usize;

        let content = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
            FrankClawError::AgentRuntime {
                msg: format!("failed to read memory file '{}': {e}", path_str),
            }
        })?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let selected: String = lines
            .into_iter()
            .skip(from)
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n");

        let truncated = selected.len() > MAX_OUTPUT_BYTES;
        let output = if truncated {
            selected[..MAX_OUTPUT_BYTES].to_string()
        } else {
            selected
        };

        Ok(serde_json::json!({
            "path": path_str,
            "content": output,
            "total_lines": total_lines,
            "from": from,
            "truncated": truncated,
        }))
    }
}

fn validate_memory_path(memory_dir: &Path, requested: &str) -> Result<()> {
    if requested.is_empty() {
        return Err(FrankClawError::InvalidRequest {
            msg: "memory path must not be empty".into(),
        });
    }

    if requested.starts_with('/') || requested.starts_with('\\') {
        return Err(FrankClawError::InvalidRequest {
            msg: "memory path must be relative".into(),
        });
    }

    for component in Path::new(requested).components() {
        if let std::path::Component::ParentDir = component {
            return Err(FrankClawError::InvalidRequest {
                msg: "memory path must not contain '..' components".into(),
            });
        }
    }

    // If the resolved path exists, verify it's inside memory_dir.
    let resolved = memory_dir.join(requested);
    if resolved.exists() {
        let canonical = resolved.canonicalize().map_err(|e| FrankClawError::AgentRuntime {
            msg: format!("failed to resolve memory path: {e}"),
        })?;
        let dir_canonical = memory_dir.canonicalize().map_err(|e| FrankClawError::AgentRuntime {
            msg: format!("failed to resolve memory directory: {e}"),
        })?;
        if !canonical.starts_with(&dir_canonical) {
            return Err(FrankClawError::InvalidRequest {
                msg: "memory path escapes the memory directory".into(),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn validates_memory_paths() {
        let memory_dir = PathBuf::from("/tmp/frankclaw-memory-test");
        std::fs::create_dir_all(&memory_dir).ok();

        // Absolute path rejected.
        assert!(validate_memory_path(&memory_dir, "/etc/passwd").is_err());

        // Parent traversal rejected.
        assert!(validate_memory_path(&memory_dir, "../secrets.txt").is_err());

        // Empty path rejected.
        assert!(validate_memory_path(&memory_dir, "").is_err());

        // Normal relative path accepted.
        assert!(validate_memory_path(&memory_dir, "notes.md").is_ok());
    }

    #[test]
    fn memory_get_definition_is_valid() {
        let tool = MemoryGetTool;
        let def = tool.definition();
        assert_eq!(def.name, "memory.get");
        assert_eq!(def.risk_level, ToolRiskLevel::ReadOnly);
    }
}
