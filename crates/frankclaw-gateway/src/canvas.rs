use std::sync::Arc;

use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanvasBlockKind {
    Markdown,
    Code,
    Note,
    Checklist,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CanvasBlock {
    pub kind: CanvasBlockKind,
    pub text: String,
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
