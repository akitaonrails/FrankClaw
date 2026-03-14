//! Job state machine for background task lifecycle management.
//!
//! Tracks job state transitions (Pending → InProgress → Completed/Failed/Stuck),
//! enforces valid transitions, and supports self-repair for stuck jobs.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of state transitions to retain per job.
const MAX_TRANSITIONS: usize = 200;

/// Default maximum repair attempts before manual intervention is required.
pub const DEFAULT_MAX_REPAIR_ATTEMPTS: u32 = 3;

/// State of a background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Job is queued but not yet started.
    Pending,
    /// Job is actively executing.
    InProgress,
    /// Job completed successfully.
    Completed,
    /// Job output was submitted for review/delivery.
    Submitted,
    /// Job output was accepted (final success state).
    Accepted,
    /// Job failed permanently.
    Failed,
    /// Job is stuck (no progress for too long).
    Stuck,
    /// Job was cancelled by user or system.
    Cancelled,
}

impl JobState {
    /// Check whether a transition from this state to `target` is valid.
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            // From Pending
            (Self::Pending | Self::Stuck, Self::InProgress) |
(Self::Pending | Self::InProgress | Self::Stuck, Self::Cancelled) |
(Self::InProgress, Self::Completed | Self::Failed | Self::Stuck) |
(Self::Completed, Self::Submitted | Self::Failed) |
(Self::Submitted, Self::Accepted | Self::Failed) | (Self::Stuck, Self::Failed)
        )
    }

    /// Whether this state is terminal (no further transitions allowed except cancel).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Failed | Self::Cancelled
        )
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
            Self::Submitted => write!(f, "submitted"),
            Self::Accepted => write!(f, "accepted"),
            Self::Failed => write!(f, "failed"),
            Self::Stuck => write!(f, "stuck"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// A recorded state transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransition {
    pub from: JobState,
    pub to: JobState,
    pub timestamp: DateTime<Utc>,
    pub reason: Option<String>,
}

/// Context for a background job, tracking its full lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobContext {
    pub job_id: String,
    pub state: JobState,
    pub title: String,
    pub description: String,

    /// Number of tokens consumed so far.
    pub total_tokens_used: u64,
    /// Maximum token budget for this job (0 = unlimited).
    pub max_tokens: u64,
    /// Number of self-repair attempts made.
    pub repair_attempts: u32,
    /// Maximum allowed repair attempts.
    pub max_repair_attempts: u32,

    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,

    /// Ordered history of state transitions (capped at MAX_TRANSITIONS).
    pub transitions: Vec<StateTransition>,
}

impl JobContext {
    /// Create a new job context in the Pending state.
    pub fn new(job_id: impl Into<String>, title: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            state: JobState::Pending,
            title: title.into(),
            description: description.into(),
            total_tokens_used: 0,
            max_tokens: 0,
            repair_attempts: 0,
            max_repair_attempts: DEFAULT_MAX_REPAIR_ATTEMPTS,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            transitions: Vec::new(),
        }
    }

    /// Transition to a new state, recording the transition with an optional reason.
    ///
    /// Returns `Err` if the transition is invalid.
    pub fn transition_to(
        &mut self,
        new_state: JobState,
        reason: Option<String>,
    ) -> std::result::Result<(), String> {
        if !self.state.can_transition_to(new_state) {
            return Err(format!(
                "cannot transition from {} to {}",
                self.state, new_state
            ));
        }

        self.transitions.push(StateTransition {
            from: self.state,
            to: new_state,
            timestamp: Utc::now(),
            reason,
        });

        // Cap transition history.
        if self.transitions.len() > MAX_TRANSITIONS {
            let drain_count = self.transitions.len() - MAX_TRANSITIONS;
            self.transitions.drain(..drain_count);
        }

        self.state = new_state;

        // Auto-update timestamps.
        match new_state {
            JobState::InProgress if self.started_at.is_none() => {
                self.started_at = Some(Utc::now());
            }
            JobState::Completed
            | JobState::Accepted
            | JobState::Failed
            | JobState::Cancelled => {
                self.completed_at = Some(Utc::now());
            }
            _ => {}
        }

        Ok(())
    }

    /// Mark the job as stuck with a reason.
    pub fn mark_stuck(&mut self, reason: impl Into<String>) -> std::result::Result<(), String> {
        self.transition_to(JobState::Stuck, Some(reason.into()))
    }

    /// Attempt recovery from stuck state.
    ///
    /// Returns `Err` if the job is not stuck or has exceeded max repair attempts.
    pub fn attempt_recovery(&mut self) -> std::result::Result<(), String> {
        if self.state != JobState::Stuck {
            return Err(format!("job is not stuck (state: {})", self.state));
        }

        if self.repair_attempts >= self.max_repair_attempts {
            return Err(format!(
                "exceeded max repair attempts ({}/{})",
                self.repair_attempts, self.max_repair_attempts
            ));
        }

        self.repair_attempts += 1;
        self.transition_to(
            JobState::InProgress,
            Some(format!("recovery attempt {}", self.repair_attempts)),
        )
    }

    /// Number of transitions recorded.
    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }

    /// Record token usage.
    pub fn add_tokens(&mut self, tokens: u64) {
        self.total_tokens_used = self.total_tokens_used.saturating_add(tokens);
    }

    /// Check if the token budget has been exceeded.
    pub fn is_over_budget(&self) -> bool {
        self.max_tokens > 0 && self.total_tokens_used > self.max_tokens
    }
}

/// Result of a self-repair attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairResult {
    /// Recovery succeeded, job is back in progress.
    Success { message: String },
    /// Transient failure, can retry later.
    Retry { message: String },
    /// Permanent failure, job cannot be recovered.
    Failed { message: String },
    /// Exceeded max repair attempts, manual intervention required.
    ManualRequired { message: String },
}

/// Information about a detected stuck job.
#[derive(Debug, Clone)]
pub struct StuckJob {
    pub job_id: String,
    pub last_activity: DateTime<Utc>,
    pub stuck_duration: std::time::Duration,
    pub repair_attempts: u32,
    pub max_repair_attempts: u32,
}

/// Attempt to repair a stuck job context.
///
/// Returns the repair result and mutates the context if recovery succeeds.
pub fn repair_stuck_job(ctx: &mut JobContext) -> RepairResult {
    if ctx.state != JobState::Stuck {
        return RepairResult::Failed {
            message: format!("job {} is not stuck (state: {})", ctx.job_id, ctx.state),
        };
    }

    if ctx.repair_attempts >= ctx.max_repair_attempts {
        return RepairResult::ManualRequired {
            message: format!(
                "job {} exceeded {} repair attempts",
                ctx.job_id, ctx.max_repair_attempts
            ),
        };
    }

    match ctx.attempt_recovery() {
        Ok(()) => RepairResult::Success {
            message: format!(
                "job {} recovered to in_progress (attempt {})",
                ctx.job_id, ctx.repair_attempts
            ),
        },
        Err(e) => RepairResult::Failed {
            message: format!("failed to recover job {}: {}", ctx.job_id, e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_job_starts_pending() {
        let ctx = JobContext::new("job-1", "Test", "A test job");
        assert_eq!(ctx.state, JobState::Pending);
        assert!(ctx.started_at.is_none());
        assert!(ctx.completed_at.is_none());
        assert_eq!(ctx.repair_attempts, 0);
        assert!(ctx.transitions.is_empty());
    }

    #[test]
    fn happy_path_transitions() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.transition_to(JobState::InProgress, None).unwrap();
        assert_eq!(ctx.state, JobState::InProgress);
        assert!(ctx.started_at.is_some());

        ctx.transition_to(JobState::Completed, Some("done".into()))
            .unwrap();
        assert_eq!(ctx.state, JobState::Completed);
        assert!(ctx.completed_at.is_some());

        ctx.transition_to(JobState::Submitted, None).unwrap();
        ctx.transition_to(JobState::Accepted, None).unwrap();
        assert_eq!(ctx.state, JobState::Accepted);
        assert!(ctx.state.is_terminal());
        assert_eq!(ctx.transition_count(), 4);
    }

    #[test]
    fn invalid_transition_rejected() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        let err = ctx.transition_to(JobState::Completed, None);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("cannot transition"));
    }

    #[test]
    fn stuck_and_recovery() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.transition_to(JobState::InProgress, None).unwrap();
        ctx.mark_stuck("no progress for 5 minutes").unwrap();
        assert_eq!(ctx.state, JobState::Stuck);

        ctx.attempt_recovery().unwrap();
        assert_eq!(ctx.state, JobState::InProgress);
        assert_eq!(ctx.repair_attempts, 1);
    }

    #[test]
    fn recovery_fails_when_not_stuck() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.transition_to(JobState::InProgress, None).unwrap();
        let err = ctx.attempt_recovery();
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("not stuck"));
    }

    #[test]
    fn recovery_fails_after_max_attempts() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.max_repair_attempts = 2;

        ctx.transition_to(JobState::InProgress, None).unwrap();

        // First recovery
        ctx.mark_stuck("stuck-1").unwrap();
        ctx.attempt_recovery().unwrap();

        // Second recovery
        ctx.mark_stuck("stuck-2").unwrap();
        ctx.attempt_recovery().unwrap();

        // Third attempt should fail
        ctx.mark_stuck("stuck-3").unwrap();
        let err = ctx.attempt_recovery();
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("exceeded max repair attempts"));
    }

    #[test]
    fn terminal_states_block_transitions() {
        for terminal in [JobState::Accepted, JobState::Failed, JobState::Cancelled] {
            assert!(terminal.is_terminal());
            // No state can be reached from terminal states
            for target in [
                JobState::Pending,
                JobState::InProgress,
                JobState::Completed,
                JobState::Submitted,
                JobState::Accepted,
                JobState::Failed,
                JobState::Stuck,
                JobState::Cancelled,
            ] {
                assert!(
                    !terminal.can_transition_to(target),
                    "{terminal} should not transition to {target}"
                );
            }
        }
    }

    #[test]
    fn non_terminal_states_are_not_terminal() {
        for state in [
            JobState::Pending,
            JobState::InProgress,
            JobState::Completed,
            JobState::Submitted,
            JobState::Stuck,
        ] {
            assert!(!state.is_terminal(), "{state} should not be terminal");
        }
    }

    #[test]
    fn transition_history_capped() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        // Generate many transitions by toggling InProgress ↔ Stuck
        ctx.transition_to(JobState::InProgress, None).unwrap();
        ctx.max_repair_attempts = 500;

        for i in 0..250 {
            ctx.mark_stuck(format!("stuck-{i}")).unwrap();
            ctx.attempt_recovery().unwrap();
        }

        assert!(ctx.transitions.len() <= MAX_TRANSITIONS);
    }

    #[test]
    fn token_budget_tracking() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.max_tokens = 1000;
        assert!(!ctx.is_over_budget());

        ctx.add_tokens(500);
        assert!(!ctx.is_over_budget());

        ctx.add_tokens(600);
        assert!(ctx.is_over_budget());
        assert_eq!(ctx.total_tokens_used, 1100);
    }

    #[test]
    fn unlimited_budget_never_exceeded() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.max_tokens = 0; // unlimited
        ctx.add_tokens(u64::MAX);
        assert!(!ctx.is_over_budget());
    }

    #[test]
    fn repair_stuck_job_success() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.transition_to(JobState::InProgress, None).unwrap();
        ctx.mark_stuck("timeout").unwrap();

        let result = repair_stuck_job(&mut ctx);
        assert!(matches!(result, RepairResult::Success { .. }));
        assert_eq!(ctx.state, JobState::InProgress);
    }

    #[test]
    fn repair_stuck_job_manual_required() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.max_repair_attempts = 1;
        ctx.transition_to(JobState::InProgress, None).unwrap();
        ctx.mark_stuck("stuck-1").unwrap();
        ctx.attempt_recovery().unwrap();
        ctx.mark_stuck("stuck-2").unwrap();

        let result = repair_stuck_job(&mut ctx);
        assert!(matches!(result, RepairResult::ManualRequired { .. }));
    }

    #[test]
    fn repair_not_stuck_job_fails() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.transition_to(JobState::InProgress, None).unwrap();

        let result = repair_stuck_job(&mut ctx);
        assert!(matches!(result, RepairResult::Failed { .. }));
    }

    #[test]
    fn cancellation_from_various_states() {
        for initial in [
            JobState::Pending,
            JobState::InProgress,
            JobState::Stuck,
        ] {
            let mut ctx = JobContext::new("job-1", "Test", "");
            if initial != JobState::Pending {
                ctx.transition_to(JobState::InProgress, None).unwrap();
            }
            if initial == JobState::Stuck {
                ctx.mark_stuck("test").unwrap();
            }
            ctx.transition_to(JobState::Cancelled, Some("user cancelled".into()))
                .unwrap();
            assert_eq!(ctx.state, JobState::Cancelled);
            assert!(ctx.completed_at.is_some());
        }
    }

    #[test]
    fn completed_can_fail() {
        let mut ctx = JobContext::new("job-1", "Test", "");
        ctx.transition_to(JobState::InProgress, None).unwrap();
        ctx.transition_to(JobState::Completed, None).unwrap();
        ctx.transition_to(JobState::Failed, Some("post-completion failure".into()))
            .unwrap();
        assert_eq!(ctx.state, JobState::Failed);
    }

    #[test]
    fn display_impl() {
        assert_eq!(JobState::Pending.to_string(), "pending");
        assert_eq!(JobState::InProgress.to_string(), "in_progress");
        assert_eq!(JobState::Stuck.to_string(), "stuck");
        assert_eq!(JobState::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn serialization_roundtrip() {
        let mut ctx = JobContext::new("job-1", "Test Job", "A test");
        ctx.transition_to(JobState::InProgress, None).unwrap();
        ctx.add_tokens(100);

        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: JobContext = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.job_id, "job-1");
        assert_eq!(deserialized.state, JobState::InProgress);
        assert_eq!(deserialized.total_tokens_used, 100);
        assert_eq!(deserialized.transitions.len(), 1);
    }
}
