//! Minotaur - Secure AI Agent Sandbox Wrapper
//!
//! Wraps any command in OrbStack + Podman rootless containers with
//! temporary cloud credentials and SSH agent forwarding.

pub mod cli;
pub mod config;
#[path = "creds/mod.rs"]
pub mod credentials;
pub mod error;
pub mod orchestration;
pub mod session;

pub use error::{MinotaurError, MinotaurResult};
