//! Sandbox configuration types
//!
//! Defines configuration for native sandbox mode including resource limits,
//! path validation, and security-sensitive path blocking.

use crate::error::{MinoError, MinoResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Paths that are ALWAYS blocked from passthrough (credential stores)
const SENSITIVE_PATHS: &[&str] = &[
    ".ssh",
    ".aws",
    ".azure",
    ".config/gh",
    ".gnupg",
    ".config/gcloud",
    ".kube",
];

/// Valid cache access modes
const VALID_CACHE_MODES: &[&str] = &["read-only", "read-write", "none"];

/// Sandbox-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Max virtual memory in MB (0 = no limit)
    pub max_memory_mb: u64,

    /// Max processes
    pub max_processes: u32,

    /// Max CPU time in seconds (0 = no limit)
    pub max_cpu_seconds: u64,

    /// Max file size in MB (0 = no limit)
    pub max_file_size_mb: u64,

    /// macOS: sandbox user name
    pub sandbox_user: String,

    /// Cache access mode: "read-only", "read-write", "none"
    pub cache_mode: String,

    /// Additional read-only paths
    pub passthrough_paths: Vec<String>,

    /// Additional read-write paths
    pub writable_paths: Vec<String>,

    /// Dotfiles to copy into sandbox HOME
    pub dotfiles: Vec<String>,

    /// Allow mounting sensitive paths (overrides block list)
    pub allow_sensitive: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: 4096,
            max_processes: 256,
            max_cpu_seconds: 0,
            max_file_size_mb: 0,
            sandbox_user: "mino-sandbox".to_string(),
            cache_mode: "read-only".to_string(),
            passthrough_paths: vec![],
            writable_paths: vec![],
            dotfiles: vec![],
            allow_sensitive: false,
        }
    }
}

/// Validate that a path is not in the sensitive paths blocklist.
/// Returns error if `allow_sensitive` is false and path matches.
pub fn validate_path_not_sensitive(
    path: &Path,
    home_dir: &Path,
    allow_sensitive: bool,
) -> MinoResult<()> {
    if allow_sensitive {
        return Ok(());
    }

    for sensitive in SENSITIVE_PATHS {
        let sensitive_path = home_dir.join(sensitive);
        if path == sensitive_path || path.starts_with(&sensitive_path) {
            return Err(MinoError::User(format!(
                "Path '{}' is a sensitive credential store and is blocked by default. \
                 Set allow_sensitive = true in [sandbox] config to override.",
                path.display()
            )));
        }
    }

    Ok(())
}

/// Validate all paths in the sandbox config
pub fn validate_sandbox_paths(config: &SandboxConfig, home_dir: &Path) -> MinoResult<()> {
    for path_str in &config.passthrough_paths {
        let path = Path::new(path_str);
        validate_path_not_sensitive(path, home_dir, config.allow_sensitive)?;
    }

    for path_str in &config.writable_paths {
        let path = Path::new(path_str);
        validate_path_not_sensitive(path, home_dir, config.allow_sensitive)?;
    }

    validate_cache_mode(&config.cache_mode)?;

    Ok(())
}

/// Parse and validate cache mode
pub fn validate_cache_mode(mode: &str) -> MinoResult<()> {
    if VALID_CACHE_MODES.contains(&mode) {
        Ok(())
    } else {
        Err(MinoError::User(format!(
            "Invalid cache mode '{}'. Valid modes: {}",
            mode,
            VALID_CACHE_MODES.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_values() {
        let config = SandboxConfig::default();
        assert_eq!(config.max_memory_mb, 4096);
        assert_eq!(config.max_processes, 256);
        assert_eq!(config.max_cpu_seconds, 0);
        assert_eq!(config.max_file_size_mb, 0);
        assert_eq!(config.sandbox_user, "mino-sandbox");
        assert_eq!(config.cache_mode, "read-only");
        assert!(config.passthrough_paths.is_empty());
        assert!(config.writable_paths.is_empty());
        assert!(config.dotfiles.is_empty());
        assert!(!config.allow_sensitive);
    }

    #[test]
    fn serializes_and_deserializes() {
        let config = SandboxConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SandboxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_memory_mb, config.max_memory_mb);
        assert_eq!(parsed.sandbox_user, config.sandbox_user);
    }

    #[test]
    fn deserializes_from_toml() {
        let toml = r#"
            max_memory_mb = 8192
            sandbox_user = "custom-user"
        "#;
        let config: SandboxConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.max_memory_mb, 8192);
        assert_eq!(config.sandbox_user, "custom-user");
        // Defaults for unset fields
        assert_eq!(config.max_processes, 256);
    }

    #[test]
    fn empty_config_deserializes_to_defaults() {
        let config: SandboxConfig = toml::from_str("").unwrap();
        assert_eq!(config.max_memory_mb, 4096);
        assert_eq!(config.cache_mode, "read-only");
    }

    #[test]
    fn sensitive_path_ssh_blocked() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh");
        let err = validate_path_not_sensitive(&path, &home, false).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn sensitive_path_ssh_subdir_blocked() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh/config");
        let err = validate_path_not_sensitive(&path, &home, false).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn sensitive_path_aws_blocked() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.aws");
        assert!(validate_path_not_sensitive(&path, &home, false).is_err());
    }

    #[test]
    fn sensitive_path_gcloud_blocked() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.config/gcloud");
        assert!(validate_path_not_sensitive(&path, &home, false).is_err());
    }

    #[test]
    fn sensitive_path_kube_blocked() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.kube");
        assert!(validate_path_not_sensitive(&path, &home, false).is_err());
    }

    #[test]
    fn non_sensitive_path_allowed() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/projects");
        assert!(validate_path_not_sensitive(&path, &home, false).is_ok());
    }

    #[test]
    fn allow_sensitive_overrides_block() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh");
        assert!(validate_path_not_sensitive(&path, &home, true).is_ok());
    }

    #[test]
    fn cache_mode_valid_read_only() {
        assert!(validate_cache_mode("read-only").is_ok());
    }

    #[test]
    fn cache_mode_valid_read_write() {
        assert!(validate_cache_mode("read-write").is_ok());
    }

    #[test]
    fn cache_mode_valid_none() {
        assert!(validate_cache_mode("none").is_ok());
    }

    #[test]
    fn cache_mode_invalid() {
        let err = validate_cache_mode("foo").unwrap_err();
        assert!(err.to_string().contains("Invalid cache mode"));
        assert!(err.to_string().contains("foo"));
    }

    #[test]
    fn validate_sandbox_paths_blocks_sensitive() {
        let home = PathBuf::from("/home/user");
        let config = SandboxConfig {
            passthrough_paths: vec!["/home/user/.ssh".to_string()],
            ..Default::default()
        };
        assert!(validate_sandbox_paths(&config, &home).is_err());
    }

    #[test]
    fn validate_sandbox_paths_blocks_sensitive_writable() {
        let home = PathBuf::from("/home/user");
        let config = SandboxConfig {
            writable_paths: vec!["/home/user/.aws".to_string()],
            ..Default::default()
        };
        assert!(validate_sandbox_paths(&config, &home).is_err());
    }

    #[test]
    fn validate_sandbox_paths_allows_non_sensitive() {
        let home = PathBuf::from("/home/user");
        let config = SandboxConfig {
            passthrough_paths: vec!["/home/user/projects".to_string()],
            writable_paths: vec!["/home/user/tmp".to_string()],
            ..Default::default()
        };
        assert!(validate_sandbox_paths(&config, &home).is_ok());
    }

    #[test]
    fn validate_sandbox_paths_checks_cache_mode() {
        let home = PathBuf::from("/home/user");
        let config = SandboxConfig {
            cache_mode: "invalid".to_string(),
            ..Default::default()
        };
        assert!(validate_sandbox_paths(&config, &home).is_err());
    }
}
