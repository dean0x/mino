//! Configuration management for Mino

pub mod schema;

pub use schema::Config;

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
}

impl ConfigManager {
    /// Create a new config manager with default path
    pub fn new() -> Self {
        Self {
            config_path: Self::default_config_path(),
        }
    }

    /// Create a config manager with a custom path
    pub fn with_path(path: PathBuf) -> Self {
        Self { config_path: path }
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

        merged_value
            .try_into()
            .map_err(|e: toml::de::Error| MinoError::ConfigInvalid {
                path: local_path.unwrap_or(&self.config_path).to_path_buf(),
                reason: format!("{} (source: {})", e, config_source),
            })
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

        toml::from_str(&content).map_err(|e| MinoError::ConfigInvalid {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })
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

    /// Ensure the config directory exists
    async fn ensure_config_dir(&self) -> MinoResult<()> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| MinoError::ConfigDirCreate {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
        }
        Ok(())
    }

    /// Ensure all state directories exist
    pub async fn ensure_state_dirs() -> MinoResult<()> {
        let dirs = [
            Self::state_dir(),
            Self::sessions_dir(),
            Self::credentials_dir(),
        ];

        for dir in &dirs {
            fs::create_dir_all(dir).await.map_err(|e| {
                MinoError::io(format!("creating directory {}", dir.display()), e)
            })?;
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
            image = "fedora:41"
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
            image = "fedora:41"
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
            "fedora:41"
        );
        // Overlay credentials added
        let aws = table["credentials"].as_table().unwrap()["aws"]
            .as_table()
            .unwrap();
        assert_eq!(aws["enabled"].as_bool().unwrap(), true);
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
            image = "fedora:41"
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
            image = "fedora:41"
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
}
