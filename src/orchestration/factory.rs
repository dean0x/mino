//! Runtime factory for creating platform-appropriate container runtimes
//!
//! Provides automatic platform detection and runtime instantiation.

use crate::config::schema::VmConfig;
use crate::config::Config;
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::native_podman::NativePodmanRuntime;
use crate::orchestration::orbstack_runtime::OrbStackRuntime;
use crate::orchestration::runtime::ContainerRuntime;

/// Detected platform
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS - uses OrbStack + Podman
    MacOS,
    /// Linux - uses native rootless Podman
    Linux,
    /// Unsupported platform
    Unsupported,
}

impl Platform {
    /// Detect the current platform
    pub fn detect() -> Self {
        match std::env::consts::OS {
            "macos" => Platform::MacOS,
            "linux" => Platform::Linux,
            _ => Platform::Unsupported,
        }
    }

    /// Get a human-readable platform name
    pub fn name(&self) -> &'static str {
        match self {
            Platform::MacOS => "macOS",
            Platform::Linux => "Linux",
            Platform::Unsupported => "Unsupported",
        }
    }
}

/// Create a container runtime appropriate for the current platform
///
/// # Arguments
/// * `config` - The application configuration
///
/// # Returns
/// * `Ok(Box<dyn ContainerRuntime>)` - A boxed runtime implementation
/// * `Err` - If the platform is unsupported
pub fn create_runtime(config: &Config) -> MinotaurResult<Box<dyn ContainerRuntime>> {
    match Platform::detect() {
        Platform::MacOS => Ok(Box::new(OrbStackRuntime::new(config.vm.clone()))),
        Platform::Linux => Ok(Box::new(NativePodmanRuntime::new())),
        Platform::Unsupported => Err(MinotaurError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        )),
    }
}

/// Create a container runtime with explicit VM config (for status checks)
///
/// This variant is useful when you need to create a runtime with specific
/// VM configuration that may differ from the main config.
pub fn create_runtime_with_vm(vm_config: VmConfig) -> MinotaurResult<Box<dyn ContainerRuntime>> {
    match Platform::detect() {
        Platform::MacOS => Ok(Box::new(OrbStackRuntime::new(vm_config))),
        Platform::Linux => Ok(Box::new(NativePodmanRuntime::new())),
        Platform::Unsupported => Err(MinotaurError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_detect_returns_valid() {
        let platform = Platform::detect();
        // Should be one of the known platforms on any test machine
        assert!(matches!(
            platform,
            Platform::MacOS | Platform::Linux | Platform::Unsupported
        ));
    }

    #[test]
    fn platform_name() {
        assert_eq!(Platform::MacOS.name(), "macOS");
        assert_eq!(Platform::Linux.name(), "Linux");
        assert_eq!(Platform::Unsupported.name(), "Unsupported");
    }

    #[test]
    fn create_runtime_succeeds_on_supported_platform() {
        let config = Config::default();
        let result = create_runtime(&config);
        // On macOS or Linux, this should succeed
        // On other platforms, it should fail with UnsupportedPlatform
        match Platform::detect() {
            Platform::MacOS | Platform::Linux => {
                assert!(result.is_ok());
            }
            Platform::Unsupported => {
                assert!(result.is_err());
            }
        }
    }
}
