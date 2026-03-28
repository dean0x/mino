//! Native sandbox - cross-platform dispatch via SandboxPlatform trait
//!
//! Defines the `SandboxPlatform` trait and a factory function that returns
//! the correct platform implementation based on the target OS.
//! Linux uses user namespaces via `unshare` + `pivot_root`.
//! macOS uses a dedicated system user + pf packet filter via a privileged helper.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use crate::error::MinoError;
use crate::error::MinoResult;
use crate::network::NetworkMode;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::process::SandboxProcess;

/// Configuration for spawning a native sandbox
pub struct SandboxSpawnConfig {
    /// Session identifier for tracking
    pub session_id: String,

    /// Host project directory to mount read-write at /workspace
    pub project_dir: PathBuf,

    /// Command and arguments to execute inside the sandbox
    pub command: Vec<String>,

    /// Environment variables to set in the sandbox
    pub env: HashMap<String, String>,

    /// Network isolation mode
    pub network_mode: NetworkMode,

    /// Sandbox-specific configuration (resource limits, paths, etc.)
    pub sandbox_config: SandboxConfig,

    /// Optional directory containing prepared dotfiles to copy into sandbox HOME
    pub dotfile_dir: Option<PathBuf>,

    /// Whether to inherit stdio (interactive mode)
    pub interactive: bool,
}

/// Platform-specific sandbox operations.
///
/// Each supported OS implements this trait to provide validate, spawn, exec,
/// and cleanup operations using its native isolation mechanisms.
#[async_trait]
pub trait SandboxPlatform: Send + Sync {
    /// Check if native sandbox prerequisites are met for this platform.
    async fn validate_setup(&self) -> MinoResult<()>;

    /// Spawn a sandboxed process using platform-specific isolation.
    async fn spawn(&self, config: SandboxSpawnConfig) -> MinoResult<SandboxProcess>;

    /// Execute a command inside an existing sandbox session.
    async fn exec(
        &self,
        pid: u32,
        session_name: &str,
        sandbox_user: &str,
        command: &[String],
    ) -> MinoResult<i32>;

    /// Clean up sandbox resources (ACLs, firewall rules, etc.).
    async fn cleanup(
        &self,
        session_id: &str,
        project_dir: &Path,
        sandbox_user: &str,
    ) -> MinoResult<()>;
}

/// Create the appropriate `SandboxPlatform` for the current OS.
pub fn create_sandbox_platform() -> MinoResult<Box<dyn SandboxPlatform>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(super::linux::LinuxSandbox))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(super::macos::MacosSandbox))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(MinoError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_sandbox_platform_returns_impl() {
        // On macOS and Linux, should return Ok. On other platforms, Err.
        let result = create_sandbox_platform();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!(result.is_ok());
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn validate_setup_returns_error_without_prerequisites() {
        let platform = create_sandbox_platform().unwrap();
        let result = platform.validate_setup().await;

        #[cfg(target_os = "macos")]
        {
            // In test environment, the helper binary won't be installed
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                err.to_string().contains("Native sandbox not set up")
                    || err.to_string().contains("Sandbox helper error"),
                "unexpected error: {}",
                err
            );
        }

        // On Linux, the result depends on the system configuration
        #[cfg(target_os = "linux")]
        {
            let _ = result;
        }
    }

    #[tokio::test]
    async fn spawn_config_has_correct_fields() {
        let config = SandboxSpawnConfig {
            session_id: "test-sess".to_string(),
            project_dir: PathBuf::from("/tmp"),
            command: vec!["true".to_string()],
            env: HashMap::from([("KEY".to_string(), "val".to_string())]),
            network_mode: NetworkMode::Bridge,
            sandbox_config: SandboxConfig::default(),
            dotfile_dir: Some(PathBuf::from("/tmp/dots")),
            interactive: true,
        };

        assert_eq!(config.session_id, "test-sess");
        assert_eq!(config.project_dir, PathBuf::from("/tmp"));
        assert_eq!(config.command, vec!["true"]);
        assert_eq!(config.env.get("KEY").unwrap(), "val");
        assert!(config.interactive);
        assert_eq!(config.dotfile_dir, Some(PathBuf::from("/tmp/dots")));
    }
}
