use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanvasDocument {
    pub title: String,
    pub body: String,
    pub session_key: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Default)]
pub struct CanvasStore {
    current: tokio::sync::RwLock<Option<CanvasDocument>>,
}

impl CanvasStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn get(&self) -> Option<CanvasDocument> {
        self.current.read().await.clone()
    }

    pub async fn set(&self, document: CanvasDocument) {
        *self.current.write().await = Some(document);
    }

    pub async fn clear(&self) {
        *self.current.write().await = None;
    }
}
