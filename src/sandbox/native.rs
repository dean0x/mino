//! Native sandbox - cross-platform dispatch
//!
//! Routes to platform-specific implementations based on the target OS.
//! Linux uses user namespaces via `unshare` + `pivot_root`.
//! macOS uses a dedicated system user + pf packet filter via a privileged helper.

#[allow(unused_imports)]
use crate::error::{MinoError, MinoResult};
use crate::network::NetworkMode;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::process::SandboxProcess;
use std::collections::HashMap;
use std::path::PathBuf;

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

/// Native sandbox facade — dispatches to platform-specific implementations
pub struct NativeSandbox;

impl NativeSandbox {
    /// Check if native sandbox prerequisites are met for this platform
    pub async fn validate_setup() -> MinoResult<()> {
        #[cfg(target_os = "linux")]
        {
            super::linux::validate_linux_setup().await
        }
        #[cfg(target_os = "macos")]
        {
            super::macos::validate_macos_setup().await
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(MinoError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            ))
        }
    }

    /// Spawn a sandboxed process using platform-specific isolation
    pub async fn spawn(config: SandboxSpawnConfig) -> MinoResult<SandboxProcess> {
        #[cfg(target_os = "linux")]
        {
            super::linux::spawn_linux_sandbox(config).await
        }
        #[cfg(target_os = "macos")]
        {
            super::macos::spawn_macos_sandbox(config).await
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = config;
            Err(MinoError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn validate_setup_returns_error_without_prerequisites() {
        // On macOS: returns SandboxNotSetup (helper not installed in test env)
        // On Linux: result depends on kernel namespace support
        let result = NativeSandbox::validate_setup().await;

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
            // Just verify it doesn't panic — the result depends on the host
            let _ = result;
        }
    }

    #[tokio::test]
    async fn spawn_config_has_correct_fields() {
        // Verify SandboxSpawnConfig construction works and holds all fields
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
