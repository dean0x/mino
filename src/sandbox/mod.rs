//! Native sandbox mode for process-level isolation
//!
//! Provides kernel-enforced process isolation without containers or VMs.
//! Linux: User namespaces (unshare). macOS: Dedicated user + pf packet filter.

pub mod config;
pub mod detection;
pub mod dotfiles;
pub mod fs_copy;
pub mod helper;
pub mod helper_protocol;
pub mod linux;
pub mod macos;
pub mod native;
pub mod process;
pub mod proxy;
pub mod resource_limits;

use crate::error::{MinoError, MinoResult};
use serde::{Deserialize, Serialize};

/// Runtime execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    /// Traditional container-based isolation (Podman)
    Container,
    /// Native kernel-level process isolation (no container/VM)
    Native,
}

impl std::str::FromStr for RuntimeMode {
    type Err = MinoError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "container" => Ok(Self::Container),
            "native" => Ok(Self::Native),
            other => Err(MinoError::User(format!(
                "Invalid runtime mode '{}'. Valid modes: container, native",
                other
            ))),
        }
    }
}

impl std::fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container => write!(f, "container"),
            Self::Native => write!(f, "native"),
        }
    }
}

/// Resolve the effective runtime mode from CLI flag and config value.
/// CLI flag takes precedence. "auto" checks if native sandbox is set up.
pub fn resolve_runtime_mode(
    cli_runtime: Option<&str>,
    config_runtime: &str,
) -> MinoResult<RuntimeMode> {
    // CLI takes precedence
    let raw = cli_runtime.unwrap_or(config_runtime);

    match raw.to_lowercase().as_str() {
        // "auto" currently defaults to Container until native setup detection exists
        "auto" => Ok(RuntimeMode::Container),
        other => other.parse::<RuntimeMode>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_container_mode() {
        let mode = "container".parse::<RuntimeMode>().unwrap();
        assert_eq!(mode, RuntimeMode::Container);
    }

    #[test]
    fn parse_native_mode() {
        let mode = "native".parse::<RuntimeMode>().unwrap();
        assert_eq!(mode, RuntimeMode::Native);
    }

    #[test]
    fn parse_invalid_mode() {
        let err = "docker".parse::<RuntimeMode>().unwrap_err();
        assert!(err.to_string().contains("Invalid runtime mode"));
        assert!(err.to_string().contains("docker"));
    }

    #[test]
    fn to_string_roundtrip() {
        let container = RuntimeMode::Container;
        let parsed: RuntimeMode = container.to_string().parse().unwrap();
        assert_eq!(parsed, container);

        let native = RuntimeMode::Native;
        let parsed: RuntimeMode = native.to_string().parse().unwrap();
        assert_eq!(parsed, native);
    }

    #[test]
    fn serde_roundtrip() {
        let native = RuntimeMode::Native;
        let json = serde_json::to_string(&native).unwrap();
        assert_eq!(json, "\"native\"");
        let parsed: RuntimeMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, native);

        let container = RuntimeMode::Container;
        let json = serde_json::to_string(&container).unwrap();
        assert_eq!(json, "\"container\"");
        let parsed: RuntimeMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, container);
    }

    #[test]
    fn resolve_cli_overrides_config() {
        let mode = resolve_runtime_mode(Some("native"), "container").unwrap();
        assert_eq!(mode, RuntimeMode::Native);
    }

    #[test]
    fn resolve_config_fallback() {
        let mode = resolve_runtime_mode(None, "native").unwrap();
        assert_eq!(mode, RuntimeMode::Native);
    }

    #[test]
    fn resolve_auto_defaults_to_container() {
        let mode = resolve_runtime_mode(None, "auto").unwrap();
        assert_eq!(mode, RuntimeMode::Container);
    }

    #[test]
    fn resolve_auto_from_cli() {
        let mode = resolve_runtime_mode(Some("auto"), "native").unwrap();
        assert_eq!(mode, RuntimeMode::Container);
    }

    #[test]
    fn resolve_invalid_returns_error() {
        let err = resolve_runtime_mode(Some("podman"), "container").unwrap_err();
        assert!(err.to_string().contains("Invalid runtime mode"));
    }
}
