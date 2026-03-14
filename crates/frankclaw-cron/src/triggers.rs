//! Event trigger system for routines beyond basic cron schedules.
//!
//! Supports four trigger types: cron schedules, message pattern matching,
//! structured system events, and manual invocation. Includes guardrails
//! to prevent runaway execution (cooldown, max concurrent, dedup).
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default cooldown between trigger fires (5 minutes).
pub const DEFAULT_COOLDOWN_SECS: u64 = 300;

/// Default maximum concurrent executions of a single routine.
pub const DEFAULT_MAX_CONCURRENT: u32 = 1;

/// How a routine is triggered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerType {
    /// Fire on a cron schedule.
    Cron {
        schedule: String,
        #[serde(default)]
        timezone: Option<String>,
    },

    /// Fire when a channel message matches a regex pattern.
    Event {
        /// Optional channel filter (e.g. "telegram", "discord").
        #[serde(default)]
        channel: Option<String>,
        /// Regex pattern to match against message content.
        pattern: String,
    },

    /// Fire when a structured system event is emitted.
    SystemEvent {
        /// Event namespace (e.g. "github", "workflow").
        source: String,
        /// Event type within the namespace (e.g. "issue.opened").
        event_type: String,
        /// Optional exact-match filters on top-level payload fields.
        #[serde(default)]
        filters: HashMap<String, String>,
    },

    /// Only fires via explicit tool call or CLI command.
    Manual,
}

/// Guardrails to prevent runaway routine execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerGuardrails {
    /// Minimum time between fires (seconds).
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,

    /// Maximum simultaneous executions of this routine.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,

    /// Optional dedup window — identical event content within this window is ignored.
    #[serde(default)]
    pub dedup_window_secs: Option<u64>,
}

fn default_cooldown_secs() -> u64 {
    DEFAULT_COOLDOWN_SECS
}

fn default_max_concurrent() -> u32 {
    DEFAULT_MAX_CONCURRENT
}

impl Default for TriggerGuardrails {
    fn default() -> Self {
        Self {
            cooldown_secs: DEFAULT_COOLDOWN_SECS,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            dedup_window_secs: None,
        }
    }
}

impl TriggerGuardrails {
    /// Cooldown as a `Duration`.
    pub fn cooldown(&self) -> Duration {
        Duration::from_secs(self.cooldown_secs)
    }

    /// Dedup window as a `Duration`, if set.
    pub fn dedup_window(&self) -> Option<Duration> {
        self.dedup_window_secs.map(Duration::from_secs)
    }
}

/// What happens when a routine fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutineAction {
    /// Single LLM call, no tool access. Fast and cheap.
    Lightweight {
        prompt: String,
        #[serde(default)]
        context_paths: Vec<String>,
        #[serde(default = "default_max_tokens")]
        max_tokens: u32,
    },

    /// Full multi-turn worker job with tool access.
    FullJob {
        title: String,
        description: String,
        #[serde(default = "default_max_iterations")]
        max_iterations: u32,
        #[serde(default)]
        tool_permissions: Vec<String>,
    },
}

fn default_max_tokens() -> u32 {
    2048
}

fn default_max_iterations() -> u32 {
    10
}

/// A structured system event that can trigger routines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemEvent {
    /// Event namespace (e.g. "github", "workflow").
    pub source: String,
    /// Event type within namespace (e.g. "issue.opened").
    pub event_type: String,
    /// Arbitrary JSON payload.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// When the event was emitted.
    pub timestamp: DateTime<Utc>,
}

impl SystemEvent {
    /// Create a new system event with the current timestamp.
    pub fn new(source: impl Into<String>, event_type: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            event_type: event_type.into(),
            payload: serde_json::Value::Null,
            timestamp: Utc::now(),
        }
    }

    /// Set the payload.
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }
}

/// Runtime state for tracking trigger execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TriggerState {
    /// When this routine last fired.
    pub last_fired_at: Option<DateTime<Utc>>,
    /// Number of currently active executions.
    pub active_count: u32,
    /// Total number of times this routine has fired.
    pub fire_count: u64,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
    /// Hash of last event content (for dedup).
    pub last_event_hash: Option<u64>,
}

impl TriggerState {
    /// Check if the routine can fire given its guardrails and current state.
    pub fn can_fire(&self, guardrails: &TriggerGuardrails) -> FireCheck {
        // Check concurrent limit.
        if self.active_count >= guardrails.max_concurrent {
            return FireCheck::Blocked {
                reason: format!(
                    "max concurrent executions reached ({}/{})",
                    self.active_count, guardrails.max_concurrent
                ),
            };
        }

        // Check cooldown.
        if let Some(last_fired) = self.last_fired_at {
            let elapsed = Utc::now().signed_duration_since(last_fired);
            let cooldown = chrono::Duration::seconds(guardrails.cooldown_secs as i64);
            if elapsed < cooldown {
                let remaining = (cooldown - elapsed).num_seconds();
                return FireCheck::Blocked {
                    reason: format!("cooldown active ({remaining}s remaining)"),
                };
            }
        }

        FireCheck::Allowed
    }

    /// Record that the routine has fired.
    pub fn record_fire(&mut self) {
        self.last_fired_at = Some(Utc::now());
        self.active_count += 1;
        self.fire_count += 1;
    }

    /// Record that an execution has completed.
    pub fn record_completion(&mut self, success: bool) {
        self.active_count = self.active_count.saturating_sub(1);
        if success {
            self.consecutive_failures = 0;
        } else {
            self.consecutive_failures += 1;
        }
    }
}

/// Result of checking whether a trigger can fire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireCheck {
    /// Trigger is allowed to fire.
    Allowed,
    /// Trigger is blocked with a reason.
    Blocked { reason: String },
}

/// Check if a message matches an event trigger.
///
/// Returns `true` if the pattern matches the message content and
/// the channel filter (if any) matches the message channel.
pub fn matches_event_trigger(
    trigger: &TriggerType,
    message_content: &str,
    message_channel: Option<&str>,
) -> bool {
    match trigger {
        TriggerType::Event { channel, pattern } => {
            // Check channel filter.
            if let Some(required_channel) = channel {
                if let Some(actual_channel) = message_channel {
                    if !required_channel.eq_ignore_ascii_case(actual_channel) {
                        return false;
                    }
                } else {
                    return false;
                }
            }

            // Check pattern match (case-insensitive).
            regex::Regex::new(pattern)
                .is_ok_and(|re| re.is_match(message_content))
        }
        _ => false,
    }
}

/// Check if a system event matches a system event trigger.
pub fn matches_system_event(trigger: &TriggerType, event: &SystemEvent) -> bool {
    match trigger {
        TriggerType::SystemEvent {
            source,
            event_type,
            filters,
        } => {
            // Case-insensitive source and event type matching.
            if !source.eq_ignore_ascii_case(&event.source) {
                return false;
            }
            if !event_type.eq_ignore_ascii_case(&event.event_type) {
                return false;
            }

            // Check filters against top-level payload fields.
            if !filters.is_empty() {
                if let serde_json::Value::Object(map) = &event.payload {
                    for (key, expected_value) in filters {
                        match map.get(key) {
                            Some(serde_json::Value::String(actual)) => {
                                if actual != expected_value {
                                    return false;
                                }
                            }
                            _ => return false,
                        }
                    }
                } else {
                    return false;
                }
            }

            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- TriggerType serialization ---

    #[test]
    fn cron_trigger_serialization_roundtrip() {
        let trigger = TriggerType::Cron {
            schedule: "0 9 * * MON-FRI".into(),
            timezone: Some("America/New_York".into()),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        let deserialized: TriggerType = serde_json::from_str(&json).unwrap();
        match deserialized {
            TriggerType::Cron { schedule, timezone } => {
                assert_eq!(schedule, "0 9 * * MON-FRI");
                assert_eq!(timezone.unwrap(), "America/New_York");
            }
            _ => panic!("expected Cron trigger"),
        }
    }

    #[test]
    fn event_trigger_serialization() {
        let trigger = TriggerType::Event {
            channel: Some("telegram".into()),
            pattern: r"(?i)deploy\s+\w+".into(),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        assert!(json.contains("\"type\":\"event\""));
        let deserialized: TriggerType = serde_json::from_str(&json).unwrap();
        match deserialized {
            TriggerType::Event { channel, pattern } => {
                assert_eq!(channel.unwrap(), "telegram");
                assert!(pattern.contains("deploy"));
            }
            _ => panic!("expected Event trigger"),
        }
    }

    #[test]
    fn system_event_trigger_serialization() {
        let mut filters = HashMap::new();
        filters.insert("repo".into(), "frankclaw".into());
        let trigger = TriggerType::SystemEvent {
            source: "github".into(),
            event_type: "issue.opened".into(),
            filters,
        };
        let json = serde_json::to_string(&trigger).unwrap();
        let deserialized: TriggerType = serde_json::from_str(&json).unwrap();
        match deserialized {
            TriggerType::SystemEvent {
                source,
                event_type,
                filters,
            } => {
                assert_eq!(source, "github");
                assert_eq!(event_type, "issue.opened");
                assert_eq!(filters.get("repo").unwrap(), "frankclaw");
            }
            _ => panic!("expected SystemEvent trigger"),
        }
    }

    #[test]
    fn manual_trigger_serialization() {
        let trigger = TriggerType::Manual;
        let json = serde_json::to_string(&trigger).unwrap();
        assert!(json.contains("\"type\":\"manual\""));
    }

    // --- Guardrails ---

    #[test]
    fn guardrails_defaults() {
        let g = TriggerGuardrails::default();
        assert_eq!(g.cooldown_secs, DEFAULT_COOLDOWN_SECS);
        assert_eq!(g.max_concurrent, DEFAULT_MAX_CONCURRENT);
        assert!(g.dedup_window_secs.is_none());
        assert_eq!(g.cooldown(), Duration::from_secs(300));
        assert!(g.dedup_window().is_none());
    }

    #[test]
    fn guardrails_dedup_window() {
        let g = TriggerGuardrails {
            dedup_window_secs: Some(60),
            ..Default::default()
        };
        assert_eq!(g.dedup_window().unwrap(), Duration::from_secs(60));
    }

    // --- TriggerState ---

    #[test]
    fn fresh_state_allows_fire() {
        let state = TriggerState::default();
        let guardrails = TriggerGuardrails::default();
        assert_eq!(state.can_fire(&guardrails), FireCheck::Allowed);
    }

    #[test]
    fn max_concurrent_blocks_fire() {
        let state = TriggerState {
            active_count: 1,
            ..Default::default()
        };
        let guardrails = TriggerGuardrails {
            max_concurrent: 1,
            ..Default::default()
        };
        match state.can_fire(&guardrails) {
            FireCheck::Blocked { reason } => assert!(reason.contains("max concurrent")),
            _ => panic!("expected blocked"),
        }
    }

    #[test]
    fn cooldown_blocks_fire() {
        let state = TriggerState {
            last_fired_at: Some(Utc::now()),
            ..Default::default()
        };
        let guardrails = TriggerGuardrails {
            cooldown_secs: 300,
            ..Default::default()
        };
        match state.can_fire(&guardrails) {
            FireCheck::Blocked { reason } => assert!(reason.contains("cooldown")),
            _ => panic!("expected blocked"),
        }
    }

    #[test]
    fn zero_cooldown_allows_immediate_refire() {
        let state = TriggerState {
            last_fired_at: Some(Utc::now()),
            ..Default::default()
        };
        let guardrails = TriggerGuardrails {
            cooldown_secs: 0,
            ..Default::default()
        };
        assert_eq!(state.can_fire(&guardrails), FireCheck::Allowed);
    }

    #[test]
    fn record_fire_updates_state() {
        let mut state = TriggerState::default();
        state.record_fire();
        assert_eq!(state.active_count, 1);
        assert_eq!(state.fire_count, 1);
        assert!(state.last_fired_at.is_some());
    }

    #[test]
    fn record_completion_success() {
        let mut state = TriggerState {
            active_count: 2,
            consecutive_failures: 3,
            ..Default::default()
        };
        state.record_completion(true);
        assert_eq!(state.active_count, 1);
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn record_completion_failure() {
        let mut state = TriggerState {
            active_count: 1,
            consecutive_failures: 2,
            ..Default::default()
        };
        state.record_completion(false);
        assert_eq!(state.active_count, 0);
        assert_eq!(state.consecutive_failures, 3);
    }

    // --- Event matching ---

    #[test]
    fn event_trigger_matches_message() {
        let trigger = TriggerType::Event {
            channel: None,
            pattern: r"(?i)deploy\s+\w+".into(),
        };
        assert!(matches_event_trigger(&trigger, "please deploy staging", None));
        assert!(!matches_event_trigger(&trigger, "hello world", None));
    }

    #[test]
    fn event_trigger_channel_filter() {
        let trigger = TriggerType::Event {
            channel: Some("telegram".into()),
            pattern: ".*".into(),
        };
        assert!(matches_event_trigger(
            &trigger,
            "anything",
            Some("telegram")
        ));
        assert!(matches_event_trigger(
            &trigger,
            "anything",
            Some("Telegram")
        ));
        assert!(!matches_event_trigger(
            &trigger,
            "anything",
            Some("discord")
        ));
        assert!(!matches_event_trigger(&trigger, "anything", None));
    }

    #[test]
    fn event_trigger_invalid_regex_returns_false() {
        let trigger = TriggerType::Event {
            channel: None,
            pattern: "[invalid regex".into(),
        };
        assert!(!matches_event_trigger(&trigger, "anything", None));
    }

    #[test]
    fn non_event_trigger_never_matches_message() {
        let trigger = TriggerType::Cron {
            schedule: "* * * * *".into(),
            timezone: None,
        };
        assert!(!matches_event_trigger(&trigger, "anything", None));
    }

    // --- System event matching ---

    #[test]
    fn system_event_matches() {
        let trigger = TriggerType::SystemEvent {
            source: "github".into(),
            event_type: "issue.opened".into(),
            filters: HashMap::new(),
        };
        let event = SystemEvent::new("GitHub", "issue.opened");
        assert!(matches_system_event(&trigger, &event));
    }

    #[test]
    fn system_event_source_mismatch() {
        let trigger = TriggerType::SystemEvent {
            source: "github".into(),
            event_type: "issue.opened".into(),
            filters: HashMap::new(),
        };
        let event = SystemEvent::new("gitlab", "issue.opened");
        assert!(!matches_system_event(&trigger, &event));
    }

    #[test]
    fn system_event_type_mismatch() {
        let trigger = TriggerType::SystemEvent {
            source: "github".into(),
            event_type: "issue.opened".into(),
            filters: HashMap::new(),
        };
        let event = SystemEvent::new("github", "issue.closed");
        assert!(!matches_system_event(&trigger, &event));
    }

    #[test]
    fn system_event_with_filters() {
        let mut filters = HashMap::new();
        filters.insert("repo".into(), "frankclaw".into());
        let trigger = TriggerType::SystemEvent {
            source: "github".into(),
            event_type: "push".into(),
            filters,
        };

        let event = SystemEvent::new("github", "push")
            .with_payload(serde_json::json!({"repo": "frankclaw", "branch": "main"}));
        assert!(matches_system_event(&trigger, &event));

        let event_wrong_repo = SystemEvent::new("github", "push")
            .with_payload(serde_json::json!({"repo": "other"}));
        assert!(!matches_system_event(&trigger, &event_wrong_repo));
    }

    #[test]
    fn system_event_filter_missing_field() {
        let mut filters = HashMap::new();
        filters.insert("repo".into(), "frankclaw".into());
        let trigger = TriggerType::SystemEvent {
            source: "github".into(),
            event_type: "push".into(),
            filters,
        };
        let event = SystemEvent::new("github", "push")
            .with_payload(serde_json::json!({"branch": "main"}));
        assert!(!matches_system_event(&trigger, &event));
    }

    #[test]
    fn system_event_filter_non_object_payload() {
        let mut filters = HashMap::new();
        filters.insert("repo".into(), "frankclaw".into());
        let trigger = TriggerType::SystemEvent {
            source: "github".into(),
            event_type: "push".into(),
            filters,
        };
        let event = SystemEvent::new("github", "push")
            .with_payload(serde_json::json!("just a string"));
        assert!(!matches_system_event(&trigger, &event));
    }

    #[test]
    fn non_system_event_trigger_never_matches() {
        let trigger = TriggerType::Manual;
        let event = SystemEvent::new("github", "push");
        assert!(!matches_system_event(&trigger, &event));
    }

    // --- RoutineAction ---

    #[test]
    fn lightweight_action_serialization() {
        let action = RoutineAction::Lightweight {
            prompt: "check status".into(),
            context_paths: vec!["README.md".into()],
            max_tokens: 1024,
        };
        let json = serde_json::to_string(&action).unwrap();
        let deserialized: RoutineAction = serde_json::from_str(&json).unwrap();
        match deserialized {
            RoutineAction::Lightweight {
                prompt,
                context_paths,
                max_tokens,
            } => {
                assert_eq!(prompt, "check status");
                assert_eq!(context_paths, vec!["README.md"]);
                assert_eq!(max_tokens, 1024);
            }
            _ => panic!("expected Lightweight"),
        }
    }

    #[test]
    fn full_job_action_serialization() {
        let action = RoutineAction::FullJob {
            title: "Deploy".into(),
            description: "Deploy to staging".into(),
            max_iterations: 20,
            tool_permissions: vec!["bash".into(), "write_file".into()],
        };
        let json = serde_json::to_string(&action).unwrap();
        let deserialized: RoutineAction = serde_json::from_str(&json).unwrap();
        match deserialized {
            RoutineAction::FullJob {
                title,
                max_iterations,
                tool_permissions,
                ..
            } => {
                assert_eq!(title, "Deploy");
                assert_eq!(max_iterations, 20);
                assert_eq!(tool_permissions.len(), 2);
            }
            _ => panic!("expected FullJob"),
        }
    }

    #[test]
    fn lightweight_action_defaults() {
        let json = r#"{"type":"lightweight","prompt":"test"}"#;
        let action: RoutineAction = serde_json::from_str(json).unwrap();
        match action {
            RoutineAction::Lightweight {
                max_tokens,
                context_paths,
                ..
            } => {
                assert_eq!(max_tokens, 2048);
                assert!(context_paths.is_empty());
            }
            _ => panic!("expected Lightweight"),
        }
    }
}
