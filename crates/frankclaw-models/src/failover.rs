use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::warn;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::model::{
    CompletionRequest, CompletionResponse, ModelDef, ModelProvider, StreamDelta,
};

/// Failover chain that tries providers in order, with cooldowns on failure.
pub struct FailoverChain {
    providers: Vec<Arc<dyn ModelProvider>>,
    cooldowns: DashMap<String, Instant>,
    cooldown_duration: Duration,
}

impl FailoverChain {
    pub fn new(providers: Vec<Arc<dyn ModelProvider>>, cooldown_secs: u64) -> Self {
        Self {
            providers,
            cooldowns: DashMap::new(),
            cooldown_duration: Duration::from_secs(cooldown_secs),
        }
    }

    /// Try each provider in order. Skip cooled-down providers.
    pub async fn complete(
        &self,
        request: CompletionRequest,
        stream_tx: Option<tokio::sync::mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse> {
        let mut last_error = None;

        for provider in &self.providers {
            let id = provider.id().to_string();

            // Skip if still cooling down.
            if let Some(until) = self.cooldowns.get(&id) {
                if Instant::now() < *until {
                    continue;
                }
                // Cooldown expired, remove it.
                drop(until);
                self.cooldowns.remove(&id);
            }

            match provider.complete(request.clone(), stream_tx.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    warn!(provider = %id, error = %e, "provider failed, trying next");
                    self.cooldowns
                        .insert(id, Instant::now() + self.cooldown_duration);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(FrankClawError::AllProvidersFailed))
    }

    /// List models from all non-cooled-down providers.
    pub async fn list_models(&self) -> Result<Vec<ModelDef>> {
        let mut all = Vec::new();
        for provider in &self.providers {
            match provider.list_models().await {
                Ok(models) => all.extend(models),
                Err(e) => {
                    warn!(provider = %provider.id(), error = %e, "failed to list models");
                }
            }
        }
        Ok(all)
    }
}
