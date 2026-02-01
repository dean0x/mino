//! Configuration management for Minotaur

pub mod schema;

pub use schema::Config;

use crate::error::{MinotaurError, MinotaurResult};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info};

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
            .join("minotaur")
            .join("config.toml")
    }

    /// Get the state directory path
    pub fn state_dir() -> PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("minotaur")
    }

    /// Get the sessions directory path
    pub fn sessions_dir() -> PathBuf {
        Self::state_dir().join("sessions")
    }

    /// Get the credentials cache directory path
    pub fn credentials_dir() -> PathBuf {
        Self::state_dir().join("credentials")
    }

    /// Get the audit log path
    pub fn audit_log_path() -> PathBuf {
        Self::state_dir().join("audit.log")
    }

    /// Load configuration, creating default if not exists
    pub async fn load(&self) -> MinotaurResult<Config> {
        if !self.config_path.exists() {
            debug!("Config file not found, using defaults");
            return Ok(Config::default());
        }

        self.load_from_file(&self.config_path).await
    }

    /// Load configuration from a specific file
    pub async fn load_from_file(&self, path: &Path) -> MinotaurResult<Config> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| MinotaurError::io(format!("reading config from {}", path.display()), e))?;

        toml::from_str(&content).map_err(|e| MinotaurError::ConfigInvalid {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })
    }

    /// Save configuration to file
    pub async fn save(&self, config: &Config) -> MinotaurResult<()> {
        self.ensure_config_dir().await?;

        let content = toml::to_string_pretty(config)?;
        fs::write(&self.config_path, content).await.map_err(|e| {
            MinotaurError::io(
                format!("writing config to {}", self.config_path.display()),
                e,
            )
        })?;

        info!("Configuration saved to {}", self.config_path.display());
        Ok(())
    }

    /// Ensure the config directory exists
    async fn ensure_config_dir(&self) -> MinotaurResult<()> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| MinotaurError::ConfigDirCreate {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
        }
        Ok(())
    }

    /// Ensure all state directories exist
    pub async fn ensure_state_dirs() -> MinotaurResult<()> {
        let dirs = [
            Self::state_dir(),
            Self::sessions_dir(),
            Self::credentials_dir(),
        ];

        for dir in &dirs {
            fs::create_dir_all(dir).await.map_err(|e| {
                MinotaurError::io(format!("creating directory {}", dir.display()), e)
            })?;
        }

        // Set restrictive permissions on credentials directory
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(Self::credentials_dir(), perms)
                .map_err(|e| MinotaurError::io("setting credentials dir permissions", e))?;
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
        assert_eq!(config.vm.name, "minotaur");
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
}
