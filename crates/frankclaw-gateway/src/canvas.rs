use std::sync::Arc;

use std::collections::HashMap;

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

#[derive(Default)]
pub struct CanvasStore {
    documents: tokio::sync::RwLock<HashMap<String, CanvasDocument>>,
}

impl CanvasStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
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

    pub async fn set(&self, mut document: CanvasDocument) -> CanvasDocument {
        let mut documents = self.documents.write().await;
        let next_revision = documents
            .get(&document.id)
            .map(|existing| existing.revision + 1)
            .unwrap_or(1);
        document.revision = next_revision;
        documents.insert(document.id.clone(), document.clone());
        document
    }

    pub async fn patch(&self, canvas_id: &str, patch: CanvasPatch) -> CanvasDocument {
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
        document.revision += 1;
        document.updated_at = chrono::Utc::now();
        documents.insert(document.id.clone(), document.clone());
        document
    }

    pub async fn clear(&self, canvas_id: &str) {
        self.documents.write().await.remove(canvas_id);
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

fn render_markdown_block(block: &CanvasBlock) -> String {
    let text = block.text.trim();
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
}
