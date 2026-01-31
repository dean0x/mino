//! Orchestration module for container runtimes
//!
//! Provides platform-agnostic container management:
//! - macOS: OrbStack VM + Podman
//! - Linux: Native rootless Podman

mod factory;
mod native_podman;
pub mod orbstack;
mod orbstack_runtime;
pub mod podman;
mod runtime;

pub use factory::{create_runtime, create_runtime_with_vm, Platform};
pub use orbstack::OrbStack;
pub use podman::{ContainerConfig, ContainerInfo};
pub use runtime::ContainerRuntime;
