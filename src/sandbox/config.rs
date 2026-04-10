//! Sandbox configuration types
//!
//! Defines configuration for native sandbox mode including resource limits,
//! path validation, and security-sensitive path blocking.

use crate::config::schema::ContainerConfig;
use crate::error::{MinoError, MinoResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Default sandbox user name on macOS.
///
/// Used as fallback when session records lack a `sandbox_user` field
/// (e.g., sessions created before the field was added).
pub const DEFAULT_SANDBOX_USER: &str = "_mino_agent";

/// Paths that are ALWAYS blocked from passthrough (credential stores)
///
/// Exposed as `pub(crate)` so the setup detection function can filter candidates
/// against this list without duplicating the constant.
pub(crate) const SENSITIVE_PATHS: &[&str] = &[
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

    /// Opt-in allowlist for specific sensitive paths.
    ///
    /// Paths listed here are permitted even when `allow_sensitive = false`,
    /// provided they also appear in `SENSITIVE_PATHS`. This is a narrow
    /// escape hatch for dirs like `.config/gh`, `.docker`, `.kube` that
    /// hold credentials but are commonly needed by AI coding agents.
    ///
    /// Example: `allow_sensitive_paths = [".config/gh", ".docker"]`
    pub allow_sensitive_paths: Vec<String>,

    /// Host directories to auto-mount read-only on sandbox startup (opt-in).
    ///
    /// Example: to re-enable the previous defaults, set:
    /// `auto_passthrough_dirs = [".oh-my-zsh", ".nvm", ".zsh"]`
    pub auto_passthrough_dirs: Vec<String>,

    /// Host directories to copy (not mount) into the sandbox HOME (opt-in).
    ///
    /// Directories are copied so the agent gets a mutable sandbox-local version.
    /// For `.claude`, an allowlist-based copy is used to exclude large state dirs.
    ///
    /// Example: to re-enable the previous default, set:
    /// `auto_copy_dirs = [".claude"]`
    pub auto_copy_dirs: Vec<String>,

    /// Network mode for native sandbox (falls back to [container] network if None)
    pub network: Option<String>,

    /// Network allow rules for native sandbox (falls back to [container] if None)
    pub network_allow: Option<Vec<String>>,

    /// Network preset for native sandbox (falls back to [container] if None)
    pub network_preset: Option<String>,

    /// Environment variables for native sandbox (falls back to [container] if None)
    pub env: Option<HashMap<String, String>>,

    /// Host environment keys to inherit into the sandbox.
    ///
    /// When `None` (unset in config), the default list is used:
    /// `["ANTHROPIC_API_KEY", "LANG", "LC_ALL", "TZ", "TERM"]`.
    ///
    /// Set to an explicit list to override (use `[]` to disable all passthrough).
    /// Add other AI provider keys here (e.g., `"OPENAI_API_KEY"`, `"GROQ_API_KEY"`)
    /// without requiring a code change.
    pub env_passthrough: Option<Vec<String>>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: 4096,
            max_processes: 256,
            max_cpu_seconds: 0,
            max_file_size_mb: 0,
            sandbox_user: DEFAULT_SANDBOX_USER.to_string(),
            cache_mode: CacheMode::ReadOnly,
            passthrough_paths: vec![],
            writable_paths: vec![],
            dotfiles: vec![],
            allow_sensitive: false,
            allow_sensitive_paths: vec![],
            auto_passthrough_dirs: vec![],
            auto_copy_dirs: vec![],
            network: None,
            network_allow: None,
            network_preset: None,
            env: None,
            env_passthrough: None,
        }
    }
}

/// Default host environment keys inherited into the sandbox.
///
/// Users may override this list via `sandbox.env_passthrough` in config.
/// Locale vars (`LANG`, `LC_ALL`, `TZ`, `TERM`) keep the sandbox locale-consistent
/// with the host. `ANTHROPIC_API_KEY` is included so the agent can authenticate
/// without requiring explicit env var injection via `sandbox.env`.
///
/// Add other provider keys (e.g., `OPENAI_API_KEY`) by setting `env_passthrough`
/// in `[sandbox]` config rather than adding them here.
pub const DEFAULT_ENV_PASSTHROUGH: &[&str] = &["ANTHROPIC_API_KEY", "LANG", "LC_ALL", "TZ", "TERM"];

impl SandboxConfig {
    /// Validate that `auto_passthrough_dirs` and `auto_copy_dirs` do not overlap
    /// with the default dotfile list or each other, and that sensitive paths are
    /// gated by either `allow_sensitive = true` or an explicit `allow_sensitive_paths`
    /// entry.
    ///
    /// ## Conflict semantics (ancestor-based)
    ///
    /// Two path entries *conflict* if one is equal to or an ancestor of the other
    /// along path-component boundaries:
    ///
    /// - `.config` vs `.config/git/ignore` → **conflict** (ancestor)
    /// - `.config/gh` vs `.config/git/ignore` → **no conflict** (siblings under `.config`)
    /// - `.conf` vs `.config/x` → **no conflict** (different first component)
    ///
    /// Siblings are safe because all staging stages only CREATE entries; they never
    /// remove existing ones. `tokio::fs::create_dir_all` is idempotent, so two
    /// stages that both create `.config/` as a parent directory do not race.
    ///
    /// ## Sensitive paths
    ///
    /// Entries in `SENSITIVE_PATHS` are rejected unless:
    /// - `allow_sensitive = true` (nuclear override), **or**
    /// - the entry appears in `allow_sensitive_paths` (narrow per-dir opt-in)
    ///
    /// # Errors
    /// Returns the first conflicting or disallowed entry with a diagnostic message.
    pub fn validate(&self) -> MinoResult<()> {
        use crate::sandbox::dotfiles::DEFAULT_DOTFILES;

        // Validate sensitive paths gate
        for name in self
            .auto_passthrough_dirs
            .iter()
            .chain(&self.auto_copy_dirs)
        {
            if SENSITIVE_PATHS.contains(&name.as_str()) {
                // Permitted if nuclear flag is set OR name is in the narrow allowlist
                if !self.allow_sensitive && !self.allow_sensitive_paths.iter().any(|a| a == name) {
                    return Err(MinoError::User(format!(
                        "auto_passthrough_dirs/auto_copy_dirs entry '{}' is a sensitive \
                         credential path. Add it to allow_sensitive_paths to opt in, \
                         or set allow_sensitive = true to bypass all checks.",
                        name
                    )));
                }
            }
        }

        // Validate allow_sensitive_paths entries are actually in SENSITIVE_PATHS.
        // Unknown entries are warned about (non-fatal) to future-proof against
        // new SENSITIVE_PATHS additions. We log a debug message only; rejecting
        // them would break configs written today when SENSITIVE_PATHS grows tomorrow.
        // (Implementation note: no runtime logging here — this is a pure validate fn;
        //  the config loader may emit a warning at the call site if desired.)

        // Check both lists for conflicts with DEFAULT_DOTFILES
        let dir_list_entries = [
            ("auto_passthrough_dirs", &self.auto_passthrough_dirs),
            ("auto_copy_dirs", &self.auto_copy_dirs),
        ];
        for (list_name, entries) in &dir_list_entries {
            for name in *entries {
                for default in DEFAULT_DOTFILES {
                    if path_conflicts(name, default) {
                        return Err(MinoError::User(format!(
                            "{} entry '{}' conflicts with a default dotfile. \
                             Remove it from {} or from the dotfiles list.",
                            list_name, name, list_name
                        )));
                    }
                }
            }
        }

        // Check for conflicts within auto_passthrough_dirs (nested entries)
        for (i, a) in self.auto_passthrough_dirs.iter().enumerate() {
            for b in self.auto_passthrough_dirs.iter().skip(i + 1) {
                if path_conflicts(a, b) {
                    return Err(MinoError::User(format!(
                        "auto_passthrough_dirs entries '{}' and '{}' conflict (one is an \
                         ancestor of the other).",
                        a, b
                    )));
                }
            }
        }

        // Check for conflicts between auto_passthrough_dirs and auto_copy_dirs
        for a in &self.auto_passthrough_dirs {
            for b in &self.auto_copy_dirs {
                if path_conflicts(a, b) {
                    return Err(MinoError::User(format!(
                        "auto_copy_dirs entry '{}' conflicts with auto_passthrough_dirs \
                         entry '{}'. A directory can only appear in one list.",
                        b, a
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Check whether two path strings conflict under ancestor-based semantics.
///
/// Two paths conflict iff one is equal to or an ancestor of the other along
/// path-component boundaries. Siblings under a shared parent do NOT conflict.
///
/// Examples:
/// - `.config` vs `.config/git/ignore` → true (`.config` is ancestor of the other)
/// - `.config/gh` vs `.config/git/ignore` → false (siblings)
/// - `.conf` vs `.config/x` → false (different first component)
/// - `.cargo` vs `.cargo` → true (equal)
pub(crate) fn path_conflicts(a: &str, b: &str) -> bool {
    let a_parts: Vec<&str> = a.split('/').filter(|s| !s.is_empty()).collect();
    let b_parts: Vec<&str> = b.split('/').filter(|s| !s.is_empty()).collect();
    if a_parts.is_empty() || b_parts.is_empty() {
        return false;
    }
    let min_len = a_parts.len().min(b_parts.len());
    a_parts[..min_len] == b_parts[..min_len]
}

/// Validate that a path is not in the sensitive paths blocklist.
///
/// Returns error if `allow_sensitive` is false and path matches a sensitive
/// path that is NOT in `allow_sensitive_paths`.
///
/// # Allowlist semantics
///
/// `allow_sensitive_paths` is a narrow opt-in: only entries that exactly match
/// a canonical name in [`SENSITIVE_PATHS`] (e.g., `.docker`, `.config/gh`) can
/// unblock their corresponding sensitive directory. Entries that do not match
/// any `SENSITIVE_PATHS` name are ignored. This prevents catastrophic bypass
/// values like `""`, `"."`, or `".."` from granting access to unrelated
/// sensitive directories.
pub fn validate_path_not_sensitive(
    path: &Path,
    home_dir: &Path,
    allow_sensitive: bool,
    allow_sensitive_paths: &[String],
) -> MinoResult<()> {
    if allow_sensitive {
        return Ok(());
    }

    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    for sensitive in SENSITIVE_PATHS {
        let sensitive_path = home_dir.join(sensitive);
        let resolved_sensitive =
            std::fs::canonicalize(&sensitive_path).unwrap_or_else(|_| sensitive_path.clone());
        if resolved == resolved_sensitive || resolved.starts_with(&resolved_sensitive) {
            // Narrow allowlist: the per-directory opt-in only unblocks its
            // exact SENSITIVE_PATHS name. Entries that do not match the
            // current sensitive name are ignored — so `""`, `"."`, or `".."`
            // cannot bypass unrelated sensitive directories.
            if allow_sensitive_paths.iter().any(|a| a == *sensitive) {
                return Ok(());
            }
            return Err(MinoError::User(format!(
                "Path '{}' resolves to sensitive credential store '{}' and is blocked by default. \
                 Add it to allow_sensitive_paths or set allow_sensitive = true in [sandbox] config.",
                path.display(),
                resolved_sensitive.display()
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
        validate_path_not_sensitive(
            path,
            home_dir,
            config.allow_sensitive,
            &config.allow_sensitive_paths,
        )?;
    }

    validate_sandbox_user(&config.sandbox_user)?;

    Ok(())
}

/// Resolve effective network config for native sandbox.
///
/// Sandbox-specific values take precedence over container config.
/// When a sandbox field is `None`, the corresponding container field is used.
pub fn resolve_sandbox_network<'a>(
    sandbox: &'a SandboxConfig,
    container: &'a ContainerConfig,
) -> (&'a str, &'a [String], Option<&'a str>) {
    let network = sandbox.network.as_deref().unwrap_or(&container.network);
    let allow = sandbox
        .network_allow
        .as_deref()
        .unwrap_or(&container.network_allow);
    let preset = sandbox
        .network_preset
        .as_deref()
        .or(container.network_preset.as_deref());
    (network, allow, preset)
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
    if username.len() > 32 {
        return Err(MinoError::User(
            "sandbox_user exceeds 32 characters".to_string(),
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
        assert!(config.allow_sensitive_paths.is_empty());
        assert!(config.auto_passthrough_dirs.is_empty());
        assert!(config.auto_copy_dirs.is_empty());
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
        let err = validate_path_not_sensitive(&path, &home, false, &[]).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn sensitive_path_ssh_subdir_blocked() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh/config");
        let err = validate_path_not_sensitive(&path, &home, false, &[]).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn all_sensitive_paths_blocked() {
        let home = PathBuf::from("/home/user");
        for suffix in SENSITIVE_PATHS {
            let path = home.join(suffix);
            assert!(
                validate_path_not_sensitive(&path, &home, false, &[]).is_err(),
                "expected {} to be blocked",
                path.display()
            );
        }
    }

    #[test]
    fn non_sensitive_path_allowed() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/projects");
        assert!(validate_path_not_sensitive(&path, &home, false, &[]).is_ok());
    }

    #[test]
    fn allow_sensitive_overrides_block() {
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh");
        assert!(validate_path_not_sensitive(&path, &home, true, &[]).is_ok());
    }

    #[test]
    fn allow_sensitive_paths_permits_exact_match() {
        // Narrow allowlist: `.docker` in allow_sensitive_paths permits .docker
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.docker");
        let allow = vec![".docker".to_string()];
        assert!(validate_path_not_sensitive(&path, &home, false, &allow).is_ok());
    }

    #[test]
    fn allow_sensitive_paths_permits_subdirectory_of_allowed() {
        // Subpaths of an allowed sensitive dir are also permitted
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.docker/config.json");
        let allow = vec![".docker".to_string()];
        assert!(validate_path_not_sensitive(&path, &home, false, &allow).is_ok());
    }

    #[test]
    fn allow_sensitive_paths_does_not_permit_other_sensitive_dirs() {
        // Allowing .docker must NOT unblock .ssh
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh");
        let allow = vec![".docker".to_string()];
        let err = validate_path_not_sensitive(&path, &home, false, &allow).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn allow_sensitive_paths_empty_string_does_not_bypass() {
        // Security-critical: `""` must NOT bypass the sensitive-path blocklist.
        // Regression for HIGH severity bypass where `home.join("")` == home and
        // starts_with(home) matched any path under home.
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh");
        let allow = vec!["".to_string()];
        let err = validate_path_not_sensitive(&path, &home, false, &allow).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn allow_sensitive_paths_dot_does_not_bypass() {
        // Security: `.` must not bypass — `home.join(".") == home` canonically.
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.aws");
        let allow = vec![".".to_string()];
        let err = validate_path_not_sensitive(&path, &home, false, &allow).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn allow_sensitive_paths_dotdot_does_not_bypass() {
        // Security: `..` must not bypass.
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.aws");
        let allow = vec!["..".to_string()];
        let err = validate_path_not_sensitive(&path, &home, false, &allow).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn allow_sensitive_paths_unknown_entry_ignored() {
        // Entries that don't match any SENSITIVE_PATHS name are silently ignored
        // (not errors) so that future additions to SENSITIVE_PATHS don't break
        // existing configs. Unknown entries also must not bypass any check.
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.ssh");
        let allow = vec![".some-unknown-credential-store".to_string()];
        let err = validate_path_not_sensitive(&path, &home, false, &allow).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[test]
    fn allow_sensitive_paths_multi_segment_permits_config_gh() {
        // `.config/gh` is in SENSITIVE_PATHS; allow it via allow_sensitive_paths
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.config/gh");
        let allow = vec![".config/gh".to_string()];
        assert!(validate_path_not_sensitive(&path, &home, false, &allow).is_ok());
    }

    #[test]
    fn allow_sensitive_paths_config_gh_does_not_permit_gcloud() {
        // Two SENSITIVE_PATHS entries under .config are independent
        let home = PathBuf::from("/home/user");
        let path = PathBuf::from("/home/user/.config/gcloud");
        let allow = vec![".config/gh".to_string()];
        let err = validate_path_not_sensitive(&path, &home, false, &allow).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
    }

    #[cfg(unix)]
    #[test]
    fn sensitive_path_symlink_detected() {
        use std::os::unix::fs::symlink;
        let tmp = std::env::temp_dir().join("mino-test-symlink-check");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let home = tmp.join("home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();

        // Create a symlink that points to .ssh
        let link_path = tmp.join("sneaky-link");
        symlink(home.join(".ssh"), &link_path).unwrap();

        let err = validate_path_not_sensitive(&link_path, &home, false, &[]).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn non_existent_path_uses_literal_match() {
        // When canonicalize fails (path doesn't exist), falls back to literal comparison
        let home = PathBuf::from("/nonexistent-home-dir");
        let path = PathBuf::from("/nonexistent-home-dir/.ssh");
        let err = validate_path_not_sensitive(&path, &home, false, &[]).unwrap_err();
        assert!(err.to_string().contains("sensitive credential store"));
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
    fn auto_dirs_default_empty() {
        let config = SandboxConfig::default();
        assert!(config.auto_passthrough_dirs.is_empty());
        assert!(config.auto_copy_dirs.is_empty());
    }

    #[test]
    fn auto_dirs_deserialize_from_toml() {
        let toml = r#"
            auto_passthrough_dirs = [".oh-my-zsh", ".nvm"]
            auto_copy_dirs = [".claude"]
        "#;
        let config: SandboxConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.auto_passthrough_dirs, vec![".oh-my-zsh", ".nvm"]);
        assert_eq!(config.auto_copy_dirs, vec![".claude"]);
    }

    #[test]
    fn auto_dirs_empty_toml_defaults_empty() {
        let config: SandboxConfig = toml::from_str("").unwrap();
        assert!(config.auto_passthrough_dirs.is_empty());
        assert!(config.auto_copy_dirs.is_empty());
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
    fn validate_sandbox_user_rejects_too_long() {
        let long_name = "a".repeat(33);
        let err = validate_sandbox_user(&long_name).unwrap_err();
        assert!(err.to_string().contains("exceeds 32 characters"));
    }

    #[test]
    fn validate_sandbox_user_accepts_exactly_32() {
        let name = "a".repeat(32);
        assert!(validate_sandbox_user(&name).is_ok());
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

    // ---- resolve_sandbox_network tests ----

    #[test]
    fn resolve_sandbox_network_prefers_sandbox_when_present() {
        let sandbox = SandboxConfig {
            network: Some("none".to_string()),
            network_allow: Some(vec!["example.com:443".to_string()]),
            network_preset: Some("dev".to_string()),
            ..Default::default()
        };
        let container = crate::config::schema::ContainerConfig::default();
        let (network, allow, preset) = resolve_sandbox_network(&sandbox, &container);
        assert_eq!(network, "none");
        assert_eq!(allow, &["example.com:443".to_string()]);
        assert_eq!(preset, Some("dev"));
    }

    #[test]
    fn resolve_sandbox_network_falls_back_to_container() {
        let sandbox = SandboxConfig::default();
        let container = crate::config::schema::ContainerConfig {
            network: "host".to_string(),
            network_allow: vec!["api.example.com:443".to_string()],
            network_preset: Some("registries".to_string()),
            ..Default::default()
        };
        let (network, allow, preset) = resolve_sandbox_network(&sandbox, &container);
        assert_eq!(network, "host");
        assert_eq!(allow, &["api.example.com:443".to_string()]);
        assert_eq!(preset, Some("registries"));
    }

    #[test]
    fn resolve_sandbox_network_mixed_override() {
        let sandbox = SandboxConfig {
            network: Some("none".to_string()),
            // network_allow and network_preset are None -> fall back
            ..Default::default()
        };
        let container = crate::config::schema::ContainerConfig {
            network_allow: vec!["fallback.com:80".to_string()],
            network_preset: Some("dev".to_string()),
            ..Default::default()
        };
        let (network, allow, preset) = resolve_sandbox_network(&sandbox, &container);
        assert_eq!(network, "none");
        assert_eq!(allow, &["fallback.com:80".to_string()]);
        assert_eq!(preset, Some("dev"));
    }

    #[test]
    fn sandbox_config_new_fields_default_to_none() {
        // Invariant: the fallback-to-[container] behavior in resolve_sandbox_network
        // and build_sandbox_env relies on these fields being Option::None by default.
        // If any of these fields changes to a non-Option type (or a Some default),
        // the fallback silently breaks and users who set values only under [container]
        // will get unexpected behavior. This test guards that invariant.
        let config = SandboxConfig::default();
        assert!(config.network.is_none());
        assert!(config.network_allow.is_none());
        assert!(config.network_preset.is_none());
        assert!(config.env.is_none());
        assert!(config.env_passthrough.is_none());
    }

    #[test]
    fn sandbox_config_deserializes_network_fields() {
        let toml = r#"
            network = "none"
            network_allow = ["example.com:443"]
            network_preset = "dev"
        "#;
        let config: SandboxConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.network.as_deref(), Some("none"));
        assert_eq!(
            config.network_allow.as_ref().unwrap(),
            &["example.com:443".to_string()]
        );
        assert_eq!(config.network_preset.as_deref(), Some("dev"));
    }

    #[test]
    fn sandbox_config_deserializes_env_field() {
        let toml = r#"
            [env]
            MY_VAR = "my_value"
            ANOTHER = "val2"
        "#;
        let config: SandboxConfig = toml::from_str(toml).unwrap();
        let env = config.env.unwrap();
        assert_eq!(env.get("MY_VAR").unwrap(), "my_value");
        assert_eq!(env.get("ANOTHER").unwrap(), "val2");
    }

    // ---- SandboxConfig::validate collision tests ----

    #[test]
    fn validate_accepts_disjoint_names() {
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".oh-my-zsh".to_string(), ".nvm".to_string()],
            auto_copy_dirs: vec![".claude".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_passthrough_overlap_with_defaults() {
        // ".gitconfig" is in DEFAULT_DOTFILES
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".gitconfig".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(".gitconfig"));
        assert!(err.to_string().contains("auto_passthrough_dirs"));
    }

    #[test]
    fn validate_rejects_copy_overlap_with_defaults() {
        // ".zshrc" is in DEFAULT_DOTFILES
        let config = SandboxConfig {
            auto_copy_dirs: vec![".zshrc".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(".zshrc"));
        assert!(err.to_string().contains("auto_copy_dirs"));
    }

    #[test]
    fn validate_rejects_passthrough_and_copy_sharing_names() {
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".claude".to_string()],
            auto_copy_dirs: vec![".claude".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(".claude"));
    }

    #[test]
    fn validate_accepts_empty_dirs() {
        let config = SandboxConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_top_segment_collision_with_default_dotfile() {
        // `.config/git/ignore` is in DEFAULT_DOTFILES; top segment is `.config`.
        // Putting `.config` in auto_copy_dirs would race against
        // write_sanitized_dotfiles writing into `staging/.config/git/ignore`.
        let config = SandboxConfig {
            auto_copy_dirs: vec![".config".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(".config"));
    }

    #[test]
    fn validate_rejects_passthrough_top_segment_collision_with_default_dotfile() {
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".config".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(".config"));
    }

    // Under the new ancestor-based semantics, .myapp/a and .myapp/b are siblings
    // under .myapp — they don't conflict with each other. Only an ancestor vs
    // descendant relationship triggers a conflict.
    #[test]
    fn validate_accepts_siblings_under_shared_parent() {
        // .config/gh and .config/bar are siblings — no path-component conflict.
        // .config/gh IS sensitive, so we use allow_sensitive_paths to permit it.
        // .config/bar is not sensitive, so it needs no special treatment.
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".config/gh".to_string()],
            auto_copy_dirs: vec![".config/bar".to_string()],
            allow_sensitive_paths: vec![".config/gh".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_accepts_non_overlapping_nested_entries() {
        // Different top segments should be allowed even if the paths look similar.
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".localshare".to_string()],
            auto_copy_dirs: vec![".localcache".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    // ---- new multi-segment and allow_sensitive_paths tests ----

    #[test]
    fn validate_accepts_disjoint_single_segment_entries() {
        // .cargo, .nvm, .oh-my-zsh are all single-segment and disjoint
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![
                ".cargo".to_string(),
                ".nvm".to_string(),
                ".oh-my-zsh".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_nested_ancestor_inside_passthrough() {
        // .foo and .foo/bar conflict: .foo is ancestor of .foo/bar
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".foo".to_string(), ".foo/bar".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(".foo"));
    }

    #[test]
    fn validate_rejects_prefix_string_not_path_boundary() {
        // .conf and .config/x must NOT conflict — different first path component
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".conf".to_string(), ".config-extra".to_string()],
            ..Default::default()
        };
        // These are disjoint single-segment entries — should be fine
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_allows_sensitive_path_when_in_allowlist() {
        // .config/gh is in SENSITIVE_PATHS; it's allowed when in allow_sensitive_paths
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".config/gh".to_string()],
            allow_sensitive_paths: vec![".config/gh".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_sensitive_path_without_allowlist() {
        // .config/gh without allow_sensitive_paths is rejected
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".config/gh".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(".config/gh"));
        assert!(err.to_string().contains("allow_sensitive_paths"));
    }

    #[test]
    fn validate_with_allow_sensitive_flag_still_works() {
        // Legacy allow_sensitive = true must still allow sensitive paths
        let config = SandboxConfig {
            auto_passthrough_dirs: vec![".ssh".to_string()],
            allow_sensitive: true,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    // ---- path_conflicts unit tests ----

    #[test]
    fn path_conflicts_equal_paths() {
        assert!(path_conflicts(".cargo", ".cargo"));
    }

    #[test]
    fn path_conflicts_ancestor_vs_descendant() {
        assert!(path_conflicts(".config", ".config/git/ignore"));
        assert!(path_conflicts(".config/git/ignore", ".config"));
    }

    #[test]
    fn path_conflicts_siblings_do_not_conflict() {
        assert!(!path_conflicts(".config/gh", ".config/git"));
        assert!(!path_conflicts(".config/gh", ".config/git/ignore"));
    }

    #[test]
    fn path_conflicts_different_first_component() {
        assert!(!path_conflicts(".conf", ".config/x"));
        assert!(!path_conflicts(".cargo", ".cabal"));
    }

    #[test]
    fn path_conflicts_empty_paths() {
        assert!(!path_conflicts("", ".cargo"));
        assert!(!path_conflicts(".cargo", ""));
    }
}
