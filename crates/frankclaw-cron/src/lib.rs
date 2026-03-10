#![forbid(unsafe_code)]

mod service;

pub use service::{CronJob, CronService};

use serde::{Deserialize, Serialize};

/// Status of a cron job run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLog {
    pub job_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub status: RunStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Success,
    Failed,
    Skipped,
}
