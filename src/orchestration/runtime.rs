//! Container runtime abstraction
//!
//! Provides a trait for container operations that can be implemented
//! by different backends (OrbStack+Podman on macOS, native Podman on Linux).

use crate::error::MinotaurResult;
use crate::orchestration::podman::ContainerConfig;
use async_trait::async_trait;

/// Abstract container runtime interface
///
/// This trait allows minotaur to work with different container runtimes:
/// - macOS: OrbStack VM + Podman
/// - Linux: Native rootless Podman
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Check if the runtime is available on this system
    async fn is_available(&self) -> MinotaurResult<bool>;

    /// Ensure the runtime is ready (start VM, check rootless setup, etc.)
    async fn ensure_ready(&self) -> MinotaurResult<()>;

    /// Run a container and return the container ID
    async fn run(&self, config: &ContainerConfig, command: &[String]) -> MinotaurResult<String>;

    /// Attach to a running container interactively
    async fn attach(&self, container_id: &str) -> MinotaurResult<i32>;

    /// Stop a container gracefully
    async fn stop(&self, container_id: &str) -> MinotaurResult<()>;

    /// Kill a container immediately
    async fn kill(&self, container_id: &str) -> MinotaurResult<()>;

    /// Remove a container
    async fn remove(&self, container_id: &str) -> MinotaurResult<()>;

    /// Get container logs
    async fn logs(&self, container_id: &str, lines: u32) -> MinotaurResult<String>;

    /// Follow container logs interactively
    async fn logs_follow(&self, container_id: &str) -> MinotaurResult<()>;

    /// Get the human-readable runtime name for display
    fn runtime_name(&self) -> &'static str;
}
