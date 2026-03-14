//! Subagent system: spawn child agents for complex multi-step tasks.
//!
//! Supports hierarchical agent spawning with depth limits, concurrency control,
//! lifecycle tracking, and push-based result delivery. Subagents inherit
//! parent context (channel, session) and run independently until completion.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use tracing::debug;
use uuid::Uuid;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::types::{AgentId, SessionKey};

/// Maximum spawn depth (parent=0, child=1, grandchild=2, etc.).
const DEFAULT_MAX_DEPTH: u32 = 3;

/// Public access to the default max depth for the runtime.
pub const DEFAULT_MAX_DEPTH_PUB: u32 = DEFAULT_MAX_DEPTH;

/// Maximum concurrent children per parent session.
const DEFAULT_MAX_CHILDREN: usize = 5;

/// Default timeout for a subagent run (seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Unique run identifier for a subagent execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Request to spawn a subagent.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    /// Task description for the subagent.
    pub task: String,
    /// Human-readable label for this run.
    pub label: Option<String>,
    /// Target agent to run as (defaults to parent's agent).
    pub agent_id: AgentId,
    /// Optional model override for the subagent.
    pub model_override: Option<String>,
    /// Maximum execution time in seconds.
    pub timeout_secs: Option<u64>,
    /// Parent's session key (for context inheritance).
    pub parent_session_key: SessionKey,
    /// The spawn depth of the parent (0 for top-level agents).
    pub parent_depth: u32,
}

/// Result of a spawn attempt.
#[derive(Debug, Clone)]
pub enum SpawnResult {
    /// Subagent was accepted and is running.
    Accepted {
        run_id: RunId,
        child_session_key: SessionKey,
    },
    /// Spawn was rejected (depth limit, concurrency limit, etc.).
    Rejected {
        reason: String,
    },
}

/// Lifecycle state of a subagent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Accepted but not yet started processing.
    Pending,
    /// Actively processing the task.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed with an error.
    Failed,
    /// Timed out.
    TimedOut,
    /// Explicitly killed.
    Killed,
}

/// Record of a subagent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: RunId,
    pub child_session_key: SessionKey,
    pub parent_session_key: SessionKey,
    pub agent_id: AgentId,
    pub task: String,
    pub label: Option<String>,
    pub model_override: Option<String>,
    pub depth: u32,
    pub state: RunState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub result_text: Option<String>,
    pub error: Option<String>,
    pub timeout_secs: u64,
}

/// Completion notification sent from subagent back to parent.
#[derive(Debug, Clone)]
pub struct CompletionNotice {
    pub run_id: RunId,
    pub state: RunState,
    pub result_text: Option<String>,
    pub error: Option<String>,
}

/// Registry that tracks active subagent runs and enforces limits.
pub struct SubagentRegistry {
    /// All known runs, keyed by RunId.
    runs: Mutex<HashMap<RunId, RunRecord>>,
    /// Completion channels: parent waits on these for push-based results.
    completions: Mutex<HashMap<RunId, oneshot::Sender<CompletionNotice>>>,
    /// Configuration.
    max_depth: u32,
    max_children: usize,
}

impl Default for SubagentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentRegistry {
    pub fn new() -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
            completions: Mutex::new(HashMap::new()),
            max_depth: DEFAULT_MAX_DEPTH,
            max_children: DEFAULT_MAX_CHILDREN,
        }
    }

    pub fn with_limits(max_depth: u32, max_children: usize) -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
            completions: Mutex::new(HashMap::new()),
            max_depth,
            max_children,
        }
    }

    /// Attempt to register a new subagent run.
    ///
    /// Returns `SpawnResult::Accepted` with run details if the spawn is allowed,
    /// or `SpawnResult::Rejected` if limits are exceeded.
    pub async fn register_spawn(&self, request: &SpawnRequest) -> SpawnResult {
        let child_depth = request.parent_depth + 1;

        // Check depth limit.
        if child_depth > self.max_depth {
            return SpawnResult::Rejected {
                reason: format!(
                    "spawn depth limit exceeded: depth {} > max {}",
                    child_depth, self.max_depth
                ),
            };
        }

        let mut runs = self.runs.lock().await;

        // Check concurrent children limit for this parent.
        let active_children = runs
            .values()
            .filter(|r| {
                r.parent_session_key == request.parent_session_key
                    && matches!(r.state, RunState::Pending | RunState::Running)
            })
            .count();

        if active_children >= self.max_children {
            return SpawnResult::Rejected {
                reason: format!(
                    "concurrent children limit exceeded: {} active >= max {}",
                    active_children, self.max_children
                ),
            };
        }

        let run_id = RunId::new();
        let child_session_key = SessionKey::from_raw(format!(
            "subagent:{}:{}",
            request.agent_id, run_id
        ));

        let record = RunRecord {
            run_id: run_id.clone(),
            child_session_key: child_session_key.clone(),
            parent_session_key: request.parent_session_key.clone(),
            agent_id: request.agent_id.clone(),
            task: request.task.clone(),
            label: request.label.clone(),
            model_override: request.model_override.clone(),
            depth: child_depth,
            state: RunState::Pending,
            created_at: Utc::now(),
            started_at: None,
            ended_at: None,
            result_text: None,
            error: None,
            timeout_secs: request.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
        };

        runs.insert(run_id.clone(), record);
        debug!(%run_id, depth = child_depth, agent = %request.agent_id, "subagent spawn registered");

        SpawnResult::Accepted {
            run_id,
            child_session_key,
        }
    }

    /// Mark a run as started (transitioned from Pending to Running).
    pub async fn mark_running(&self, run_id: &RunId) -> Result<()> {
        let mut runs = self.runs.lock().await;
        let record = runs.get_mut(run_id).ok_or_else(|| FrankClawError::Internal {
            msg: format!("subagent run not found: {run_id}"),
        })?;
        record.state = RunState::Running;
        record.started_at = Some(Utc::now());
        Ok(())
    }

    /// Complete a run and notify the parent.
    pub async fn complete(&self, notice: CompletionNotice) -> Result<()> {
        {
            let mut runs = self.runs.lock().await;
            let record = runs
                .get_mut(&notice.run_id)
                .ok_or_else(|| FrankClawError::Internal {
                    msg: format!("subagent run not found: {}", notice.run_id),
                })?;
            record.state = notice.state.clone();
            record.ended_at = Some(Utc::now());
            record.result_text.clone_from(&notice.result_text);
            record.error.clone_from(&notice.error);
        }

        // Notify the parent via completion channel if one is registered.
        let mut completions = self.completions.lock().await;
        if let Some(tx) = completions.remove(&notice.run_id) {
            let _ = tx.send(notice);
        }

        Ok(())
    }

    /// Register a completion listener for a run.
    /// Returns a receiver that will get the completion notice.
    pub async fn listen_completion(
        &self,
        run_id: &RunId,
    ) -> Result<oneshot::Receiver<CompletionNotice>> {
        let (tx, rx) = oneshot::channel();
        let mut completions = self.completions.lock().await;
        completions.insert(run_id.clone(), tx);
        Ok(rx)
    }

    /// Get the current state of a run.
    pub async fn get_run(&self, run_id: &RunId) -> Option<RunRecord> {
        self.runs.lock().await.get(run_id).cloned()
    }

    /// List all runs for a given parent session.
    pub async fn list_children(&self, parent_key: &SessionKey) -> Vec<RunRecord> {
        self.runs
            .lock()
            .await
            .values()
            .filter(|r| r.parent_session_key == *parent_key)
            .cloned()
            .collect()
    }

    /// List active (pending or running) runs across all parents.
    pub async fn active_runs(&self) -> Vec<RunRecord> {
        self.runs
            .lock()
            .await
            .values()
            .filter(|r| matches!(r.state, RunState::Pending | RunState::Running))
            .cloned()
            .collect()
    }

    /// Kill a run, marking it as killed and notifying the parent.
    pub async fn kill(&self, run_id: &RunId) -> Result<()> {
        self.complete(CompletionNotice {
            run_id: run_id.clone(),
            state: RunState::Killed,
            result_text: None,
            error: Some("killed by parent".into()),
        })
        .await
    }

    /// Clean up completed/failed/killed runs older than the given duration.
    pub async fn cleanup_old_runs(&self, max_age: std::time::Duration) {
        let cutoff = Utc::now() - chrono::Duration::from_std(max_age).unwrap_or_default();
        let mut runs = self.runs.lock().await;
        runs.retain(|_, record| {
            if matches!(
                record.state,
                RunState::Completed | RunState::Failed | RunState::TimedOut | RunState::Killed
            ) {
                record
                    .ended_at
                    .is_none_or(|t| t > cutoff)
            } else {
                true // Keep active runs.
            }
        });
    }

    /// Get the spawn depth of a session by traversing the parent chain.
    pub async fn depth_of(&self, session_key: &SessionKey) -> u32 {
        let runs = self.runs.lock().await;
        // Find the run record for this session key.
        let Some(record) = runs.values().find(|r| r.child_session_key == *session_key) else {
            return 0; // Top-level session.
        };
        record.depth
    }
}

/// Build the system prompt prefix for a subagent, providing context about
/// its role and constraints.
pub fn build_subagent_context(record: &RunRecord, max_depth: u32) -> String {
    use crate::prompts;
    use crate::sanitize;
    const MAX_TASK_LEN: usize = 2000;

    let mut parts = Vec::new();

    let depth_str = record.depth.to_string();
    let max_depth_str = max_depth.to_string();
    parts.push(prompts::render(prompts::SUBAGENT_IDENTITY, &[
        ("depth", &depth_str),
        ("max_depth", &max_depth_str),
    ]));

    // Truncate and sanitize task/label to prevent memory abuse and prompt
    // injection — these could originate from LLM-generated spawn requests.
    if let Some(ref label) = record.label {
        let safe_label = sanitize::sanitize_for_prompt(label);
        let safe_label = if safe_label.len() > MAX_TASK_LEN { &safe_label[..MAX_TASK_LEN] } else { &safe_label };
        parts.push(format!("Task label: {safe_label}"));
    }

    let safe_task = sanitize::sanitize_for_prompt(&record.task);
    let safe_task = if safe_task.len() > MAX_TASK_LEN { &safe_task[..MAX_TASK_LEN] } else { &safe_task };
    parts.push(format!("Task: {safe_task}"));

    let timeout_str = record.timeout_secs.to_string();
    parts.push(prompts::render(prompts::SUBAGENT_TIMEOUT, &[
        ("timeout_secs", &timeout_str),
    ]));

    if record.depth < max_depth {
        parts.push(prompts::SUBAGENT_CAN_SPAWN.trim().to_string());
    } else {
        parts.push(prompts::SUBAGENT_MAX_DEPTH.trim().to_string());
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_request(parent_depth: u32) -> SpawnRequest {
        SpawnRequest {
            task: "test task".into(),
            label: Some("test".into()),
            agent_id: AgentId::new("test-agent"),
            model_override: None,
            timeout_secs: None,
            parent_session_key: SessionKey::from_raw("parent-session"),
            parent_depth,
        }
    }

    #[tokio::test]
    async fn spawn_accepted_within_limits() {
        let registry = SubagentRegistry::new();
        let result = registry.register_spawn(&spawn_request(0)).await;
        match result {
            SpawnResult::Accepted { run_id, child_session_key } => {
                assert!(!run_id.0.is_empty());
                assert!(child_session_key.as_str().starts_with("subagent:"));
            }
            SpawnResult::Rejected { reason } => panic!("expected accepted, got rejected: {reason}"),
        }
    }

    #[tokio::test]
    async fn spawn_rejected_at_max_depth() {
        let registry = SubagentRegistry::with_limits(2, 5);
        // parent_depth=2, child would be 3 > max 2
        let result = registry.register_spawn(&spawn_request(2)).await;
        assert!(matches!(result, SpawnResult::Rejected { .. }));
    }

    #[tokio::test]
    async fn spawn_rejected_at_concurrency_limit() {
        let registry = SubagentRegistry::with_limits(5, 2);
        // Fill up 2 slots.
        let _r1 = registry.register_spawn(&spawn_request(0)).await;
        let _r2 = registry.register_spawn(&spawn_request(0)).await;
        // Third should be rejected.
        let r3 = registry.register_spawn(&spawn_request(0)).await;
        assert!(matches!(r3, SpawnResult::Rejected { .. }));
    }

    #[tokio::test]
    async fn completed_runs_dont_count_against_concurrency() {
        let registry = SubagentRegistry::with_limits(5, 2);
        let r1 = registry.register_spawn(&spawn_request(0)).await;
        let _r2 = registry.register_spawn(&spawn_request(0)).await;

        // Complete the first run.
        if let SpawnResult::Accepted { run_id, .. } = r1 {
            registry
                .complete(CompletionNotice {
                    run_id,
                    state: RunState::Completed,
                    result_text: Some("done".into()),
                    error: None,
                })
                .await
                .unwrap();
        }

        // Now a third spawn should succeed.
        let r3 = registry.register_spawn(&spawn_request(0)).await;
        assert!(matches!(r3, SpawnResult::Accepted { .. }));
    }

    #[tokio::test]
    async fn lifecycle_transitions() {
        let registry = SubagentRegistry::new();
        let SpawnResult::Accepted { run_id, .. } = registry.register_spawn(&spawn_request(0)).await
        else {
            panic!("expected accepted");
        };

        let record = registry.get_run(&run_id).await.unwrap();
        assert_eq!(record.state, RunState::Pending);

        registry.mark_running(&run_id).await.unwrap();
        let record = registry.get_run(&run_id).await.unwrap();
        assert_eq!(record.state, RunState::Running);
        assert!(record.started_at.is_some());

        registry
            .complete(CompletionNotice {
                run_id: run_id.clone(),
                state: RunState::Completed,
                result_text: Some("result".into()),
                error: None,
            })
            .await
            .unwrap();

        let record = registry.get_run(&run_id).await.unwrap();
        assert_eq!(record.state, RunState::Completed);
        assert_eq!(record.result_text.as_deref(), Some("result"));
        assert!(record.ended_at.is_some());
    }

    #[tokio::test]
    async fn completion_notification_delivered() {
        let registry = SubagentRegistry::new();
        let SpawnResult::Accepted { run_id, .. } = registry.register_spawn(&spawn_request(0)).await
        else {
            panic!("expected accepted");
        };

        let rx = registry.listen_completion(&run_id).await.unwrap();

        registry
            .complete(CompletionNotice {
                run_id: run_id.clone(),
                state: RunState::Completed,
                result_text: Some("hello from child".into()),
                error: None,
            })
            .await
            .unwrap();

        let notice = rx.await.unwrap();
        assert_eq!(notice.result_text.as_deref(), Some("hello from child"));
    }

    #[tokio::test]
    async fn kill_sets_state_and_notifies() {
        let registry = SubagentRegistry::new();
        let SpawnResult::Accepted { run_id, .. } = registry.register_spawn(&spawn_request(0)).await
        else {
            panic!("expected accepted");
        };

        let rx = registry.listen_completion(&run_id).await.unwrap();
        registry.kill(&run_id).await.unwrap();

        let notice = rx.await.unwrap();
        assert_eq!(notice.state, RunState::Killed);

        let record = registry.get_run(&run_id).await.unwrap();
        assert_eq!(record.state, RunState::Killed);
    }

    #[tokio::test]
    async fn list_children_filters_by_parent() {
        let registry = SubagentRegistry::new();
        let _r1 = registry.register_spawn(&spawn_request(0)).await;
        let _r2 = registry.register_spawn(&spawn_request(0)).await;

        // Different parent.
        let mut req3 = spawn_request(0);
        req3.parent_session_key = SessionKey::from_raw("other-parent");
        let _r3 = registry.register_spawn(&req3).await;

        let children = registry
            .list_children(&SessionKey::from_raw("parent-session"))
            .await;
        assert_eq!(children.len(), 2);

        let other_children = registry
            .list_children(&SessionKey::from_raw("other-parent"))
            .await;
        assert_eq!(other_children.len(), 1);
    }

    #[tokio::test]
    async fn depth_of_returns_correct_depth() {
        let registry = SubagentRegistry::new();
        let SpawnResult::Accepted {
            child_session_key, ..
        } = registry.register_spawn(&spawn_request(0)).await
        else {
            panic!("expected accepted");
        };

        assert_eq!(registry.depth_of(&child_session_key).await, 1);
        assert_eq!(
            registry
                .depth_of(&SessionKey::from_raw("unknown"))
                .await,
            0
        );
    }

    #[tokio::test]
    async fn cleanup_removes_old_completed_runs() {
        let registry = SubagentRegistry::new();
        let SpawnResult::Accepted { run_id, .. } = registry.register_spawn(&spawn_request(0)).await
        else {
            panic!("expected accepted");
        };

        registry
            .complete(CompletionNotice {
                run_id: run_id.clone(),
                state: RunState::Completed,
                result_text: None,
                error: None,
            })
            .await
            .unwrap();

        // Spawn an active run that should survive cleanup.
        let SpawnResult::Accepted {
            run_id: active_id, ..
        } = registry.register_spawn(&spawn_request(0)).await
        else {
            panic!("expected accepted");
        };

        // With a large max_age, the just-completed run survives (it's recent).
        registry
            .cleanup_old_runs(std::time::Duration::from_secs(3600))
            .await;
        assert!(registry.get_run(&run_id).await.is_some());

        // With zero max_age, completed runs are cleaned (cutoff = now).
        registry
            .cleanup_old_runs(std::time::Duration::from_secs(0))
            .await;
        assert!(registry.get_run(&run_id).await.is_none());

        // Active (pending) run is always kept.
        assert!(registry.get_run(&active_id).await.is_some());
    }

    #[test]
    fn build_subagent_context_includes_task() {
        let record = RunRecord {
            run_id: RunId::new(),
            child_session_key: SessionKey::from_raw("child"),
            parent_session_key: SessionKey::from_raw("parent"),
            agent_id: AgentId::new("test"),
            task: "Find all TODO comments".into(),
            label: Some("code-scan".into()),
            model_override: None,
            depth: 1,
            state: RunState::Running,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            ended_at: None,
            result_text: None,
            error: None,
            timeout_secs: 120,
        };

        let ctx = build_subagent_context(&record, 3);
        assert!(ctx.contains("depth 1/3"));
        assert!(ctx.contains("Find all TODO comments"));
        assert!(ctx.contains("code-scan"));
        assert!(ctx.contains("120 seconds"));
        assert!(ctx.contains("may spawn sub-subagents"));
    }

    #[test]
    fn build_subagent_context_at_max_depth_blocks_spawning() {
        let record = RunRecord {
            run_id: RunId::new(),
            child_session_key: SessionKey::from_raw("child"),
            parent_session_key: SessionKey::from_raw("parent"),
            agent_id: AgentId::new("test"),
            task: "Leaf task".into(),
            label: None,
            model_override: None,
            depth: 3,
            state: RunState::Running,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            ended_at: None,
            result_text: None,
            error: None,
            timeout_secs: 60,
        };

        let ctx = build_subagent_context(&record, 3);
        assert!(ctx.contains("cannot spawn further subagents"));
    }
}
