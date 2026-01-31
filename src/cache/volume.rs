//! Cache volume state management
//!
//! Tracks cache volume state (miss, building, complete) and manages
//! the lifecycle of content-addressed cache volumes.

use crate::cache::lockfile::{Ecosystem, LockfileInfo};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Format bytes as human-readable size (e.g., "1.5 GB")
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Convert GB to bytes
pub fn gb_to_bytes(gb: u32) -> u64 {
    u64::from(gb) * 1024 * 1024 * 1024
}

/// Cache size status relative to configured limit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheSizeStatus {
    /// Under 80% of limit
    Ok,
    /// Between 80% and 100% of limit
    Warning,
    /// At or over the limit
    Exceeded,
}

impl CacheSizeStatus {
    /// Determine status based on current size and limit
    pub fn from_usage(current_bytes: u64, limit_bytes: u64) -> Self {
        if limit_bytes == 0 {
            return Self::Ok;
        }
        let percent = (current_bytes as f64 / limit_bytes as f64) * 100.0;
        if percent >= 100.0 {
            Self::Exceeded
        } else if percent >= 80.0 {
            Self::Warning
        } else {
            Self::Ok
        }
    }

    /// Get percentage of limit used
    pub fn percentage(current_bytes: u64, limit_bytes: u64) -> f64 {
        if limit_bytes == 0 {
            return 0.0;
        }
        (current_bytes as f64 / limit_bytes as f64) * 100.0
    }
}

/// Volume label keys used to track cache metadata
pub mod labels {
    /// Marks volume as a minotaur cache
    pub const MINOTAUR_CACHE: &str = "io.minotaur.cache";
    /// The ecosystem (npm, cargo, etc.)
    pub const ECOSYSTEM: &str = "io.minotaur.cache.ecosystem";
    /// The lockfile hash
    pub const HASH: &str = "io.minotaur.cache.hash";
    /// Cache state (building, complete)
    pub const STATE: &str = "io.minotaur.cache.state";
    /// Creation timestamp (RFC3339)
    pub const CREATED_AT: &str = "io.minotaur.cache.created_at";
}

/// State of a cache volume
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheState {
    /// No volume exists (will be created)
    Miss,
    /// Volume exists but session hasn't completed cleanly
    Building,
    /// Volume is finalized and immutable
    Complete,
}

impl CacheState {
    /// Whether this cache should be mounted read-only
    pub fn is_readonly(&self) -> bool {
        matches!(self, Self::Complete)
    }

    /// Parse from label value
    pub fn from_label(s: &str) -> Self {
        match s {
            "complete" => Self::Complete,
            "building" => Self::Building,
            _ => Self::Building,
        }
    }

    /// Convert to label value
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Miss => "building",
            Self::Building => "building",
            Self::Complete => "complete",
        }
    }
}

impl fmt::Display for CacheState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Miss => write!(f, "miss"),
            Self::Building => write!(f, "building"),
            Self::Complete => write!(f, "complete"),
        }
    }
}

/// Information about a cache volume
#[derive(Debug, Clone)]
pub struct CacheVolume {
    /// Volume name (minotaur-cache-{ecosystem}-{hash})
    pub name: String,
    /// The ecosystem this cache belongs to
    pub ecosystem: Ecosystem,
    /// Lockfile hash
    pub hash: String,
    /// Current state of the cache
    pub state: CacheState,
    /// When the volume was created
    pub created_at: DateTime<Utc>,
    /// Size in bytes (if known)
    pub size_bytes: Option<u64>,
}

impl CacheVolume {
    /// Create a new cache volume record
    pub fn new(ecosystem: Ecosystem, hash: String, state: CacheState) -> Self {
        Self {
            name: format!("minotaur-cache-{}-{}", ecosystem, hash),
            ecosystem,
            hash,
            state,
            created_at: Utc::now(),
            size_bytes: None,
        }
    }

    /// Create from lockfile info (for a new cache)
    pub fn from_lockfile(info: &LockfileInfo, state: CacheState) -> Self {
        Self::new(info.ecosystem, info.hash.clone(), state)
    }

    /// Generate labels for volume creation
    pub fn labels(&self) -> HashMap<String, String> {
        let mut labels = HashMap::new();
        labels.insert(labels::MINOTAUR_CACHE.to_string(), "true".to_string());
        labels.insert(labels::ECOSYSTEM.to_string(), self.ecosystem.to_string());
        labels.insert(labels::HASH.to_string(), self.hash.clone());
        labels.insert(labels::STATE.to_string(), self.state.as_label().to_string());
        labels.insert(
            labels::CREATED_AT.to_string(),
            self.created_at.to_rfc3339(),
        );
        labels
    }

    /// Parse ecosystem from string
    fn parse_ecosystem(s: &str) -> Option<Ecosystem> {
        match s {
            "npm" => Some(Ecosystem::Npm),
            "yarn" => Some(Ecosystem::Yarn),
            "pnpm" => Some(Ecosystem::Pnpm),
            "cargo" => Some(Ecosystem::Cargo),
            "pip" => Some(Ecosystem::Pip),
            "poetry" => Some(Ecosystem::Poetry),
            "go" => Some(Ecosystem::Go),
            _ => None,
        }
    }

    /// Try to parse from volume labels
    pub fn from_labels(name: &str, labels: &HashMap<String, String>) -> Option<Self> {
        // Must be a minotaur cache
        if labels.get(labels::MINOTAUR_CACHE) != Some(&"true".to_string()) {
            return None;
        }

        let ecosystem = labels
            .get(labels::ECOSYSTEM)
            .and_then(|s| Self::parse_ecosystem(s))?;

        let hash = labels.get(labels::HASH)?.clone();

        let state = labels
            .get(labels::STATE)
            .map(|s| CacheState::from_label(s))
            .unwrap_or(CacheState::Building);

        let created_at = labels
            .get(labels::CREATED_AT)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        Some(Self {
            name: name.to_string(),
            ecosystem,
            hash,
            state,
            created_at,
            size_bytes: None,
        })
    }

    /// Check if this volume is older than the given number of days
    pub fn is_older_than_days(&self, days: u32) -> bool {
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(days));
        self.created_at < cutoff
    }
}

/// Cache mount specification for container creation
#[derive(Debug, Clone)]
pub struct CacheMount {
    /// Volume name
    pub volume_name: String,
    /// Mount path inside container
    pub container_path: String,
    /// Whether to mount read-only
    pub readonly: bool,
    /// Ecosystem for setting env vars
    pub ecosystem: Ecosystem,
}

impl CacheMount {
    /// Generate the volume mount string for podman
    pub fn volume_arg(&self) -> String {
        let ro = if self.readonly { ":ro" } else { "" };
        format!("{}:{}{}", self.volume_name, self.container_path, ro)
    }
}

/// Determine cache mounts for a set of lockfiles
pub fn plan_cache_mounts(
    lockfiles: &[LockfileInfo],
    volume_states: &HashMap<String, CacheState>,
) -> Vec<CacheMount> {
    lockfiles
        .iter()
        .map(|info| {
            let volume_name = info.volume_name();
            let state = volume_states
                .get(&volume_name)
                .copied()
                .unwrap_or(CacheState::Miss);

            CacheMount {
                volume_name,
                container_path: "/cache".to_string(),
                readonly: state.is_readonly(),
                ecosystem: info.ecosystem,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cache_state_readonly() {
        assert!(!CacheState::Miss.is_readonly());
        assert!(!CacheState::Building.is_readonly());
        assert!(CacheState::Complete.is_readonly());
    }

    #[test]
    fn cache_state_label_roundtrip() {
        for state in [CacheState::Building, CacheState::Complete] {
            let label = state.as_label();
            let parsed = CacheState::from_label(label);
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn cache_volume_new() {
        let vol = CacheVolume::new(Ecosystem::Npm, "abc123def456".to_string(), CacheState::Building);

        assert_eq!(vol.name, "minotaur-cache-npm-abc123def456");
        assert_eq!(vol.ecosystem, Ecosystem::Npm);
        assert_eq!(vol.state, CacheState::Building);
    }

    #[test]
    fn cache_volume_from_lockfile() {
        let info = LockfileInfo {
            ecosystem: Ecosystem::Cargo,
            path: PathBuf::from("/test/Cargo.lock"),
            hash: "a1b2c3d4e5f6".to_string(),
        };

        let vol = CacheVolume::from_lockfile(&info, CacheState::Complete);

        assert_eq!(vol.name, "minotaur-cache-cargo-a1b2c3d4e5f6");
        assert_eq!(vol.ecosystem, Ecosystem::Cargo);
    }

    #[test]
    fn cache_volume_labels() {
        let vol = CacheVolume::new(Ecosystem::Npm, "abc123".to_string(), CacheState::Building);
        let labels = vol.labels();

        assert_eq!(labels.get(labels::MINOTAUR_CACHE), Some(&"true".to_string()));
        assert_eq!(labels.get(labels::ECOSYSTEM), Some(&"npm".to_string()));
        assert_eq!(labels.get(labels::HASH), Some(&"abc123".to_string()));
        assert_eq!(labels.get(labels::STATE), Some(&"building".to_string()));
    }

    #[test]
    fn cache_volume_from_labels() {
        let mut labels = HashMap::new();
        labels.insert(labels::MINOTAUR_CACHE.to_string(), "true".to_string());
        labels.insert(labels::ECOSYSTEM.to_string(), "cargo".to_string());
        labels.insert(labels::HASH.to_string(), "xyz789".to_string());
        labels.insert(labels::STATE.to_string(), "complete".to_string());
        labels.insert(
            labels::CREATED_AT.to_string(),
            "2024-01-15T10:00:00Z".to_string(),
        );

        let vol = CacheVolume::from_labels("minotaur-cache-cargo-xyz789", &labels).unwrap();

        assert_eq!(vol.ecosystem, Ecosystem::Cargo);
        assert_eq!(vol.hash, "xyz789");
        assert_eq!(vol.state, CacheState::Complete);
    }

    #[test]
    fn cache_mount_volume_arg() {
        let mount = CacheMount {
            volume_name: "minotaur-cache-npm-abc123".to_string(),
            container_path: "/cache".to_string(),
            readonly: true,
            ecosystem: Ecosystem::Npm,
        };

        assert_eq!(mount.volume_arg(), "minotaur-cache-npm-abc123:/cache:ro");

        let mount_rw = CacheMount {
            readonly: false,
            ..mount
        };
        assert_eq!(mount_rw.volume_arg(), "minotaur-cache-npm-abc123:/cache");
    }

    #[test]
    fn plan_cache_mounts_miss() {
        let lockfiles = vec![LockfileInfo {
            ecosystem: Ecosystem::Npm,
            path: PathBuf::from("/test/package-lock.json"),
            hash: "abc123def456".to_string(),
        }];

        let states = HashMap::new(); // No existing volumes

        let mounts = plan_cache_mounts(&lockfiles, &states);

        assert_eq!(mounts.len(), 1);
        assert!(!mounts[0].readonly); // Miss = read-write
    }

    #[test]
    fn plan_cache_mounts_complete() {
        let lockfiles = vec![LockfileInfo {
            ecosystem: Ecosystem::Npm,
            path: PathBuf::from("/test/package-lock.json"),
            hash: "abc123def456".to_string(),
        }];

        let mut states = HashMap::new();
        states.insert(
            "minotaur-cache-npm-abc123def456".to_string(),
            CacheState::Complete,
        );

        let mounts = plan_cache_mounts(&lockfiles, &states);

        assert_eq!(mounts.len(), 1);
        assert!(mounts[0].readonly); // Complete = read-only
    }
}
