//! Container runtime abstraction
//!
//! Provides a trait for container operations that can be implemented
//! by different backends (OrbStack+Podman on macOS, native Podman on Linux).

use crate::error::MinoResult;
use crate::orchestration::podman::ContainerConfig;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;

/// Information about a container volume
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    /// Volume name
    pub name: String,
    /// Volume labels
    pub labels: HashMap<String, String>,
    /// Mount point path (inside container runtime)
    pub mountpoint: Option<String>,
    /// Creation timestamp (RFC3339)
    pub created_at: Option<String>,
    /// Size in bytes (if available)
    pub size_bytes: Option<u64>,
}

/// Abstract container runtime interface
///
/// This trait allows mino to work with different container runtimes:
/// - macOS: OrbStack VM + Podman
/// - Linux: Native rootless Podman
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Check if the runtime is available on this system
    async fn is_available(&self) -> MinoResult<bool>;

    /// Ensure the runtime is ready (start VM, check rootless setup, etc.)
    async fn ensure_ready(&self) -> MinoResult<()>;

    /// Run a container in detached mode and return the container ID
    async fn run(&self, config: &ContainerConfig, command: &[String]) -> MinoResult<String>;

    /// Create a container without starting it. Returns container ID.
    async fn create(&self, config: &ContainerConfig, command: &[String]) -> MinoResult<String>;

    /// Start a created container attached to the terminal. Returns exit code.
    async fn start_attached(&self, container_id: &str) -> MinoResult<i32>;

    /// Attach to a running container interactively
    async fn attach(&self, container_id: &str) -> MinoResult<i32>;

    /// Stop a container gracefully
    async fn stop(&self, container_id: &str) -> MinoResult<()>;

    /// Kill a container immediately
    async fn kill(&self, container_id: &str) -> MinoResult<()>;

    /// Remove a container
    async fn remove(&self, container_id: &str) -> MinoResult<()>;

    /// Get container logs
    async fn logs(&self, container_id: &str, lines: u32) -> MinoResult<String>;

    /// Follow container logs interactively
    async fn logs_follow(&self, container_id: &str) -> MinoResult<()>;

    /// Check if a container image exists locally
    async fn image_exists(&self, image: &str) -> MinoResult<bool>;

    /// Build an image from a context directory
    async fn build_image(&self, context_dir: &Path, tag: &str) -> MinoResult<()>;

    /// Remove a container image
    async fn image_remove(&self, image: &str) -> MinoResult<()>;

    /// List images matching a name prefix
    async fn image_list_prefixed(&self, prefix: &str) -> MinoResult<Vec<String>>;

    /// Get the human-readable runtime name for display
    fn runtime_name(&self) -> &'static str;

    // Volume operations for persistent caching

    /// Create a new volume with the given name and labels
    async fn volume_create(
        &self,
        name: &str,
        labels: &HashMap<String, String>,
    ) -> MinoResult<()>;

    /// Check if a volume exists
    async fn volume_exists(&self, name: &str) -> MinoResult<bool>;

    /// Remove a volume
    async fn volume_remove(&self, name: &str) -> MinoResult<()>;

    /// List volumes matching a name prefix
    async fn volume_list(&self, prefix: &str) -> MinoResult<Vec<VolumeInfo>>;

    /// Get detailed info about a specific volume
    async fn volume_inspect(&self, name: &str) -> MinoResult<Option<VolumeInfo>>;

    /// Update labels on an existing volume
    /// Note: Podman doesn't support label updates directly, so this removes and recreates
    /// the volume. Only use for state transitions, not for volumes with data.
    async fn volume_update_labels(
        &self,
        name: &str,
        labels: &HashMap<String, String>,
    ) -> MinoResult<()>;

    /// Get disk usage for volumes matching a prefix
    /// Returns a map of volume name -> size in bytes
    async fn volume_disk_usage(&self, prefix: &str) -> MinoResult<HashMap<String, u64>>;
}
