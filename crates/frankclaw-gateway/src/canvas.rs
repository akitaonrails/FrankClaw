use std::sync::Arc;

use std::collections::HashMap;

use async_trait::async_trait;
use frankclaw_core::canvas as core_canvas;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::protocol::{EventFrame, EventType, Frame};

/// Maximum total document size (title + body + all block text) in bytes.
const MAX_DOCUMENT_SIZE: usize = 1_024 * 1_024;

/// Maximum number of blocks per document.
const MAX_BLOCKS_PER_DOCUMENT: usize = 200;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanvasBlockKind {
    Markdown,
    Code,
    Note,
    Checklist,
    Status,
    Metric,
    Action,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CanvasBlock {
    pub kind: CanvasBlockKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanvasDocument {
    pub id: String,
    pub title: String,
    pub body: String,
    pub session_key: Option<String>,
    #[serde(default)]
    pub blocks: Vec<CanvasBlock>,
    pub revision: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct CanvasPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub session_key: Option<Option<String>>,
    pub append_blocks: Vec<CanvasBlock>,
    /// If set, the patch is rejected unless the current revision matches.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasExportFormat {
    Json,
    Markdown,
}

impl CanvasExportFormat {
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            Some("markdown") | Some("md") => Self::Markdown,
            _ => Self::Json,
        }
    }

    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Json => "application/json; charset=utf-8",
            Self::Markdown => "text/markdown; charset=utf-8",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "markdown",
        }
    }
}

pub struct CanvasStore {
    documents: tokio::sync::RwLock<HashMap<String, CanvasDocument>>,
    broadcast: Option<tokio::sync::broadcast::Sender<String>>,
}

impl CanvasStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            documents: tokio::sync::RwLock::new(HashMap::new()),
            broadcast: None,
        })
    }

    pub fn with_broadcast(broadcast: tokio::sync::broadcast::Sender<String>) -> Arc<Self> {
        Arc::new(Self {
            documents: tokio::sync::RwLock::new(HashMap::new()),
            broadcast: Some(broadcast),
        })
    }

    fn notify(&self, canvas_id: &str, document: Option<&CanvasDocument>) {
        if let Some(tx) = &self.broadcast {
            let event = Frame::Event(EventFrame {
                event: EventType::CanvasUpdated,
                payload: serde_json::json!({
                    "canvas_id": canvas_id,
                    "canvas": document,
                }),
            });
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = tx.send(json);
            }
        }
    }

    pub fn key_for(canvas_id: Option<&str>, session_key: Option<&str>) -> String {
        if let Some(canvas_id) = canvas_id.map(str::trim).filter(|value| !value.is_empty()) {
            return canvas_id.to_string();
        }
        if let Some(session_key) = session_key.map(str::trim).filter(|value| !value.is_empty()) {
            return format!("session:{session_key}");
        }
        "main".to_string()
    }

    pub async fn get(&self, canvas_id: &str) -> Option<CanvasDocument> {
        self.documents.read().await.get(canvas_id).cloned()
    }

    pub async fn set(&self, mut document: CanvasDocument) -> Result<CanvasDocument> {
        validate_document_size(&document)?;
        let mut documents = self.documents.write().await;
        let next_revision = documents
            .get(&document.id)
            .map(|existing| existing.revision + 1)
            .unwrap_or(1);
        document.revision = next_revision;
        documents.insert(document.id.clone(), document.clone());
        Ok(document)
    }

    pub async fn patch(&self, canvas_id: &str, patch: CanvasPatch) -> Result<CanvasDocument> {
        let mut documents = self.documents.write().await;
        let existing = documents
            .get(canvas_id)
            .cloned()
            .unwrap_or_else(|| CanvasDocument {
                id: canvas_id.to_string(),
                title: String::new(),
                body: String::new(),
                session_key: None,
                blocks: Vec::new(),
                revision: 0,
                updated_at: chrono::Utc::now(),
            });
        // Conflict detection: reject stale patches.
        if let Some(expected) = patch.expected_revision {
            if existing.revision != expected {
                return Err(FrankClawError::InvalidRequest {
                    msg: format!(
                        "canvas revision conflict: expected {expected}, current is {}",
                        existing.revision
                    ),
                });
            }
        }
        let mut document = existing;
        if let Some(title) = patch.title {
            document.title = title;
        }
        if let Some(body) = patch.body {
            document.body = body;
        }
        if let Some(session_key) = patch.session_key {
            document.session_key = session_key;
        }
        document.blocks.extend(patch.append_blocks);
        // Enforce block count limit.
        if document.blocks.len() > MAX_BLOCKS_PER_DOCUMENT {
            return Err(FrankClawError::InvalidRequest {
                msg: format!(
                    "canvas block count exceeds limit ({} > {MAX_BLOCKS_PER_DOCUMENT})",
                    document.blocks.len()
                ),
            });
        }
        validate_document_size(&document)?;
        document.revision += 1;
        document.updated_at = chrono::Utc::now();
        documents.insert(document.id.clone(), document.clone());
        Ok(document)
    }

    pub async fn clear(&self, canvas_id: &str) {
        self.documents.write().await.remove(canvas_id);
    }
}

// -- Conversions between gateway canvas types and core canvas types --

fn core_block_to_local(block: &core_canvas::CanvasBlock) -> CanvasBlock {
    let kind: CanvasBlockKind = serde_json::from_value(
        serde_json::Value::String(block.kind.clone()),
    )
    .unwrap_or(CanvasBlockKind::Markdown);
    CanvasBlock {
        kind,
        text: block.text.clone(),
        meta: block.meta.clone(),
    }
}

fn local_block_to_core(block: &CanvasBlock) -> core_canvas::CanvasBlock {
    let kind_str = serde_json::to_value(&block.kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "markdown".into());
    core_canvas::CanvasBlock {
        kind: kind_str,
        text: block.text.clone(),
        meta: block.meta.clone(),
    }
}

fn local_doc_to_core(doc: &CanvasDocument) -> core_canvas::CanvasDocument {
    core_canvas::CanvasDocument {
        id: doc.id.clone(),
        title: doc.title.clone(),
        body: doc.body.clone(),
        session_key: doc.session_key.clone(),
        blocks: doc.blocks.iter().map(local_block_to_core).collect(),
        revision: doc.revision,
        updated_at: doc.updated_at,
    }
}

fn core_doc_to_local(doc: &core_canvas::CanvasDocument) -> CanvasDocument {
    CanvasDocument {
        id: doc.id.clone(),
        title: doc.title.clone(),
        body: doc.body.clone(),
        session_key: doc.session_key.clone(),
        blocks: doc.blocks.iter().map(core_block_to_local).collect(),
        revision: doc.revision,
        updated_at: doc.updated_at,
    }
}

#[async_trait]
impl core_canvas::CanvasService for CanvasStore {
    async fn get(&self, canvas_id: &str) -> Option<core_canvas::CanvasDocument> {
        self.get(canvas_id).await.as_ref().map(local_doc_to_core)
    }

    async fn set(&self, document: core_canvas::CanvasDocument) -> Result<core_canvas::CanvasDocument> {
        let local = core_doc_to_local(&document);
        let result = self.set(local).await?;
        self.notify(&result.id, Some(&result));
        Ok(local_doc_to_core(&result))
    }

    async fn patch(
        &self,
        canvas_id: &str,
        title: Option<String>,
        body: Option<String>,
        append_blocks: Vec<core_canvas::CanvasBlock>,
    ) -> Result<core_canvas::CanvasDocument> {
        let local_blocks: Vec<CanvasBlock> = append_blocks.iter().map(core_block_to_local).collect();
        let patch = CanvasPatch {
            title,
            body,
            session_key: None,
            append_blocks: local_blocks,
            expected_revision: None,
        };
        let result = self.patch(canvas_id, patch).await?;
        self.notify(&result.id, Some(&result));
        Ok(local_doc_to_core(&result))
    }

    async fn clear(&self, canvas_id: &str) {
        self.clear(canvas_id).await;
        self.notify(canvas_id, None);
    }
}

pub fn export_document(document: &CanvasDocument, format: CanvasExportFormat) -> String {
    match format {
        CanvasExportFormat::Json => serde_json::to_string_pretty(document)
            .unwrap_or_else(|_| "{}".to_string()),
        CanvasExportFormat::Markdown => render_markdown(document),
    }
}

fn render_markdown(document: &CanvasDocument) -> String {
    let mut sections = Vec::new();

    if !document.title.trim().is_empty() {
        sections.push(format!("# {}", document.title.trim()));
    }

    let mut metadata = vec![
        format!("Canvas: {}", document.id),
        format!("Revision: {}", document.revision),
        format!("Updated: {}", document.updated_at.to_rfc3339()),
    ];
    if let Some(session_key) = document.session_key.as_deref().filter(|value| !value.trim().is_empty()) {
        metadata.push(format!("Session: {}", session_key.trim()));
    }
    sections.push(metadata.join("\n"));

    if !document.body.trim().is_empty() {
        sections.push(document.body.trim().to_string());
    }

    if !document.blocks.is_empty() {
        let blocks = document
            .blocks
            .iter()
            .map(render_markdown_block)
            .collect::<Vec<_>>()
            .join("\n\n");
        sections.push(blocks);
    }

    sections.join("\n\n")
}

fn validate_document_size(document: &CanvasDocument) -> Result<()> {
    let total = document.title.len()
        + document.body.len()
        + document.blocks.iter().map(|b| b.text.len()).sum::<usize>();
    if total > MAX_DOCUMENT_SIZE {
        return Err(FrankClawError::InvalidRequest {
            msg: format!(
                "canvas document size exceeds limit ({total} bytes > {MAX_DOCUMENT_SIZE})"
            ),
        });
    }
    Ok(())
}

/// Strip HTML tags from text to prevent injection in markdown export.
fn strip_html_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

fn render_markdown_block(block: &CanvasBlock) -> String {
    let text = strip_html_tags(block.text.trim());
    let text = text.as_str();
    match block.kind {
        CanvasBlockKind::Markdown => text.to_string(),
        CanvasBlockKind::Code => format!("```text\n{}\n```", text),
        CanvasBlockKind::Note => text
            .lines()
            .map(|line| format!("> {}", line.trim()))
            .collect::<Vec<_>>()
            .join("\n"),
        CanvasBlockKind::Checklist => text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let line = line.trim();
                if line.starts_with("- [") {
                    line.to_string()
                } else {
                    format!("- [ ] {}", line)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        CanvasBlockKind::Status => {
            let level = block
                .meta
                .as_ref()
                .and_then(|meta| meta.get("level"))
                .and_then(|value| value.as_str())
                .unwrap_or("info");
            format!("**Status ({level})**\n{text}")
        }
        CanvasBlockKind::Metric => {
            let value = block
                .meta
                .as_ref()
                .and_then(|meta| meta.get("value"))
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string())
                })
                .unwrap_or_else(|| text.to_string());
            if text.is_empty() || text == value {
                format!("**Metric:** {value}")
            } else {
                format!("**Metric:** {text} = {value}")
            }
        }
        CanvasBlockKind::Action => {
            let action = block
                .meta
                .as_ref()
                .and_then(|meta| meta.get("action"))
                .and_then(|value| value.as_str())
                .unwrap_or("noop");
            let target = block
                .meta
                .as_ref()
                .and_then(|meta| meta.get("target"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            format!("**Action ({action})**\n{}\n{}", text, target)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_document_renders_markdown_snapshot() {
        let document = CanvasDocument {
            id: "ops".into(),
            title: "Ops Runbook".into(),
            body: "Current deployment summary".into(),
            session_key: Some("default:web:control".into()),
            blocks: vec![
                CanvasBlock {
                    kind: CanvasBlockKind::Note,
                    text: "deploy window open".into(),
                    meta: None,
                },
                CanvasBlock {
                    kind: CanvasBlockKind::Checklist,
                    text: "verify smoke tests\nnotify team".into(),
                    meta: None,
                },
            ],
            revision: 3,
            updated_at: chrono::DateTime::from_timestamp(1_710_000_000, 0).unwrap(),
        };

        let export = export_document(&document, CanvasExportFormat::Markdown);
        assert!(export.contains("# Ops Runbook"));
        assert!(export.contains("Session: default:web:control"));
        assert!(export.contains("> deploy window open"));
        assert!(export.contains("- [ ] verify smoke tests"));
    }

    #[test]
    fn export_document_renders_structured_component_blocks() {
        let document = CanvasDocument {
            id: "status".into(),
            title: String::new(),
            body: String::new(),
            session_key: None,
            blocks: vec![
                CanvasBlock {
                    kind: CanvasBlockKind::Status,
                    text: "Gateway healthy".into(),
                    meta: Some(serde_json::json!({ "level": "ok" })),
                },
                CanvasBlock {
                    kind: CanvasBlockKind::Metric,
                    text: "Open sessions".into(),
                    meta: Some(serde_json::json!({ "value": 12 })),
                },
                CanvasBlock {
                    kind: CanvasBlockKind::Action,
                    text: "Open dashboard".into(),
                    meta: Some(serde_json::json!({
                        "action": "open_url",
                        "target": "https://example.com/dashboard"
                    })),
                },
            ],
            revision: 1,
            updated_at: chrono::DateTime::from_timestamp(1_710_000_123, 0).unwrap(),
        };

        let export = export_document(&document, CanvasExportFormat::Markdown);
        assert!(export.contains("**Status (ok)**"));
        assert!(export.contains("**Metric:** Open sessions = 12"));
        assert!(export.contains("**Action (open_url)**"));
    }

    #[tokio::test]
    async fn document_size_limit_rejects_oversized_content() {
        let store = CanvasStore::new();
        let big_body = "x".repeat(MAX_DOCUMENT_SIZE + 1);
        let err = store
            .set(CanvasDocument {
                id: "big".into(),
                title: String::new(),
                body: big_body,
                session_key: None,
                blocks: Vec::new(),
                revision: 0,
                updated_at: chrono::Utc::now(),
            })
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("size exceeds limit"));
    }

    #[tokio::test]
    async fn block_count_limit_rejects_excess_blocks() {
        let store = CanvasStore::new();
        let blocks: Vec<CanvasBlock> = (0..MAX_BLOCKS_PER_DOCUMENT + 1)
            .map(|i| CanvasBlock {
                kind: CanvasBlockKind::Markdown,
                text: format!("block-{i}"),
                meta: None,
            })
            .collect();
        let err = store
            .patch(
                "many-blocks",
                CanvasPatch {
                    append_blocks: blocks,
                    ..Default::default()
                },
            )
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("block count exceeds"));
    }

    #[tokio::test]
    async fn patch_conflict_detection_rejects_stale_revision() {
        let store = CanvasStore::new();
        // Create document at revision 1.
        store
            .set(CanvasDocument {
                id: "conflict".into(),
                title: "v1".into(),
                body: String::new(),
                session_key: None,
                blocks: Vec::new(),
                revision: 0,
                updated_at: chrono::Utc::now(),
            })
            .await
            .expect("set should succeed");

        // Patch expecting revision 1 should succeed (advances to 2).
        store
            .patch(
                "conflict",
                CanvasPatch {
                    title: Some("v2".into()),
                    expected_revision: Some(1),
                    ..Default::default()
                },
            )
            .await
            .expect("patch at correct revision should succeed");

        // Patch expecting revision 1 again should fail (current is 2).
        let err = store
            .patch(
                "conflict",
                CanvasPatch {
                    title: Some("v3-stale".into()),
                    expected_revision: Some(1),
                    ..Default::default()
                },
            )
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("revision conflict"));
    }

    #[test]
    fn strip_html_tags_removes_script_tags() {
        assert_eq!(
            strip_html_tags("hello <script>alert('xss')</script> world"),
            "hello alert('xss') world"
        );
        assert_eq!(
            strip_html_tags("<b>bold</b> and <i>italic</i>"),
            "bold and italic"
        );
        assert_eq!(strip_html_tags("no tags here"), "no tags here");
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn export_markdown_sanitizes_html_in_blocks() {
        let document = CanvasDocument {
            id: "xss".into(),
            title: "Test".into(),
            body: String::new(),
            session_key: None,
            blocks: vec![CanvasBlock {
                kind: CanvasBlockKind::Markdown,
                text: "safe <script>alert('xss')</script> text".into(),
                meta: None,
            }],
            revision: 1,
            updated_at: chrono::DateTime::from_timestamp(1_710_000_000, 0).unwrap(),
        };
        let export = export_document(&document, CanvasExportFormat::Markdown);
        assert!(!export.contains("<script>"));
        assert!(export.contains("safe alert('xss') text"));
    }
}
