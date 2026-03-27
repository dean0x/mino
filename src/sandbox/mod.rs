//! Native sandbox mode for process-level isolation
//!
//! Provides kernel-enforced process isolation without containers or VMs.
//! Linux: User namespaces (unshare). macOS: Dedicated user + pf packet filter.

pub mod config;
pub mod dotfiles;
pub mod resource_limits;

use crate::error::{MinoError, MinoResult};

/// Runtime execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    /// Traditional container-based isolation (Podman)
    Container,
    /// Native kernel-level process isolation (no container/VM)
    Native,
}

impl RuntimeMode {
    /// Parse from string (config/CLI value)
    pub fn parse(s: &str) -> MinoResult<Self> {
        match s.to_lowercase().as_str() {
            "container" => Ok(Self::Container),
            "native" => Ok(Self::Native),
            other => Err(MinoError::User(format!(
                "Invalid runtime mode '{}'. Valid modes: container, native",
                other
            ))),
        }
    }

    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Container => "Container",
            Self::Native => "Native",
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
        other => RuntimeMode::parse(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_container_mode() {
        let mode = RuntimeMode::parse("container").unwrap();
        assert_eq!(mode, RuntimeMode::Container);
    }

    #[test]
    fn parse_native_mode() {
        let mode = RuntimeMode::parse("native").unwrap();
        assert_eq!(mode, RuntimeMode::Native);
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(RuntimeMode::parse("Container").unwrap(), RuntimeMode::Container);
        assert_eq!(RuntimeMode::parse("NATIVE").unwrap(), RuntimeMode::Native);
    }

    #[test]
    fn parse_invalid_mode() {
        let err = RuntimeMode::parse("docker").unwrap_err();
        assert!(err.to_string().contains("Invalid runtime mode"));
        assert!(err.to_string().contains("docker"));
    }

    #[test]
    fn display_name() {
        assert_eq!(RuntimeMode::Container.display_name(), "Container");
        assert_eq!(RuntimeMode::Native.display_name(), "Native");
    }

    #[test]
    fn to_string_roundtrip() {
        let container = RuntimeMode::Container;
        let parsed = RuntimeMode::parse(&container.to_string()).unwrap();
        assert_eq!(parsed, container);

        let native = RuntimeMode::Native;
        let parsed = RuntimeMode::parse(&native.to_string()).unwrap();
        assert_eq!(parsed, native);
    }

    #[test]
    fn display_format() {
        assert_eq!(RuntimeMode::Container.to_string(), "container");
        assert_eq!(RuntimeMode::Native.to_string(), "native");
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
