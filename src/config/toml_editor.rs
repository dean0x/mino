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
    pub(crate) async fn read_sandbox_allow_sensitive_paths(
        &self,
    ) -> MinoResult<Option<Vec<String>>> {
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
    pub(crate) async fn set_sandbox_allow_sensitive_paths(
        &self,
        paths: &[String],
    ) -> MinoResult<()> {
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
    // Private helpers — see also #[cfg(test)] mod tests below
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---- write_toml_keys: empty-content branch ----

    #[tokio::test]
    async fn write_toml_keys_empty_file_is_treated_as_blank_document() {
        // The `content.trim().is_empty()` guard must produce a fresh DocumentMut
        // rather than forwarding the empty string to the TOML parser (which would
        // succeed but return an empty table — behaviorally equivalent, but the
        // guard exists to avoid any parser quirks with whitespace-only files).
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        // Write a file with only whitespace — triggers the empty-content branch.
        std::fs::write(&path, "   \n\n  \t  \n").unwrap();

        let editor = TomlEditor::new(path.clone());
        editor
            .write_toml_keys(&[("auto_passthrough_dirs", &[".cargo".to_string()])])
            .await
            .unwrap();

        let result = editor.read_sandbox_passthrough_dirs().await.unwrap();
        assert_eq!(result, Some(vec![".cargo".to_string()]));
    }

    // ---- write_toml_keys: rename-failure cleanup ----

    #[tokio::test]
    #[cfg(unix)]
    async fn write_toml_keys_rename_failure_removes_tempfile() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();

        // Create a read-only subdirectory for the config file.  `rename` into a
        // read-only directory fails with EACCES, exercising the cleanup branch.
        let ro_dir = temp.path().join("ro");
        std::fs::create_dir(&ro_dir).unwrap();

        // Write an initial config so the file "exists" in a writable sibling dir
        // that we can point the editor at; we make the *target directory* read-only
        // after creating the file to force rename to fail.
        let config_path = ro_dir.join("config.toml");
        std::fs::write(&config_path, "[container]\nimage = \"test\"\n").unwrap();

        // Make the directory read-only so rename into it fails.
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let editor = TomlEditor::new(config_path.clone());
        let result = editor
            .write_toml_keys(&[("auto_passthrough_dirs", &[".cargo".to_string()])])
            .await;

        // Restore permissions so TempDir cleanup can succeed.
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        // The write must fail (EACCES on rename).
        assert!(result.is_err(), "expected rename failure to propagate as Err");

        // The tempfile must not be left on disk after the cleanup branch runs.
        let leftover: Vec<_> = std::fs::read_dir(&ro_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".mino-config-tmp-")
            })
            .collect();
        assert!(
            leftover.is_empty(),
            "tempfile should be cleaned up on rename failure, found: {:?}",
            leftover
        );
    }

    // ---- write_toml_keys: multi-key atomicity ----

    #[tokio::test]
    async fn write_toml_keys_multiple_keys_in_one_call() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let editor = TomlEditor::new(path.clone());

        editor
            .write_toml_keys(&[
                ("auto_passthrough_dirs", &[".cargo".to_string()]),
                ("allow_sensitive_paths", &[".config/gh".to_string()]),
            ])
            .await
            .unwrap();

        let passthrough = editor.read_sandbox_passthrough_dirs().await.unwrap();
        assert_eq!(passthrough, Some(vec![".cargo".to_string()]));

        let sensitive = editor.read_sandbox_allow_sensitive_paths().await.unwrap();
        assert_eq!(sensitive, Some(vec![".config/gh".to_string()]));
    }

    // ---- read_sandbox_string_array: missing file ----

    #[tokio::test]
    async fn read_sandbox_passthrough_dirs_missing_file_returns_none() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("nonexistent.toml");
        let editor = TomlEditor::new(path);

        let result = editor.read_sandbox_passthrough_dirs().await.unwrap();
        assert!(result.is_none());
    }

    // ---- read_sandbox_string_array: key absent ----

    #[tokio::test]
    async fn read_sandbox_passthrough_dirs_absent_key_returns_none() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[sandbox]\ncache_mode = \"read-write\"\n").unwrap();

        let editor = TomlEditor::new(path);
        let result = editor.read_sandbox_passthrough_dirs().await.unwrap();
        assert!(result.is_none());
    }

    // ---- ensure_config_dir ----

    #[tokio::test]
    async fn ensure_config_dir_creates_nested_dirs() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("a").join("b").join("config.toml");
        let editor = TomlEditor::new(path.clone());

        editor.ensure_config_dir().await.unwrap();
        assert!(path.parent().unwrap().is_dir());
    }
}
