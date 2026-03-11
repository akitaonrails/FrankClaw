#![forbid(unsafe_code)]

mod failover;
mod openai;
mod anthropic;
mod ollama;

pub use failover::{FailoverChain, ProviderHealth};
pub use openai::OpenAiProvider;
pub use anthropic::AnthropicProvider;
pub use ollama::OllamaProvider;
