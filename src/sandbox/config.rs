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
    ".docker",
    ".netrc",
];

/// Cache access mode for the sandbox
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheMode {
    #[default]
    ReadOnly,
    ReadWrite,
    None,
}

impl std::fmt::Display for CacheMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheMode::ReadOnly => write!(f, "read-only"),
            CacheMode::ReadWrite => write!(f, "read-write"),
            CacheMode::None => write!(f, "none"),
        }
    }
}

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

    /// Cache access mode
    pub cache_mode: CacheMode,

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
            sandbox_user: "_mino_agent".to_string(),
            cache_mode: CacheMode::ReadOnly,
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

/// Validate all paths and settings in the sandbox config
pub fn validate_sandbox_paths(config: &SandboxConfig, home_dir: &Path) -> MinoResult<()> {
    for path_str in config
        .passthrough_paths
        .iter()
        .chain(&config.writable_paths)
    {
        let path = Path::new(path_str);
        if !path.is_absolute() {
            return Err(MinoError::User(format!(
                "Sandbox path '{}' must be absolute (start with /)",
                path_str
            )));
        }
        validate_path_not_sensitive(path, home_dir, config.allow_sensitive)?;
    }

    validate_sandbox_user(&config.sandbox_user)?;

    Ok(())
}

/// Validate that the sandbox username contains only safe characters.
///
/// Prevents injection in pf rules and shell commands. Accepts alphanumeric,
/// underscore, and hyphen — matching macOS system username constraints.
pub fn validate_sandbox_user(username: &str) -> MinoResult<()> {
    if username.is_empty() {
        return Err(MinoError::User(
            "sandbox_user must not be empty".to_string(),
        ));
    }
    if !username
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(MinoError::User(format!(
            "sandbox_user '{}' contains invalid characters. \
             Only alphanumeric, underscore, and hyphen are allowed.",
            username
        )));
    }
    Ok(())
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
        assert_eq!(config.sandbox_user, "_mino_agent");
        assert_eq!(config.cache_mode, CacheMode::ReadOnly);
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
        assert_eq!(config.cache_mode, CacheMode::ReadOnly);
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
    fn all_sensitive_paths_blocked() {
        let home = PathBuf::from("/home/user");
        for suffix in SENSITIVE_PATHS {
            let path = home.join(suffix);
            assert!(
                validate_path_not_sensitive(&path, &home, false).is_err(),
                "expected {} to be blocked",
                path.display()
            );
        }
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
    fn cache_mode_serde_roundtrip() {
        let modes = [CacheMode::ReadOnly, CacheMode::ReadWrite, CacheMode::None];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: CacheMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn cache_mode_display() {
        assert_eq!(CacheMode::ReadOnly.to_string(), "read-only");
        assert_eq!(CacheMode::ReadWrite.to_string(), "read-write");
        assert_eq!(CacheMode::None.to_string(), "none");
    }

    #[test]
    fn cache_mode_deserializes_from_toml() {
        let toml_str = r#"cache_mode = "read-write""#;
        let config: SandboxConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.cache_mode, CacheMode::ReadWrite);
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
    fn validate_sandbox_paths_accepts_valid_cache_modes() {
        let home = PathBuf::from("/home/user");
        for mode in [CacheMode::ReadOnly, CacheMode::ReadWrite, CacheMode::None] {
            let config = SandboxConfig {
                cache_mode: mode,
                ..Default::default()
            };
            assert!(validate_sandbox_paths(&config, &home).is_ok());
        }
    }

    #[test]
    fn validate_sandbox_paths_rejects_relative_passthrough() {
        let home = PathBuf::from("/home/user");
        let config = SandboxConfig {
            passthrough_paths: vec!["../etc/shadow".to_string()],
            ..Default::default()
        };
        let err = validate_sandbox_paths(&config, &home).unwrap_err();
        assert!(err.to_string().contains("must be absolute"));
    }

    #[test]
    fn validate_sandbox_paths_rejects_relative_writable() {
        let home = PathBuf::from("/home/user");
        let config = SandboxConfig {
            writable_paths: vec!["relative/path".to_string()],
            ..Default::default()
        };
        let err = validate_sandbox_paths(&config, &home).unwrap_err();
        assert!(err.to_string().contains("must be absolute"));
    }

    // ---- sandbox_user validation tests ----

    #[test]
    fn validate_sandbox_user_accepts_standard_names() {
        assert!(validate_sandbox_user("_mino_agent").is_ok());
        assert!(validate_sandbox_user("sandbox-user").is_ok());
        assert!(validate_sandbox_user("user123").is_ok());
    }

    #[test]
    fn validate_sandbox_user_rejects_empty() {
        let err = validate_sandbox_user("").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn validate_sandbox_user_rejects_spaces() {
        let err = validate_sandbox_user("bad user").unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn validate_sandbox_user_rejects_newlines() {
        let err = validate_sandbox_user("user\ninjection").unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn validate_sandbox_user_rejects_special_chars() {
        assert!(validate_sandbox_user("user;drop").is_err());
        assert!(validate_sandbox_user("user{tcp}").is_err());
        assert!(validate_sandbox_user("user/path").is_err());
    }

    #[test]
    fn validate_sandbox_paths_rejects_invalid_sandbox_user() {
        let home = PathBuf::from("/home/user");
        let config = SandboxConfig {
            sandbox_user: "bad user".to_string(),
            ..Default::default()
        };
        let err = validate_sandbox_paths(&config, &home).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }
}
