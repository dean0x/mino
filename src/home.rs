//! Per-project persistent home volume management
//!
//! Provides content-addressed home volumes keyed by project directory path.
//! Each project gets its own persistent `/home/developer` that survives
//! image rebuilds and cache clears.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

/// Volume label keys for home volume metadata
pub mod labels {
    /// Marks volume as a mino home volume
    pub const MINO_HOME: &str = "io.mino.home";
    /// Canonical project directory path
    pub const PROJECT: &str = "io.mino.home.project";
    /// Creation timestamp (RFC3339)
    pub const CREATED_AT: &str = "io.mino.home.created_at";
}

/// Information about a home volume
#[derive(Debug, Clone)]
pub struct HomeVolume {
    /// Volume name (mino-home-{hash12})
    pub name: String,
    /// Project directory path this volume is associated with
    pub project_path: String,
    /// When the volume was created
    pub created_at: DateTime<Utc>,
}

impl HomeVolume {
    /// Try to parse a HomeVolume from volume labels.
    pub fn from_labels(name: &str, volume_labels: &HashMap<String, String>) -> Option<Self> {
        if volume_labels.get(labels::MINO_HOME) != Some(&"true".to_string()) {
            return None;
        }

        let project_path = volume_labels.get(labels::PROJECT)?.clone();

        let created_at = volume_labels
            .get(labels::CREATED_AT)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        Some(Self {
            name: name.to_string(),
            project_path,
            created_at,
        })
    }

    /// Generate labels for volume creation.
    pub fn labels(project_dir: &Path) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert(labels::MINO_HOME.to_string(), "true".to_string());
        map.insert(
            labels::PROJECT.to_string(),
            project_dir.display().to_string(),
        );
        map.insert(labels::CREATED_AT.to_string(), Utc::now().to_rfc3339());
        map
    }
}

/// Compute the home volume name for a project directory.
///
/// Uses SHA256 of the canonicalized path, truncated to 12 hex chars.
pub fn home_volume_name(project_dir: &Path) -> String {
    let hash = hash_project_path(project_dir);
    format!("mino-home-{}", hash)
}

/// Hash a project path to a 12-char hex string.
fn hash_project_path(project_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project_dir.to_string_lossy().as_bytes());
    let hash = hex::encode(hasher.finalize());
    hash[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn hash_is_deterministic() {
        let path = PathBuf::from("/home/user/projects/my-app");
        let a = hash_project_path(&path);
        let b = hash_project_path(&path);
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
    }

    #[test]
    fn hash_is_unique_for_different_paths() {
        let a = hash_project_path(&PathBuf::from("/project/a"));
        let b = hash_project_path(&PathBuf::from("/project/b"));
        assert_ne!(a, b);
    }

    #[test]
    fn home_volume_name_format() {
        let name = home_volume_name(&PathBuf::from("/home/user/project"));
        assert!(name.starts_with("mino-home-"));
        assert_eq!(name.len(), "mino-home-".len() + 12);
    }

    #[test]
    fn from_labels_valid() {
        let mut labels = HashMap::new();
        labels.insert(labels::MINO_HOME.to_string(), "true".to_string());
        labels.insert(
            labels::PROJECT.to_string(),
            "/home/user/project".to_string(),
        );
        labels.insert(
            labels::CREATED_AT.to_string(),
            "2026-01-15T10:00:00Z".to_string(),
        );

        let vol = HomeVolume::from_labels("mino-home-abc123def456", &labels).unwrap();
        assert_eq!(vol.name, "mino-home-abc123def456");
        assert_eq!(vol.project_path, "/home/user/project");
    }

    #[test]
    fn from_labels_missing_marker() {
        let mut labels = HashMap::new();
        labels.insert(
            labels::PROJECT.to_string(),
            "/home/user/project".to_string(),
        );

        assert!(HomeVolume::from_labels("mino-home-abc123", &labels).is_none());
    }

    #[test]
    fn from_labels_missing_project() {
        let mut labels = HashMap::new();
        labels.insert(labels::MINO_HOME.to_string(), "true".to_string());

        assert!(HomeVolume::from_labels("mino-home-abc123", &labels).is_none());
    }

    #[test]
    fn labels_roundtrip() {
        let path = PathBuf::from("/home/user/project");
        let labels = HomeVolume::labels(&path);

        assert_eq!(labels.get(labels::MINO_HOME), Some(&"true".to_string()));
        assert_eq!(
            labels.get(labels::PROJECT),
            Some(&"/home/user/project".to_string())
        );
        assert!(labels.contains_key(labels::CREATED_AT));

        // Should be parseable back
        let vol = HomeVolume::from_labels("mino-home-test", &labels).unwrap();
        assert_eq!(vol.project_path, "/home/user/project");
    }
}
