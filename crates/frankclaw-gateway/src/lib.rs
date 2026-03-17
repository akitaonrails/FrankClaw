#![forbid(unsafe_code)]

pub mod acp;
pub mod acp_transport;
pub mod audit;
pub mod auth;
pub mod broadcast;
pub mod canvas;
pub mod delivery;
pub mod health_monitor;
pub mod methods;
pub mod openai_api;
pub mod pairing;
pub mod rate_limit;
pub mod server;
pub mod state;
pub mod ui;
pub mod tunnel;
pub mod webhook_limiter;
pub mod webhooks;
pub mod ws;

#[cfg(test)]
pub(crate) mod test_fixtures;
