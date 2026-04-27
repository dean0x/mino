//! Surgical TOML editing for the Mino config file.
//!
//! [`TomlEditor`] provides atomic, comment-preserving reads and writes for the
//! `[sandbox]` section of the config file.  It uses `toml_edit` internally so
//! that existing comments and key ordering are not disturbed.
//!
//! [`ConfigManager`] owns a `TomlEditor` instance and delegates all sandbox
//! key operations to it.
//!
//! [`ConfigManager`]: super::ConfigManager

use crate::error::{MinoError, MinoResult};
use std::path::PathBuf;
use tokio::fs;
use toml_edit::DocumentMut;
use tracing::debug;

/// Atomic, comment-preserving TOML editor for the `[sandbox]` config section.
pub(crate) struct TomlEditor {
    config_path: PathBuf,
}

impl TomlEditor {
    /// Create a new editor targeting the given config file path.
    pub(crate) fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }

    /// Ensure the parent directory of the config file exists.
    pub(crate) async fn ensure_config_dir(&self) -> MinoResult<()> {
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

    // ==========================================================================
    // Public read helpers
    // ==========================================================================

    /// Read `[sandbox].auto_passthrough_dirs` from the config file.
    ///
    /// Returns `Ok(None)` if the file, section, or key is absent.
    /// Returns `Err` if the key is present but not a string array.
    pub(crate) async fn read_sandbox_passthrough_dirs(&self) -> MinoResult<Option<Vec<String>>> {
        self.read_sandbox_string_array("auto_passthrough_dirs")
            .await
    }

    /// Read `[sandbox].auto_copy_dirs` from the config file.
    pub(crate) async fn read_sandbox_copy_dirs(&self) -> MinoResult<Option<Vec<String>>> {
        self.read_sandbox_string_array("auto_copy_dirs").await
    }

    /// Read `[sandbox].allow_sensitive_paths` from the config file.
    pub(crate) async fn read_sandbox_allow_sensitive_paths(&self) -> MinoResult<Option<Vec<String>>> {
        self.read_sandbox_string_array("allow_sensitive_paths")
            .await
    }

    // ==========================================================================
    // Public write helpers
    // ==========================================================================

    /// Write `[sandbox].auto_passthrough_dirs` to the config file.
    ///
    /// Preserves all other sections, keys, and comments.  Atomic write.
    pub(crate) async fn set_sandbox_passthrough_dirs(&self, dirs: &[String]) -> MinoResult<()> {
        self.write_toml_keys(&[("auto_passthrough_dirs", dirs)])
            .await
    }

    /// Write `[sandbox].auto_copy_dirs` to the config file.
    pub(crate) async fn set_sandbox_copy_dirs(&self, dirs: &[String]) -> MinoResult<()> {
        self.write_toml_keys(&[("auto_copy_dirs", dirs)]).await
    }

    /// Write `[sandbox].allow_sensitive_paths` to the config file.
    pub(crate) async fn set_sandbox_allow_sensitive_paths(&self, paths: &[String]) -> MinoResult<()> {
        self.write_toml_keys(&[("allow_sensitive_paths", paths)])
            .await
    }

    /// Apply one or more `[sandbox].<key> = [...]` mutations atomically.
    ///
    /// Reads the current config (or starts from an empty document), applies all
    /// mutations, then writes to a tempfile in the same directory and renames
    /// over the target.  This guarantees no partial writes are visible to readers.
    ///
    /// Sharing this helper across `set_sandbox_passthrough_dirs` and
    /// `set_sandbox_allow_sensitive_paths` ensures that writing both keys
    /// simultaneously (as the sensitive-path setup step requires) is a single
    /// atomic file operation.
    pub(crate) async fn write_toml_keys(&self, mutations: &[(&str, &[String])]) -> MinoResult<()> {
        self.ensure_config_dir().await?;

        // Load existing document or start empty
        let mut doc = if self.config_path.exists() {
            let content = fs::read_to_string(&self.config_path).await.map_err(|e| {
                MinoError::io(
                    format!("reading config from {}", self.config_path.display()),
                    e,
                )
            })?;
            if content.trim().is_empty() {
                DocumentMut::new()
            } else {
                content
                    .parse::<DocumentMut>()
                    .map_err(|e| MinoError::User(format!("parsing config: {}", e)))?
            }
        } else {
            DocumentMut::new()
        };

        // Ensure [sandbox] table exists
        if !doc.contains_table("sandbox") {
            doc["sandbox"] = toml_edit::table();
        }

        // Apply each mutation
        for (key, values) in mutations {
            let mut arr = toml_edit::Array::new();
            for v in *values {
                arr.push(v.as_str());
            }
            doc["sandbox"][key] = toml_edit::value(arr);
        }

        let new_content = doc.to_string();

        // Atomic write: tempfile in same dir → rename
        let parent = self
            .config_path
            .parent()
            .ok_or_else(|| MinoError::User("config path has no parent directory".to_string()))?;

        // Use a fixed-name temp file in the same directory for atomicity.
        // (tempfile::NamedTempFile would be ideal but would require a new dep.)
        let tmp_path = parent.join(format!(".mino-config-tmp-{}", std::process::id()));
        fs::write(&tmp_path, &new_content)
            .await
            .map_err(|e| MinoError::io("writing config tempfile", e))?;
        if let Err(e) = fs::rename(&tmp_path, &self.config_path).await {
            // Best-effort cleanup: remove the tempfile so it does not linger on disk.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(MinoError::io("renaming config tempfile", e));
        }

        debug!(
            "Config updated via toml_edit: {:?}",
            mutations.iter().map(|(k, _)| k).collect::<Vec<_>>()
        );
        Ok(())
    }

    // ==========================================================================
    // Private helpers
    // ==========================================================================

    /// Read a string array from `[sandbox].<key>`.
    async fn read_sandbox_string_array(&self, key: &str) -> MinoResult<Option<Vec<String>>> {
        if !self.config_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&self.config_path).await.map_err(|e| {
            MinoError::io(
                format!("reading config from {}", self.config_path.display()),
                e,
            )
        })?;

        let doc = content
            .parse::<DocumentMut>()
            .map_err(|e| MinoError::User(format!("parsing config: {}", e)))?;

        let sandbox = match doc.get("sandbox").and_then(|v| v.as_table()) {
            Some(t) => t,
            None => return Ok(None),
        };

        let item = match sandbox.get(key) {
            Some(i) => i,
            None => return Ok(None),
        };

        let arr = item
            .as_array()
            .ok_or_else(|| MinoError::User(format!("[sandbox].{} must be an array", key)))?;

        let values: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        Ok(Some(values))
    }
}
