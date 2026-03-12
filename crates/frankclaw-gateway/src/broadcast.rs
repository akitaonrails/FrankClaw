use tokio::sync::broadcast;

/// Server-push event broadcast to all connected WebSocket clients.
///
/// Uses a tokio broadcast channel. Slow consumers are dropped (lagged)
/// rather than blocking the broadcaster — this prevents a single slow
/// client from stalling the entire gateway.
pub struct BroadcastHandle {
    tx: broadcast::Sender<String>,
}

impl BroadcastHandle {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Broadcast a JSON-serialized event to all subscribers.
    /// Returns the number of receivers that got the message.
    pub fn send(&self, message: String) -> usize {
        self.tx.send(message).unwrap_or(0)
    }

    /// Subscribe to broadcasts. Used by each WS connection task.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// Clone the underlying sender for sharing with other components.
    pub fn sender(&self) -> broadcast::Sender<String> {
        self.tx.clone()
    }
}
