//! Configuration management for Mino

pub mod schema;
pub(crate) mod toml_editor;
pub mod trust;

pub use schema::Config;
pub(crate) use toml_editor::TomlEditor;

use crate::error::{MinoError, MinoResult};
use std::path::{Path, PathBuf};
use tokio::fs;
use toml::Value;
use tracing::debug;

/// Local config filename
const LOCAL_CONFIG_FILENAME: &str = ".mino.toml";

/// Configuration manager
pub struct ConfigManager {
    config_path: PathBuf,
    editor: TomlEditor,
}

impl ConfigManager {
    /// Create a new config manager with default path
    pub fn new() -> Self {
        let config_path = Self::default_config_path();
        let editor = TomlEditor::new(config_path.clone());
        Self {
            config_path,
            editor,
        }
    }

    /// Create a config manager with a custom path
    pub fn with_path(path: PathBuf) -> Self {
        let editor = TomlEditor::new(path.clone());
        Self {
            config_path: path,
            editor,
        }
    }

    /// Get the default config file path
    pub fn default_config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mino")
            .join("config.toml")
    }

    /// Get the state directory path
    pub fn state_dir() -> PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mino")
    }

    /// Get the sessions directory path
    pub fn sessions_dir() -> PathBuf {
        Self::state_dir().join("sessions")
    }

    /// Get the credentials cache directory path
    pub fn credentials_dir() -> PathBuf {
        Self::state_dir().join("credentials")
    }

    /// Get the cache state directory path (sidecar JSON files)
    pub fn cache_state_dir() -> PathBuf {
        Self::state_dir().join("cache")
    }

    /// Get the audit log file path
    pub fn audit_log_path() -> PathBuf {
        Self::state_dir().join("audit.log")
    }

    /// Search from `start_dir` upward for `.mino.toml`.
    /// Stops at filesystem root. Returns the path if found.
    pub fn find_local_config(start_dir: &Path) -> Option<PathBuf> {
        let mut current = start_dir.to_path_buf();
        loop {
            let candidate = current.join(LOCAL_CONFIG_FILENAME);
            if candidate.is_file() {
                return Some(candidate);
            }
            if !current.pop() {
                return None;
            }
        }
    }

    /// Deep-merge two TOML value trees. Keys in `overlay` override `base`.
    /// Tables are merged recursively; all other value types (including arrays)
    /// replace outright — arrays are **not** appended.
    pub fn merge_toml(base: Value, overlay: Value) -> Value {
        match (base, overlay) {
            (Value::Table(mut base_table), Value::Table(overlay_table)) => {
                for (key, overlay_val) in overlay_table {
                    let merged = match base_table.remove(&key) {
                        Some(base_val) => Self::merge_toml(base_val, overlay_val),
                        None => overlay_val,
                    };
                    base_table.insert(key, merged);
                }
                Value::Table(base_table)
            }
            // Non-table overlay replaces base entirely
            (_base, overlay) => overlay,
        }
    }

    /// Load merged configuration: global config merged with optional local config.
    ///
    /// Precedence: local `.mino.toml` > global `~/.config/mino/config.toml` > defaults.
    /// (CLI flags override the result separately at the call site.)
    pub async fn load_merged(&self, local_path: Option<&Path>) -> MinoResult<Config> {
        // Load global as raw TOML value (empty table if file missing)
        let global_value = if self.config_path.exists() {
            let content = fs::read_to_string(&self.config_path).await.map_err(|e| {
                MinoError::io(
                    format!("reading config from {}", self.config_path.display()),
                    e,
                )
            })?;
            content
                .parse::<Value>()
                .map_err(|e| MinoError::ConfigInvalid {
                    path: self.config_path.clone(),
                    reason: e.to_string(),
                })?
        } else {
            debug!("Global config not found, using defaults");
            Value::Table(toml::map::Map::new())
        };

        // Merge local on top if present
        let merged_value = match local_path {
            Some(path) => {
                let content = fs::read_to_string(path).await.map_err(|e| {
                    MinoError::io(format!("reading local config from {}", path.display()), e)
                })?;
                let local_value =
                    content
                        .parse::<Value>()
                        .map_err(|e| MinoError::ConfigInvalid {
                            path: path.to_path_buf(),
                            reason: e.to_string(),
                        })?;
                debug!("Merging local config from {} over global", path.display());
                Self::merge_toml(global_value, local_value)
            }
            None => global_value,
        };

        // Deserialize merged tree into Config (serde defaults fill gaps)
        let config_source = match local_path {
            Some(lp) => format!(
                "merged config [global: {}, local: {}]",
                self.config_path.display(),
                lp.display()
            ),
            None => self.config_path.display().to_string(),
        };

        let config: Config =
            merged_value
                .try_into()
                .map_err(|e: toml::de::Error| MinoError::ConfigInvalid {
                    path: local_path.unwrap_or(&self.config_path).to_path_buf(),
                    reason: format!("{} (source: {})", e, config_source),
                })?;

        // Validate sandbox config: reject overlapping auto_passthrough_dirs / auto_copy_dirs.
        // This mirrors `load_from_file`. Without it, the main CLI path (which uses
        // `load_merged`) would accept overlapping entries and fail at runtime when
        // `prepare_dotfiles` stages collide on the same staging-directory entry.
        config
            .sandbox
            .validate()
            .map_err(|e| MinoError::ConfigInvalid {
                path: local_path.unwrap_or(&self.config_path).to_path_buf(),
                reason: e.to_string(),
            })?;

        Ok(config)
    }

    /// Load configuration, creating default if not exists
    pub async fn load(&self) -> MinoResult<Config> {
        if !self.config_path.exists() {
            debug!("Config file not found, using defaults");
            return Ok(Config::default());
        }

        self.load_from_file(&self.config_path).await
    }

    /// Load configuration from a specific file
    pub async fn load_from_file(&self, path: &Path) -> MinoResult<Config> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| MinoError::io(format!("reading config from {}", path.display()), e))?;

        let config: Config = toml::from_str(&content).map_err(|e| MinoError::ConfigInvalid {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        // Validate sandbox config: reject overlapping auto_passthrough_dirs / auto_copy_dirs.
        // This ensures prepare_dotfiles stages remain disjoint and can safely run in parallel.
        config
            .sandbox
            .validate()
            .map_err(|e| MinoError::ConfigInvalid {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

        Ok(config)
    }

    /// Save configuration to file
    pub async fn save(&self, config: &Config) -> MinoResult<()> {
        self.ensure_config_dir().await?;

        let content = toml::to_string_pretty(config)?;
        fs::write(&self.config_path, content).await.map_err(|e| {
            MinoError::io(
                format!("writing config to {}", self.config_path.display()),
                e,
            )
        })?;

        debug!("Configuration saved to {}", self.config_path.display());
        Ok(())
    }

    /// Ensure the config directory exists.
    ///
    /// Delegates to [`TomlEditor::ensure_config_dir`].
    async fn ensure_config_dir(&self) -> MinoResult<()> {
        self.editor.ensure_config_dir().await
    }

    // ==========================================================================
    // Surgical [sandbox] section edits via toml_edit (preserve comments/order)
    // Delegates to self.editor (TomlEditor).
    // ==========================================================================

    /// Read `[sandbox].auto_passthrough_dirs` from the config file.
    ///
    /// Returns `Ok(None)` if the file, section, or key is absent.
    /// Returns `Err` if the key is present but not a string array.
    pub async fn read_sandbox_passthrough_dirs(&self) -> MinoResult<Option<Vec<String>>> {
        self.editor.read_sandbox_passthrough_dirs().await
    }

    /// Write `[sandbox].auto_passthrough_dirs` to the config file.
    ///
    /// Preserves all other sections, keys, and comments. Atomic write.
    pub async fn set_sandbox_passthrough_dirs(&self, dirs: &[String]) -> MinoResult<()> {
        self.editor.set_sandbox_passthrough_dirs(dirs).await
    }

    /// Read `[sandbox].auto_copy_dirs` from the config file.
    pub async fn read_sandbox_copy_dirs(&self) -> MinoResult<Option<Vec<String>>> {
        self.editor.read_sandbox_copy_dirs().await
    }

    /// Write `[sandbox].auto_copy_dirs` to the config file.
    pub async fn set_sandbox_copy_dirs(&self, dirs: &[String]) -> MinoResult<()> {
        self.editor.set_sandbox_copy_dirs(dirs).await
    }

    /// Read `[sandbox].allow_sensitive_paths` from the config file.
    pub async fn read_sandbox_allow_sensitive_paths(&self) -> MinoResult<Option<Vec<String>>> {
        self.editor.read_sandbox_allow_sensitive_paths().await
    }

    /// Write `[sandbox].allow_sensitive_paths` to the config file.
    pub async fn set_sandbox_allow_sensitive_paths(&self, paths: &[String]) -> MinoResult<()> {
        self.editor.set_sandbox_allow_sensitive_paths(paths).await
    }

    /// Apply one or more `[sandbox].<key> = [...]` mutations atomically.
    ///
    /// Reads the current config (or starts from an empty document), applies all
    /// mutations, then writes to a tempfile in the same directory and renames
    /// over the target. This guarantees no partial writes are visible to readers.
    ///
    /// Sharing this helper across `set_sandbox_passthrough_dirs` and
    /// `set_sandbox_allow_sensitive_paths` ensures that writing both keys
    /// simultaneously (as the sensitive-path setup step requires) is a single
    /// atomic file operation.
    pub async fn write_toml_keys(&self, mutations: &[(&str, &[String])]) -> MinoResult<()> {
        self.editor.write_toml_keys(mutations).await
    }

    /// Ensure all state directories exist
    pub async fn ensure_state_dirs() -> MinoResult<()> {
        let dirs = [
            Self::state_dir(),
            Self::sessions_dir(),
            Self::credentials_dir(),
            Self::cache_state_dir(),
        ];

        for dir in &dirs {
            fs::create_dir_all(dir)
                .await
                .map_err(|e| MinoError::io(format!("creating directory {}", dir.display()), e))?;
        }

        // Set restrictive permissions on credentials directory
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(Self::credentials_dir(), perms)
                .map_err(|e| MinoError::io("setting credentials dir permissions", e))?;
        }

        Ok(())
    }

    /// Get the config file path
    pub fn path(&self) -> &Path {
        &self.config_path
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn load_default_when_missing() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("nonexistent.toml");
        let manager = ConfigManager::with_path(path);

        let config = manager.load().await.unwrap();
        assert_eq!(config.vm.name, "mino");
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let manager = ConfigManager::with_path(path);

        let mut config = Config::default();
        config.vm.name = "test-vm".to_string();

        manager.save(&config).await.unwrap();
        let loaded = manager.load().await.unwrap();

        assert_eq!(loaded.vm.name, "test-vm");
    }

    #[test]
    fn merge_toml_leaf_override() {
        let base: Value = toml::from_str(
            r#"
            [container]
            image = "fedora:43"
            network = "host"
            "#,
        )
        .unwrap();
        let overlay: Value = toml::from_str(
            r#"
            [container]
            image = "typescript"
            "#,
        )
        .unwrap();
        let merged = ConfigManager::merge_toml(base, overlay);
        let table = merged.as_table().unwrap();
        let container = table["container"].as_table().unwrap();
        assert_eq!(container["image"].as_str().unwrap(), "typescript");
        assert_eq!(container["network"].as_str().unwrap(), "host");
    }

    #[test]
    fn merge_toml_additive_tables() {
        let base: Value = toml::from_str(
            r#"
            [container]
            image = "fedora:43"
            "#,
        )
        .unwrap();
        let overlay: Value = toml::from_str(
            r#"
            [credentials.aws]
            enabled = true
            region = "us-west-2"
            "#,
        )
        .unwrap();
        let merged = ConfigManager::merge_toml(base, overlay);
        let table = merged.as_table().unwrap();
        // Base container preserved
        assert_eq!(
            table["container"].as_table().unwrap()["image"]
                .as_str()
                .unwrap(),
            "fedora:43"
        );
        // Overlay credentials added
        let aws = table["credentials"].as_table().unwrap()["aws"]
            .as_table()
            .unwrap();
        assert!(aws["enabled"].as_bool().unwrap());
        assert_eq!(aws["region"].as_str().unwrap(), "us-west-2");
    }

    #[test]
    fn merge_toml_nested_tables() {
        let base: Value = toml::from_str(
            r#"
            [credentials.aws]
            region = "us-east-1"
            session_duration_secs = 3600

            [credentials.gcp]
            project = "my-proj"
            "#,
        )
        .unwrap();
        let overlay: Value = toml::from_str(
            r#"
            [credentials.aws]
            region = "eu-west-1"
            "#,
        )
        .unwrap();
        let merged = ConfigManager::merge_toml(base, overlay);
        let creds = merged.as_table().unwrap()["credentials"]
            .as_table()
            .unwrap();
        let aws = creds["aws"].as_table().unwrap();
        // Overridden
        assert_eq!(aws["region"].as_str().unwrap(), "eu-west-1");
        // Preserved from base
        assert_eq!(aws["session_duration_secs"].as_integer().unwrap(), 3600);
        // Sibling table preserved
        let gcp = creds["gcp"].as_table().unwrap();
        assert_eq!(gcp["project"].as_str().unwrap(), "my-proj");
    }

    #[test]
    fn merge_toml_empty_overlay() {
        let base: Value = toml::from_str(
            r#"
            [container]
            image = "fedora:43"
            "#,
        )
        .unwrap();
        let overlay: Value = toml::from_str("").unwrap();
        let merged = ConfigManager::merge_toml(base.clone(), overlay);
        assert_eq!(merged, base);
    }

    #[test]
    fn merge_toml_array_replaces() {
        let base: Value = toml::from_str(
            r#"
            [container]
            volumes = ["/shared:/shared"]
            "#,
        )
        .unwrap();
        let overlay: Value = toml::from_str(
            r#"
            [container]
            volumes = ["/project:/project"]
            "#,
        )
        .unwrap();
        let merged = ConfigManager::merge_toml(base, overlay);
        let volumes = merged["container"]["volumes"].as_array().unwrap();
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].as_str().unwrap(), "/project:/project");
    }

    #[test]
    fn find_local_config_in_cwd() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join(".mino.toml"), "# local config").unwrap();
        let found = ConfigManager::find_local_config(temp.path());
        assert_eq!(found.unwrap(), temp.path().join(".mino.toml"));
    }

    #[test]
    fn find_local_config_in_parent() {
        let temp = TempDir::new().unwrap();
        let child = temp.path().join("sub").join("deep");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(temp.path().join(".mino.toml"), "# parent config").unwrap();
        let found = ConfigManager::find_local_config(&child);
        assert_eq!(found.unwrap(), temp.path().join(".mino.toml"));
    }

    #[test]
    fn find_local_config_missing() {
        let temp = TempDir::new().unwrap();
        let found = ConfigManager::find_local_config(temp.path());
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn load_merged_combines_global_and_local() {
        let temp = TempDir::new().unwrap();

        // Write global config
        let global_path = temp.path().join("global.toml");
        std::fs::write(
            &global_path,
            r#"
            [container]
            image = "fedora:43"
            network = "host"

            [session]
            shell = "/bin/bash"
            "#,
        )
        .unwrap();

        // Write local config
        let local_path = temp.path().join(".mino.toml");
        std::fs::write(
            &local_path,
            r#"
            [container]
            image = "typescript"

            [credentials.aws]
            enabled = true
            region = "us-west-2"
            "#,
        )
        .unwrap();

        let manager = ConfigManager::with_path(global_path);
        let config = manager.load_merged(Some(&local_path)).await.unwrap();

        // Local overrides global
        assert_eq!(config.container.image, "typescript");
        // Global preserved where local is silent
        assert_eq!(config.container.network, "host");
        assert_eq!(config.session.shell, "/bin/bash");
        // Local adds new section
        assert!(config.credentials.aws.enabled);
        assert_eq!(config.credentials.aws.region.as_deref(), Some("us-west-2"));
    }

    #[tokio::test]
    async fn load_merged_no_local() {
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("global.toml");
        std::fs::write(
            &global_path,
            r#"
            [container]
            image = "custom:latest"
            "#,
        )
        .unwrap();

        let manager = ConfigManager::with_path(global_path);
        let config = manager.load_merged(None).await.unwrap();
        assert_eq!(config.container.image, "custom:latest");
    }

    #[tokio::test]
    async fn load_merged_no_global() {
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("nonexistent.toml");
        let local_path = temp.path().join(".mino.toml");
        std::fs::write(
            &local_path,
            r#"
            [container]
            image = "typescript"
            "#,
        )
        .unwrap();

        let manager = ConfigManager::with_path(global_path);
        let config = manager.load_merged(Some(&local_path)).await.unwrap();
        assert_eq!(config.container.image, "typescript");
        // Defaults fill in the rest
        assert_eq!(config.vm.name, "mino");
    }

    #[tokio::test]
    async fn load_merged_rejects_overlapping_sandbox_dirs() {
        // Regression: load_merged() is the primary config-load entry point
        // (used by main.rs). It must call SandboxConfig::validate() so that
        // overlapping auto_passthrough_dirs / auto_copy_dirs are rejected at
        // load time — not silently accepted and caught at runtime by
        // prepare_dotfiles.
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("global.toml");
        std::fs::write(
            &global_path,
            r#"
            [sandbox]
            auto_passthrough_dirs = [".claude"]
            auto_copy_dirs = [".claude"]
            "#,
        )
        .unwrap();

        let manager = ConfigManager::with_path(global_path);
        let err = manager.load_merged(None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(".claude"),
            "expected conflict message to name '.claude', got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn load_merged_rejects_default_dotfile_collision() {
        // Collision against a name in DEFAULT_DOTFILES (.gitconfig) must also
        // be rejected on the merged-load path.
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("global.toml");
        std::fs::write(
            &global_path,
            r#"
            [sandbox]
            auto_passthrough_dirs = [".gitconfig"]
            "#,
        )
        .unwrap();

        let manager = ConfigManager::with_path(global_path);
        let err = manager.load_merged(None).await.unwrap_err();
        assert!(err.to_string().contains(".gitconfig"));
    }

    // ---- toml_edit config helpers ----

    #[tokio::test]
    async fn set_sandbox_passthrough_dirs_creates_file_when_missing() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let manager = ConfigManager::with_path(path.clone());

        manager
            .set_sandbox_passthrough_dirs(&[".cargo".to_string(), ".nvm".to_string()])
            .await
            .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("auto_passthrough_dirs"));
        assert!(content.contains(".cargo"));
        assert!(content.contains(".nvm"));
    }

    #[tokio::test]
    async fn set_sandbox_passthrough_dirs_preserves_other_sections() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        // Pre-write config with [container] and [credentials.aws] sections
        std::fs::write(
            &path,
            r#"[container]
image = "fedora:43"
network = "host"

[credentials.aws]
enabled = true
region = "us-west-2"
"#,
        )
        .unwrap();

        let manager = ConfigManager::with_path(path.clone());
        manager
            .set_sandbox_passthrough_dirs(&[".cargo".to_string()])
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        // Pre-existing sections preserved
        assert!(content.contains("[container]"));
        assert!(content.contains("fedora:43"));
        assert!(content.contains("us-west-2"));
        // New key present
        assert!(content.contains("auto_passthrough_dirs"));
        assert!(content.contains(".cargo"));
    }

    #[tokio::test]
    async fn set_sandbox_passthrough_dirs_preserves_comments() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        // Pre-write config with a comment
        std::fs::write(
            &path,
            "# mino configuration file\n\n[container]\nimage = \"fedora:43\"\n",
        )
        .unwrap();

        let manager = ConfigManager::with_path(path.clone());
        manager
            .set_sandbox_passthrough_dirs(&[".cargo".to_string()])
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        // Comment must survive
        assert!(
            content.contains("# mino configuration file"),
            "comment should be preserved, got: {}",
            content
        );
    }

    #[tokio::test]
    async fn set_sandbox_passthrough_dirs_overwrites_existing_key() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        std::fs::write(
            &path,
            "[sandbox]\nauto_passthrough_dirs = [\".oh-my-zsh\"]\n",
        )
        .unwrap();

        let manager = ConfigManager::with_path(path.clone());
        manager
            .set_sandbox_passthrough_dirs(&[".cargo".to_string()])
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(".cargo"));
        assert!(
            !content.contains(".oh-my-zsh"),
            "old value should be replaced"
        );
    }

    #[tokio::test]
    async fn set_sandbox_passthrough_dirs_creates_sandbox_section_when_missing() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        std::fs::write(&path, "[container]\nimage = \"fedora:43\"\n").unwrap();

        let manager = ConfigManager::with_path(path.clone());
        manager
            .set_sandbox_passthrough_dirs(&[".nvm".to_string()])
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[sandbox]") || content.contains("sandbox.auto_passthrough_dirs"));
        assert!(content.contains(".nvm"));
    }

    #[tokio::test]
    async fn read_sandbox_passthrough_dirs_missing_file_returns_none() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("nonexistent.toml");
        let manager = ConfigManager::with_path(path);

        let result = manager.read_sandbox_passthrough_dirs().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_sandbox_passthrough_dirs_missing_key_returns_none() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[sandbox]\ncache_mode = \"read-write\"\n").unwrap();

        let manager = ConfigManager::with_path(path);
        let result = manager.read_sandbox_passthrough_dirs().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_sandbox_passthrough_dirs_present_empty_returns_some_empty() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[sandbox]\nauto_passthrough_dirs = []\n").unwrap();

        let manager = ConfigManager::with_path(path);
        let result = manager.read_sandbox_passthrough_dirs().await.unwrap();
        assert_eq!(result, Some(vec![]));
    }

    #[tokio::test]
    async fn read_sandbox_passthrough_dirs_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let manager = ConfigManager::with_path(path);

        let dirs = vec![".cargo".to_string(), ".nvm".to_string()];
        manager.set_sandbox_passthrough_dirs(&dirs).await.unwrap();

        let result = manager.read_sandbox_passthrough_dirs().await.unwrap();
        assert_eq!(result, Some(dirs));
    }

    #[tokio::test]
    async fn set_sandbox_copy_dirs_creates_and_reads_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let manager = ConfigManager::with_path(path);

        manager
            .set_sandbox_copy_dirs(&[".claude".to_string()])
            .await
            .unwrap();

        let result = manager.read_sandbox_copy_dirs().await.unwrap();
        assert_eq!(result, Some(vec![".claude".to_string()]));
    }

    #[tokio::test]
    async fn set_sandbox_copy_dirs_preserves_passthrough_dirs_when_set_second() {
        // Critical non-clobber: set passthrough first, then set copy dirs,
        // assert passthrough is still present.
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let manager = ConfigManager::with_path(path);

        manager
            .set_sandbox_passthrough_dirs(&[".cargo".to_string()])
            .await
            .unwrap();
        manager
            .set_sandbox_copy_dirs(&[".claude".to_string()])
            .await
            .unwrap();

        let passthrough = manager.read_sandbox_passthrough_dirs().await.unwrap();
        assert_eq!(passthrough, Some(vec![".cargo".to_string()]));
        let copy = manager.read_sandbox_copy_dirs().await.unwrap();
        assert_eq!(copy, Some(vec![".claude".to_string()]));
    }

    #[tokio::test]
    async fn set_sandbox_allow_sensitive_paths_basic_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let manager = ConfigManager::with_path(path);

        manager
            .set_sandbox_allow_sensitive_paths(&[".config/gh".to_string()])
            .await
            .unwrap();

        let result = manager.read_sandbox_allow_sensitive_paths().await.unwrap();
        assert_eq!(result, Some(vec![".config/gh".to_string()]));
    }

    #[tokio::test]
    async fn set_both_passthrough_and_allow_sensitive_in_one_call() {
        // Exercises the shared write_toml_keys path with two mutations in one atomic write.
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let manager = ConfigManager::with_path(path.clone());

        let passthrough = vec![".config/gh".to_string()];
        let allowlist = vec![".config/gh".to_string()];

        manager
            .write_toml_keys(&[
                ("auto_passthrough_dirs", &passthrough),
                ("allow_sensitive_paths", &allowlist),
            ])
            .await
            .unwrap();

        // Both keys present in the final file
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("auto_passthrough_dirs"));
        assert!(content.contains("allow_sensitive_paths"));
        assert_eq!(
            manager.read_sandbox_passthrough_dirs().await.unwrap(),
            Some(passthrough)
        );
        assert_eq!(
            manager.read_sandbox_allow_sensitive_paths().await.unwrap(),
            Some(allowlist)
        );
    }

    #[tokio::test]
    async fn set_sandbox_passthrough_dirs_handles_preexisting_trailing_newlines_and_whitespace() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        // File with trailing whitespace and extra newlines
        std::fs::write(&path, "[container]\nimage = \"test\"\n\n\n  \n").unwrap();

        let manager = ConfigManager::with_path(path.clone());
        manager
            .set_sandbox_passthrough_dirs(&[".cargo".to_string()])
            .await
            .unwrap();

        let result = manager.read_sandbox_passthrough_dirs().await.unwrap();
        assert_eq!(result, Some(vec![".cargo".to_string()]));
    }

    #[tokio::test]
    async fn set_sandbox_passthrough_dirs_with_utf8_content() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        // Pre-write a UTF-8 comment
        std::fs::write(&path, "# café ☕\n\n[container]\nimage = \"test\"\n").unwrap();

        let manager = ConfigManager::with_path(path.clone());
        manager
            .set_sandbox_passthrough_dirs(&[".cargo".to_string()])
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("# café ☕"),
            "UTF-8 comment should be preserved"
        );
        assert!(content.contains(".cargo"));
    }
}
