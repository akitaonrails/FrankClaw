//! Event-driven hook system for extensibility.
//!
//! Hooks allow external code to react to lifecycle events (commands, sessions,
//! messages, etc.) without coupling to the core runtime. Execution is
//! fire-and-forget: hook errors are logged but never block the caller.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

/// Categories of hook events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Command,
    Session,
    Agent,
    Gateway,
    Message,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command => write!(f, "command"),
            Self::Session => write!(f, "session"),
            Self::Agent => write!(f, "agent"),
            Self::Gateway => write!(f, "gateway"),
            Self::Message => write!(f, "message"),
        }
    }
}

/// A hook event with context payload.
#[derive(Debug, Clone)]
pub struct HookEvent {
    /// Event category.
    pub event_type: EventType,
    /// Specific action within the category (e.g., "new", "reset", "received").
    pub action: String,
    /// Key-value context data.
    pub context: HashMap<String, serde_json::Value>,
}

impl HookEvent {
    /// Create a new hook event.
    pub fn new(event_type: EventType, action: impl Into<String>) -> Self {
        Self {
            event_type,
            action: action.into(),
            context: HashMap::new(),
        }
    }

    /// Add a context value.
    pub fn with(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        if let Ok(v) = serde_json::to_value(value) {
            self.context.insert(key.into(), v);
        }
        self
    }

    /// Composite key for specific event matching (e.g., "command:reset").
    pub fn specific_key(&self) -> String {
        format!("{}:{}", self.event_type, self.action)
    }
}

/// Type alias for async hook handler functions.
type HandlerFn = Arc<
    dyn Fn(HookEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

/// Registration entry for a hook handler.
struct HandlerEntry {
    name: String,
    handler: HandlerFn,
}

/// Registry for hook handlers.
///
/// Handlers are registered against event types (general or specific).
/// When an event fires, all matching handlers execute in parallel.
pub struct HookRegistry {
    /// Handlers keyed by event key (either "command" or "command:reset").
    handlers: RwLock<HashMap<String, Vec<HandlerEntry>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a handler for a general event type.
    ///
    /// The handler will fire for all events of this type regardless of action.
    pub async fn on<F, Fut>(&self, event_type: EventType, name: impl Into<String>, handler: F)
    where
        F: Fn(HookEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let key = event_type.to_string();
        self.register(key, name.into(), handler).await;
    }

    /// Register a handler for a specific event type + action pair.
    ///
    /// The handler will only fire when both the type and action match.
    pub async fn on_action<F, Fut>(
        &self,
        event_type: EventType,
        action: impl Into<String>,
        name: impl Into<String>,
        handler: F,
    ) where
        F: Fn(HookEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let key = format!("{}:{}", event_type, action.into());
        self.register(key, name.into(), handler).await;
    }

    /// Fire an event, running all matching handlers.
    ///
    /// Handlers for both the general type and specific type:action pair are run.
    /// Execution is fire-and-forget: errors are logged but don't propagate.
    pub async fn fire(&self, event: HookEvent) {
        let handlers = self.handlers.read().await;

        let general_key = event.event_type.to_string();
        let specific_key = event.specific_key();

        let general = handlers.get(&general_key);
        let specific = handlers.get(&specific_key);

        let total = general.map_or(0, std::vec::Vec::len)
            + specific.map_or(0, std::vec::Vec::len);

        if total == 0 {
            return;
        }

        // Collect all handlers to run.
        let mut tasks = Vec::with_capacity(total);

        if let Some(entries) = general {
            for entry in entries {
                let event = event.clone();
                let handler = entry.handler.clone();
                let name = entry.name.clone();
                tasks.push((name, handler, event));
            }
        }

        if let Some(entries) = specific {
            for entry in entries {
                let event = event.clone();
                let handler = entry.handler.clone();
                let name = entry.name.clone();
                tasks.push((name, handler, event));
            }
        }

        // Run all handlers concurrently (fire-and-forget).
        for (name, handler, event) in tasks {
            let event_key = event.specific_key();
            tokio::spawn(async move {
                let result =
                    tokio::time::timeout(std::time::Duration::from_secs(30), handler(event)).await;
                if result.is_err() {
                    warn!(hook = %name, event = %event_key, "hook timed out after 30s");
                }
            });
        }
    }

    /// Number of registered handlers across all event keys.
    pub async fn handler_count(&self) -> usize {
        self.handlers
            .read()
            .await
            .values()
            .map(std::vec::Vec::len)
            .sum()
    }

    /// Remove all handlers for a given event key.
    pub async fn clear(&self, event_type: EventType) {
        let key = event_type.to_string();
        let mut handlers = self.handlers.write().await;
        handlers.remove(&key);
    }

    async fn register<F, Fut>(&self, key: String, name: String, handler: F)
    where
        F: Fn(HookEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let wrapped: HandlerFn = Arc::new(move |event| Box::pin(handler(event)));
        let entry = HandlerEntry {
            name,
            handler: wrapped,
        };
        let mut handlers = self.handlers.write().await;
        handlers.entry(key).or_default().push(entry);
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Convenience constructors for common events ──────────────────────────

impl HookEvent {
    pub fn command_executed(command: &str, args: &str) -> Self {
        Self::new(EventType::Command, command)
            .with("command", command)
            .with("args", args)
    }

    pub fn session_created(session_key: &str) -> Self {
        Self::new(EventType::Session, "created")
            .with("session_key", session_key)
    }

    pub fn session_reset(session_key: &str) -> Self {
        Self::new(EventType::Session, "reset")
            .with("session_key", session_key)
    }

    pub fn message_received(channel: &str, sender: &str, content: &str) -> Self {
        Self::new(EventType::Message, "received")
            .with("channel", channel)
            .with("sender", sender)
            .with("content", content)
    }

    pub fn message_sent(channel: &str, recipient: &str) -> Self {
        Self::new(EventType::Message, "sent")
            .with("channel", channel)
            .with("recipient", recipient)
    }

    pub fn gateway_started(port: u16) -> Self {
        Self::new(EventType::Gateway, "started")
            .with("port", port)
    }

    pub fn gateway_stopped() -> Self {
        Self::new(EventType::Gateway, "stopped")
    }

    pub fn agent_turn_started(agent_id: &str, session_key: &str) -> Self {
        Self::new(EventType::Agent, "turn_started")
            .with("agent_id", agent_id)
            .with("session_key", session_key)
    }

    pub fn agent_turn_completed(agent_id: &str, session_key: &str) -> Self {
        Self::new(EventType::Agent, "turn_completed")
            .with("agent_id", agent_id)
            .with("session_key", session_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn fire_general_handler() {
        let registry = HookRegistry::new();
        let counter = Arc::new(AtomicU32::new(0));

        let c = counter.clone();
        registry
            .on(EventType::Command, "test", move |_event| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                }
            })
            .await;

        registry
            .fire(HookEvent::command_executed("help", ""))
            .await;

        // Give the spawned task time to run.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn fire_specific_handler() {
        let registry = HookRegistry::new();
        let counter = Arc::new(AtomicU32::new(0));

        let c = counter.clone();
        registry
            .on_action(EventType::Session, "reset", "test", move |_event| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                }
            })
            .await;

        // Should fire for session:reset.
        registry
            .fire(HookEvent::session_reset("s1"))
            .await;

        // Should NOT fire for session:created.
        registry
            .fire(HookEvent::session_created("s2"))
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn both_general_and_specific_fire() {
        let registry = HookRegistry::new();
        let counter = Arc::new(AtomicU32::new(0));

        let c1 = counter.clone();
        registry
            .on(EventType::Session, "general", move |_| {
                let c = c1.clone();
                async move { c.fetch_add(1, Ordering::Relaxed); }
            })
            .await;

        let c2 = counter.clone();
        registry
            .on_action(EventType::Session, "reset", "specific", move |_| {
                let c = c2.clone();
                async move { c.fetch_add(10, Ordering::Relaxed); }
            })
            .await;

        registry.fire(HookEvent::session_reset("s1")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Both handlers should fire: 1 + 10 = 11.
        assert_eq!(counter.load(Ordering::Relaxed), 11);
    }

    #[tokio::test]
    async fn no_handlers_is_noop() {
        let registry = HookRegistry::new();
        registry.fire(HookEvent::gateway_started(8080)).await;
        // No panic, no error.
    }

    #[tokio::test]
    async fn handler_count_tracks_registrations() {
        let registry = HookRegistry::new();
        assert_eq!(registry.handler_count().await, 0);

        registry
            .on(EventType::Command, "h1", |_| async {})
            .await;
        registry
            .on(EventType::Message, "h2", |_| async {})
            .await;

        assert_eq!(registry.handler_count().await, 2);
    }

    #[tokio::test]
    async fn clear_removes_handlers() {
        let registry = HookRegistry::new();

        registry
            .on(EventType::Command, "h1", |_| async {})
            .await;
        registry
            .on(EventType::Command, "h2", |_| async {})
            .await;

        assert_eq!(registry.handler_count().await, 2);

        registry.clear(EventType::Command).await;
        assert_eq!(registry.handler_count().await, 0);
    }

    #[test]
    fn event_constructors_set_context() {
        let e = HookEvent::command_executed("help", "verbose");
        assert_eq!(e.event_type, EventType::Command);
        assert_eq!(e.action, "help");
        assert_eq!(e.context["command"], "help");
        assert_eq!(e.context["args"], "verbose");
    }

    #[test]
    fn event_specific_key_format() {
        let e = HookEvent::session_reset("s1");
        assert_eq!(e.specific_key(), "session:reset");
    }

    #[test]
    fn event_with_chaining() {
        let e = HookEvent::new(EventType::Message, "received")
            .with("channel", "telegram")
            .with("count", 42);
        assert_eq!(e.context["channel"], "telegram");
        assert_eq!(e.context["count"], 42);
    }

    #[tokio::test]
    async fn handler_error_does_not_crash() {
        let registry = HookRegistry::new();

        registry
            .on(EventType::Command, "panicky", |_| async {
                panic!("hook panic!");
            })
            .await;

        // Should not propagate the panic.
        registry
            .fire(HookEvent::command_executed("test", ""))
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // If we reach here, the panic was contained.
    }
}
