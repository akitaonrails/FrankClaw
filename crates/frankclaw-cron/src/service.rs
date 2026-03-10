use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use chrono::{Duration, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::{AgentId, SessionKey};

use crate::{RunLog, RunStatus};

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
    path: Option<PathBuf>,
    cancel: CancellationToken,
}

impl CronService {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            path: None,
            cancel: CancellationToken::new(),
        }
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FrankClawError::ConfigIo {
                msg: format!("failed to create cron directory: {e}"),
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }

        let jobs = load_jobs(path)?;
        Ok(Self {
            jobs: Arc::new(RwLock::new(jobs)),
            path: Some(path.to_path_buf()),
            cancel: CancellationToken::new(),
        })
    }

    /// Start the cron tick loop.
    pub async fn start(
        &self,
        job_runner: Arc<
            dyn Fn(CronJob) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
                + Send
                + Sync,
        >,
    ) {
        let jobs = self.jobs.clone();
        let path = self.path.clone();
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = Utc::now();
                        let window_start = now - Duration::seconds(60);
                        let mut due_jobs = Vec::new();

                        {
                            let mut jobs_guard = jobs.write().await;
                            for job in jobs_guard.values_mut() {
                                if !job.enabled {
                                    continue;
                                }

                                let Ok(schedule) = job.schedule.parse::<Schedule>() else {
                                    warn!(job_id = %job.id, "invalid cron schedule");
                                    continue;
                                };

                                let Some(next_run) = schedule.after(&window_start).next() else {
                                    continue;
                                };
                                if next_run > now {
                                    continue;
                                }

                                let already_started = job
                                    .last_run
                                    .as_ref()
                                    .map(|run| run.started_at >= next_run)
                                    .unwrap_or(false);
                                if already_started {
                                    continue;
                                }

                                debug!(job_id = %job.id, scheduled_for = %next_run, "executing cron job");
                                job.last_run = Some(RunLog {
                                    job_id: job.id.clone(),
                                    started_at: now,
                                    finished_at: None,
                                    status: RunStatus::Running,
                                    error: None,
                                });
                                due_jobs.push(job.clone());
                            }
                        }
                        save_jobs(path.as_deref(), &jobs).await;

                        for job in due_jobs {
                            let runner = job_runner.clone();
                            let jobs = jobs.clone();
                            let path = path.clone();
                            tokio::spawn(async move {
                                let started_at = Utc::now();
                                let result = runner(job.clone()).await;
                                let finished_at = Utc::now();

                                {
                                    let mut jobs_guard = jobs.write().await;
                                    if let Some(stored) = jobs_guard.get_mut(&job.id) {
                                        stored.last_run = Some(RunLog {
                                            job_id: job.id.clone(),
                                            started_at,
                                            finished_at: Some(finished_at),
                                            status: if result.is_ok() {
                                                RunStatus::Success
                                            } else {
                                                RunStatus::Failed
                                            },
                                            error: result.err().map(|err| err.to_string()),
                                        });
                                    }
                                }
                                save_jobs(path.as_deref(), &jobs).await;
                            });
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
        save_jobs(self.path.as_deref(), &self.jobs).await;
        Ok(())
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        self.jobs.write().await.remove(id);
        save_jobs(self.path.as_deref(), &self.jobs).await;
        Ok(())
    }

    pub async fn list(&self) -> Vec<CronJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    pub async fn sync_jobs<I>(&self, jobs: I) -> Result<()>
    where
        I: IntoIterator<Item = CronJob>,
    {
        let mut next = HashMap::new();
        {
            let existing = self.jobs.read().await;
            for mut job in jobs {
                if let Some(previous) = existing.get(&job.id).and_then(|stored| stored.last_run.clone()) {
                    job.last_run = Some(previous);
                }
                job.schedule.parse::<Schedule>().map_err(|e| FrankClawError::ConfigValidation {
                    msg: format!("invalid cron schedule '{}': {e}", job.schedule),
                })?;
                next.insert(job.id.clone(), job);
            }
        }

        *self.jobs.write().await = next;
        save_jobs(self.path.as_deref(), &self.jobs).await;
        Ok(())
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

fn load_jobs(path: &Path) -> Result<HashMap<String, CronJob>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(path).map_err(|e| FrankClawError::ConfigIo {
        msg: format!("failed to read cron file: {e}"),
    })?;
    serde_json::from_str(&content).map_err(|e| FrankClawError::ConfigIo {
        msg: format!("failed to parse cron file: {e}"),
    })
}

async fn save_jobs(path: Option<&Path>, jobs: &Arc<RwLock<HashMap<String, CronJob>>>) {
    let Some(path) = path else {
        return;
    };

    let snapshot = jobs.read().await.clone();
    if let Ok(content) = serde_json::to_string_pretty(&snapshot) {
        let _ = std::fs::write(path, content);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sync_jobs_preserves_last_run_for_matching_ids() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-cron-{}.json",
            uuid::Uuid::new_v4()
        ));
        let service = CronService::open(&temp).expect("cron store should open");
        let job = CronJob {
            id: "job-1".into(),
            schedule: "0 * * * * *".into(),
            agent_id: AgentId::default_agent(),
            session_key: SessionKey::from_raw("default:cron:job:1"),
            prompt: "hello".into(),
            enabled: true,
            created_at: Utc::now(),
            last_run: Some(RunLog {
                job_id: "job-1".into(),
                started_at: Utc::now(),
                finished_at: None,
                status: RunStatus::Running,
                error: None,
            }),
        };
        service.add(job.clone()).await.expect("add should work");

        let replacement = CronJob {
            prompt: "updated".into(),
            last_run: None,
            ..job
        };
        service
            .sync_jobs(vec![replacement])
            .await
            .expect("sync should work");

        let synced = service.list().await;
        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].prompt, "updated");
        assert!(synced[0].last_run.is_some());

        let _ = std::fs::remove_file(temp);
    }
}
