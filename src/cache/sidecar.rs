//! Cache sidecar state persistence
//!
//! Per-volume JSON sidecar files that track cache state transitions.
//! Volume labels are immutable once created (Podman limitation), so
//! this module provides the authoritative state source for cache volumes.
//!
//! Sidecar files live at `~/.local/share/mino/cache/{volume_name}.json`.

use crate::cache::lockfile::Ecosystem;
use crate::cache::volume::CacheState;
use crate::config::ConfigManager;
use crate::error::{MinoError, MinoResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Persistent cache state tracked via sidecar JSON file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSidecar {
    /// Volume name (mino-cache-{ecosystem}-{hash})
    pub volume_name: String,
    /// The ecosystem this cache belongs to
    pub ecosystem: Ecosystem,
    /// Lockfile content hash
    pub hash: String,
    /// Current cache state (building or complete)
    pub state: CacheState,
    /// When the sidecar was created
    pub created_at: DateTime<Utc>,
    /// When the sidecar was last updated
    pub updated_at: DateTime<Utc>,
}

impl CacheSidecar {
    /// Create a new sidecar record with current timestamps
    pub fn new(volume_name: String, ecosystem: Ecosystem, hash: String, state: CacheState) -> Self {
        let now = Utc::now();
        Self {
            volume_name,
            ecosystem,
            hash,
            state,
            created_at: now,
            updated_at: now,
        }
    }

    /// Get the file path for a volume's sidecar
    pub fn file_path(volume_name: &str) -> PathBuf {
        ConfigManager::cache_state_dir().join(format!("{}.json", volume_name))
    }

    /// Get the file path for a volume's sidecar under a custom base directory
    #[cfg(test)]
    fn file_path_in(base_dir: &Path, volume_name: &str) -> PathBuf {
        base_dir.join(format!("{}.json", volume_name))
    }

    /// Save sidecar to disk, updating the `updated_at` timestamp
    pub async fn save(&mut self) -> MinoResult<()> {
        self.save_to(&Self::file_path(&self.volume_name)).await
    }

    /// Save sidecar to a specific path (for testability)
    async fn save_to(&mut self, path: &Path) -> MinoResult<()> {
        self.updated_at = Utc::now();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| MinoError::io("creating cache state directory", e))?;
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)
            .await
            .map_err(|e| MinoError::io(format!("writing cache sidecar {}", path.display()), e))?;

        Ok(())
    }

    /// Load a sidecar from disk. Returns None if the file does not exist.
    pub async fn load(volume_name: &str) -> MinoResult<Option<Self>> {
        Self::load_from(&Self::file_path(volume_name)).await
    }

    /// Load a sidecar from a specific path (for testability)
    async fn load_from(path: &Path) -> MinoResult<Option<Self>> {
        match fs::read_to_string(path).await {
            Ok(content) => {
                let sidecar: Self = serde_json::from_str(&content)?;
                Ok(Some(sidecar))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(MinoError::io(
                format!("reading cache sidecar {}", path.display()),
                e,
            )),
        }
    }

    /// Delete a sidecar file. Idempotent -- does not error if file is missing.
    pub async fn delete(volume_name: &str) -> MinoResult<()> {
        Self::delete_at(&Self::file_path(volume_name)).await
    }

    /// Delete a sidecar at a specific path (for testability)
    async fn delete_at(path: &Path) -> MinoResult<()> {
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(MinoError::io(
                format!("deleting cache sidecar {}", path.display()),
                e,
            )),
        }
    }

    /// List all sidecar files from the cache state directory
    pub async fn list_all() -> MinoResult<Vec<Self>> {
        Self::list_all_in(&ConfigManager::cache_state_dir()).await
    }

    /// List all sidecar files from a specific directory (for testability)
    async fn list_all_in(dir: &Path) -> MinoResult<Vec<Self>> {
        let mut entries = match fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(MinoError::io("reading cache state directory", e)),
        };

        let mut sidecars = Vec::new();

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| MinoError::io("reading cache state entry", e))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                if let Some(sidecar) = fs::read_to_string(&path)
                    .await
                    .ok()
                    .and_then(|content| serde_json::from_str::<Self>(&content).ok())
                {
                    sidecars.push(sidecar);
                }
            }
        }

        Ok(sidecars)
    }

    /// Transition this sidecar to Complete state and persist to disk
    pub async fn mark_complete(&mut self) -> MinoResult<()> {
        self.state = CacheState::Complete;
        self.save().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sidecar_new_has_timestamps() {
        let sidecar = CacheSidecar::new(
            "mino-cache-npm-abc123".to_string(),
            Ecosystem::Npm,
            "abc123".to_string(),
            CacheState::Building,
        );

        assert_eq!(sidecar.volume_name, "mino-cache-npm-abc123");
        assert_eq!(sidecar.state, CacheState::Building);
        assert_eq!(sidecar.created_at, sidecar.updated_at);
    }

    #[tokio::test]
    async fn sidecar_save_and_load_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = CacheSidecar::file_path_in(temp.path(), "mino-cache-cargo-def456");

        let mut sidecar = CacheSidecar::new(
            "mino-cache-cargo-def456".to_string(),
            Ecosystem::Cargo,
            "def456".to_string(),
            CacheState::Building,
        );

        sidecar.save_to(&path).await.unwrap();

        let loaded = CacheSidecar::load_from(&path)
            .await
            .unwrap()
            .expect("sidecar should exist");

        assert_eq!(loaded.volume_name, "mino-cache-cargo-def456");
        assert_eq!(loaded.ecosystem, Ecosystem::Cargo);
        assert_eq!(loaded.state, CacheState::Building);
    }

    #[tokio::test]
    async fn sidecar_mark_complete_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = CacheSidecar::file_path_in(temp.path(), "mino-cache-npm-aaa111");

        let mut sidecar = CacheSidecar::new(
            "mino-cache-npm-aaa111".to_string(),
            Ecosystem::Npm,
            "aaa111".to_string(),
            CacheState::Building,
        );

        sidecar.save_to(&path).await.unwrap();

        // Mark complete and re-save
        sidecar.state = CacheState::Complete;
        sidecar.save_to(&path).await.unwrap();

        let loaded = CacheSidecar::load_from(&path)
            .await
            .unwrap()
            .expect("sidecar should exist");

        assert_eq!(loaded.state, CacheState::Complete);
    }

    #[tokio::test]
    async fn sidecar_load_missing_returns_none() {
        let temp = TempDir::new().unwrap();
        let path = CacheSidecar::file_path_in(temp.path(), "nonexistent-volume");

        let result = CacheSidecar::load_from(&path).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn sidecar_delete_idempotent() {
        let temp = TempDir::new().unwrap();
        let path = CacheSidecar::file_path_in(temp.path(), "mino-cache-pip-bbb222");

        // Delete nonexistent -- should not error
        CacheSidecar::delete_at(&path).await.unwrap();

        // Create then delete
        let mut sidecar = CacheSidecar::new(
            "mino-cache-pip-bbb222".to_string(),
            Ecosystem::Pip,
            "bbb222".to_string(),
            CacheState::Building,
        );
        sidecar.save_to(&path).await.unwrap();

        CacheSidecar::delete_at(&path).await.unwrap();

        let loaded = CacheSidecar::load_from(&path).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn sidecar_list_all() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        let path1 = CacheSidecar::file_path_in(dir, "mino-cache-npm-111");
        let mut s1 = CacheSidecar::new(
            "mino-cache-npm-111".to_string(),
            Ecosystem::Npm,
            "111".to_string(),
            CacheState::Complete,
        );
        s1.save_to(&path1).await.unwrap();

        let path2 = CacheSidecar::file_path_in(dir, "mino-cache-cargo-222");
        let mut s2 = CacheSidecar::new(
            "mino-cache-cargo-222".to_string(),
            Ecosystem::Cargo,
            "222".to_string(),
            CacheState::Building,
        );
        s2.save_to(&path2).await.unwrap();

        let all = CacheSidecar::list_all_in(dir).await.unwrap();
        assert_eq!(all.len(), 2);

        let names: Vec<&str> = all.iter().map(|s| s.volume_name.as_str()).collect();
        assert!(names.contains(&"mino-cache-npm-111"));
        assert!(names.contains(&"mino-cache-cargo-222"));
    }

    #[tokio::test]
    async fn sidecar_list_all_empty_dir() {
        let temp = TempDir::new().unwrap();

        let all = CacheSidecar::list_all_in(temp.path()).await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn sidecar_list_all_nonexistent_dir() {
        let temp = TempDir::new().unwrap();
        let nonexistent = temp.path().join("does-not-exist");

        let all = CacheSidecar::list_all_in(&nonexistent).await.unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn sidecar_file_path_format() {
        let path = CacheSidecar::file_path("mino-cache-npm-abc123");
        assert!(path
            .to_string_lossy()
            .ends_with("mino-cache-npm-abc123.json"));
    }

    #[test]
    fn sidecar_serialize_deserialize() {
        let sidecar = CacheSidecar::new(
            "mino-cache-go-xyz789".to_string(),
            Ecosystem::Go,
            "xyz789".to_string(),
            CacheState::Complete,
        );

        let json = serde_json::to_string(&sidecar).unwrap();
        assert!(json.contains("\"go\""));
        assert!(json.contains("\"complete\""));

        let parsed: CacheSidecar = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ecosystem, Ecosystem::Go);
        assert_eq!(parsed.state, CacheState::Complete);
    }

    #[tokio::test]
    async fn sidecar_updated_at_changes_on_save() {
        let temp = TempDir::new().unwrap();
        let path = CacheSidecar::file_path_in(temp.path(), "mino-cache-npm-time");

        let mut sidecar = CacheSidecar::new(
            "mino-cache-npm-time".to_string(),
            Ecosystem::Npm,
            "time".to_string(),
            CacheState::Building,
        );

        let created_at = sidecar.created_at;
        sidecar.save_to(&path).await.unwrap();

        // updated_at should be >= created_at after save
        assert!(sidecar.updated_at >= created_at);
    }
}
