//! Service trait abstractions for tools.
//!
//! These traits decouple `frankclaw-tools` from concrete implementations in
//! `frankclaw-media`, `frankclaw-channels`, and `frankclaw-cron`, avoiding
//! circular dependencies.

use async_trait::async_trait;

use crate::error::Result;

/// Content returned by a URL fetch.
#[derive(Debug, Clone)]
pub struct FetchedContent {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub final_url: String,
}

/// SSRF-safe URL fetcher.
#[async_trait]
pub trait Fetcher: Send + Sync + 'static {
    async fn fetch(&self, url: &str) -> Result<FetchedContent>;
}

/// Send outbound messages through a channel adapter.
#[async_trait]
pub trait MessageSender: Send + Sync + 'static {
    async fn send_text(
        &self,
        channel: &str,
        account_id: &str,
        to: &str,
        text: &str,
        thread_id: Option<&str>,
        reply_to: Option<&str>,
    ) -> Result<String>;
}

/// Manage scheduled cron jobs.
#[async_trait]
pub trait CronManager: Send + Sync + 'static {
    async fn list_jobs(&self) -> Vec<serde_json::Value>;
    async fn add_job(
        &self,
        id: &str,
        schedule: &str,
        agent_id: &str,
        session_key: &str,
        prompt: &str,
        enabled: bool,
    ) -> Result<()>;
    async fn remove_job(&self, id: &str) -> Result<bool>;
}
