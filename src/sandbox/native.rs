//! Native sandbox - cross-platform dispatch
//!
//! Routes to platform-specific implementations based on the target OS.
//! Linux uses user namespaces via `unshare` + `pivot_root`.
//! macOS support is planned for a future phase.

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
            // TODO(Phase 4): macOS validation via dedicated user + pf firewall
            Err(MinoError::NativeUnsupported {
                feature: "native sandbox on macOS".to_string(),
            })
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
            let _ = config;
            Err(MinoError::NativeUnsupported {
                feature: "native sandbox on macOS".to_string(),
            })
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

    #[test]
    fn spawn_config_builds_correctly() {
        let config = SandboxSpawnConfig {
            session_id: "test-123".to_string(),
            project_dir: PathBuf::from("/home/user/project"),
            command: vec!["echo".to_string(), "hello".to_string()],
            env: HashMap::from([("HOME".to_string(), "/home/agent".to_string())]),
            network_mode: NetworkMode::Bridge,
            sandbox_config: SandboxConfig::default(),
            dotfile_dir: None,
            interactive: true,
        };

        assert_eq!(config.session_id, "test-123");
        assert_eq!(config.project_dir, PathBuf::from("/home/user/project"));
        assert_eq!(config.command, vec!["echo", "hello"]);
        assert!(config.interactive);
        assert!(config.dotfile_dir.is_none());
    }

    #[tokio::test]
    async fn validate_setup_returns_unsupported_on_macos() {
        // On macOS (where tests typically run), this should return NativeUnsupported
        // On Linux, this may succeed or fail depending on namespace support
        let result = NativeSandbox::validate_setup().await;

        #[cfg(target_os = "macos")]
        {
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("not supported in native sandbox"));
        }

        // On Linux, the result depends on the system configuration
        #[cfg(target_os = "linux")]
        {
            // Just verify it doesn't panic — the result depends on the host
            let _ = result;
        }
    }

    #[tokio::test]
    async fn spawn_returns_unsupported_on_macos() {
        let config = SandboxSpawnConfig {
            session_id: "test".to_string(),
            project_dir: PathBuf::from("/tmp"),
            command: vec!["true".to_string()],
            env: HashMap::new(),
            network_mode: NetworkMode::Bridge,
            sandbox_config: SandboxConfig::default(),
            dotfile_dir: None,
            interactive: false,
        };

        let result = NativeSandbox::spawn(config).await;

        #[cfg(target_os = "macos")]
        {
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("not supported in native sandbox"));
        }
    }
}
