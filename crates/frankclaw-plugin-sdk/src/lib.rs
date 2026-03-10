#![forbid(unsafe_code)]
#![doc = "Plugin SDK for extending FrankClaw with custom channels, tools, and memory backends."]

use std::sync::Arc;
use tokio::sync::mpsc;

use frankclaw_core::channel::{ChannelPlugin, InboundMessage};
use frankclaw_core::types::ChannelId;

/// Registry of loaded plugins.
pub struct PluginRegistry {
    channels: Vec<Arc<dyn ChannelPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// Register a channel plugin.
    pub fn register_channel(&mut self, plugin: Arc<dyn ChannelPlugin>) {
        tracing::info!(channel = %plugin.id(), "registered channel plugin");
        self.channels.push(plugin);
    }

    /// Get a channel plugin by ID.
    pub fn get_channel(&self, id: &ChannelId) -> Option<&Arc<dyn ChannelPlugin>> {
        self.channels.iter().find(|p| p.id() == *id)
    }

    /// List all registered channels.
    pub fn list_channels(&self) -> &[Arc<dyn ChannelPlugin>] {
        &self.channels
    }

    /// Start all registered channels, feeding inbound messages to the provided sender.
    pub async fn start_all_channels(
        &self,
        inbound_tx: mpsc::Sender<InboundMessage>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();

        for plugin in &self.channels {
            let plugin = plugin.clone();
            let tx = inbound_tx.clone();
            let handle = tokio::spawn(async move {
                if let Err(e) = plugin.start(tx).await {
                    tracing::error!(channel = %plugin.id(), error = %e, "channel stopped with error");
                }
            });
            handles.push(handle);
        }

        handles
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
