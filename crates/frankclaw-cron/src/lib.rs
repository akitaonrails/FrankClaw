#![forbid(unsafe_code)]

pub mod job;
mod service;
pub mod triggers;

pub use job::{
    repair_stuck_job, JobContext, JobState, RepairResult, StateTransition, StuckJob,
    DEFAULT_MAX_REPAIR_ATTEMPTS,
};
pub use service::{CronJob, CronService, JobRunner};
pub use triggers::{
    matches_event_trigger, matches_system_event, FireCheck, RoutineAction, SystemEvent,
    TriggerGuardrails, TriggerState, TriggerType, DEFAULT_COOLDOWN_SECS, DEFAULT_MAX_CONCURRENT,
};

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
