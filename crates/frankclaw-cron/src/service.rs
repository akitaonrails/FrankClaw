use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::{AgentId, SessionKey};

use crate::RunLog;

/// A scheduled job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub schedule: String,
    pub agent_id: AgentId,
    pub session_key: SessionKey,
    pub prompt: String,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_run: Option<RunLog>,
}

/// Cron service that schedules and executes jobs.
pub struct CronService {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    cancel: CancellationToken,
}

impl CronService {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            cancel: CancellationToken::new(),
        }
    }

    /// Start the cron tick loop.
    pub async fn start(
        &self,
        job_runner: Arc<dyn Fn(CronJob) -> tokio::task::JoinHandle<()> + Send + Sync>,
    ) {
        let jobs = self.jobs.clone();
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let jobs_snapshot = jobs.read().await;
                        let now = Utc::now();

                        for job in jobs_snapshot.values() {
                            if !job.enabled {
                                continue;
                            }

                            let Ok(schedule) = job.schedule.parse::<Schedule>() else {
                                warn!(job_id = %job.id, "invalid cron schedule");
                                continue;
                            };

                            // Check if this job should run now.
                            if let Some(next) = schedule.upcoming(chrono::Utc).next() {
                                let diff = (next - now).num_seconds().abs();
                                if diff <= 60 {
                                    debug!(job_id = %job.id, "executing cron job");
                                    job_runner(job.clone());
                                }
                            }
                        }
                    }
                    _ = cancel.cancelled() => {
                        info!("cron service stopped");
                        break;
                    }
                }
            }
        });
    }

    pub async fn add(&self, job: CronJob) -> Result<()> {
        // Validate cron expression.
        job.schedule.parse::<Schedule>().map_err(|e| {
            FrankClawError::ConfigValidation {
                msg: format!("invalid cron schedule '{}': {e}", job.schedule),
            }
        })?;

        self.jobs.write().await.insert(job.id.clone(), job);
        Ok(())
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        self.jobs.write().await.remove(id);
        Ok(())
    }

    pub async fn list(&self) -> Vec<CronJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}
