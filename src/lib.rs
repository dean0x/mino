//! Minotaur - Secure AI Agent Sandbox
//!
//! Wraps any command in rootless containers with temporary cloud
//! credentials and SSH agent forwarding.

pub mod audit;
pub mod cache;
pub mod cli;
pub mod config;
#[path = "creds/mod.rs"]
pub mod credentials;
pub mod error;
pub mod layer;
pub mod orchestration;
pub mod session;
pub mod ui;

pub use error::{MinotaurError, MinotaurResult};
