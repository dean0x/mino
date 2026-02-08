//! Lockfile detection and hashing for content-addressed caching
//!
//! Detects package manager lockfiles and generates content-addressed cache keys
//! based on the lockfile contents. Same lockfile = same cache.

use crate::error::{MinoError, MinoResult};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Supported package ecosystems
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ecosystem {
    /// npm (package-lock.json, npm-shrinkwrap.json)
    Npm,
    /// Yarn (yarn.lock)
    Yarn,
    /// pnpm (pnpm-lock.yaml)
    Pnpm,
    /// Cargo/Rust (Cargo.lock)
    Cargo,
    /// pip/Python (requirements.txt, Pipfile.lock)
    Pip,
    /// Poetry/Python (poetry.lock)
    Poetry,
    /// Go modules (go.sum)
    Go,
}

impl Ecosystem {
    /// Get the cache directory name for this ecosystem
    pub fn cache_dir(&self) -> &'static str {
        match self {
            Self::Npm | Self::Yarn | Self::Pnpm => "npm",
            Self::Cargo => "cargo",
            Self::Pip | Self::Poetry => "pip",
            Self::Go => "go",
        }
    }

    /// Get the environment variables to set for this ecosystem's cache
    pub fn cache_env_vars(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            Self::Npm => vec![("npm_config_cache", "/cache/npm")],
            Self::Yarn => vec![
                ("YARN_CACHE_FOLDER", "/cache/yarn"),
                ("npm_config_cache", "/cache/npm"),
            ],
            Self::Pnpm => vec![
                ("PNPM_HOME", "/cache/pnpm"),
                ("npm_config_cache", "/cache/npm"),
            ],
            Self::Cargo => vec![
                ("CARGO_HOME", "/cache/cargo"),
                ("SCCACHE_DIR", "/cache/sccache"),
            ],
            Self::Pip => vec![("PIP_CACHE_DIR", "/cache/pip")],
            Self::Poetry => vec![
                ("POETRY_CACHE_DIR", "/cache/poetry"),
                ("PIP_CACHE_DIR", "/cache/pip"),
            ],
            Self::Go => vec![
                ("GOMODCACHE", "/cache/go/mod"),
                ("GOCACHE", "/cache/go/build"),
            ],
        }
    }

    /// Get the lockfile patterns for this ecosystem
    fn lockfile_patterns(&self) -> &'static [&'static str] {
        match self {
            Self::Npm => &["package-lock.json", "npm-shrinkwrap.json"],
            Self::Yarn => &["yarn.lock"],
            Self::Pnpm => &["pnpm-lock.yaml"],
            Self::Cargo => &["Cargo.lock"],
            Self::Pip => &["requirements.txt", "Pipfile.lock"],
            Self::Poetry => &["poetry.lock"],
            Self::Go => &["go.sum"],
        }
    }

    /// All ecosystems in detection priority order
    fn all() -> &'static [Self] {
        &[
            Self::Npm,
            Self::Yarn,
            Self::Pnpm,
            Self::Cargo,
            Self::Pip,
            Self::Poetry,
            Self::Go,
        ]
    }
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Npm => "npm",
            Self::Yarn => "yarn",
            Self::Pnpm => "pnpm",
            Self::Cargo => "cargo",
            Self::Pip => "pip",
            Self::Poetry => "poetry",
            Self::Go => "go",
        };
        write!(f, "{}", name)
    }
}

/// Information about a detected lockfile
#[derive(Debug, Clone)]
pub struct LockfileInfo {
    /// The ecosystem this lockfile belongs to
    pub ecosystem: Ecosystem,
    /// Path to the lockfile
    pub path: PathBuf,
    /// SHA256 hash of the lockfile contents (first 12 chars)
    pub hash: String,
}

impl LockfileInfo {
    /// Generate the cache volume name for this lockfile
    pub fn volume_name(&self) -> String {
        format!("mino-cache-{}-{}", self.ecosystem, self.hash)
    }
}

/// Hash a lockfile's contents using SHA256, returning first 12 hex chars
fn hash_file_contents(path: &Path) -> MinoResult<String> {
    let contents = fs::read(path).map_err(|e| MinoError::Io {
        context: format!("reading lockfile {}", path.display()),
        source: e,
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&contents);
    let result = hasher.finalize();

    // Take first 12 hex characters (6 bytes)
    let hash = hex::encode(&result[..6]);
    Ok(hash)
}

/// Detect all lockfiles in a project directory
///
/// Scans the project root for known lockfile patterns and returns
/// information about each detected lockfile, including a content hash.
pub fn detect_lockfiles(project_dir: &Path) -> MinoResult<Vec<LockfileInfo>> {
    let mut lockfiles = Vec::new();

    for ecosystem in Ecosystem::all() {
        for pattern in ecosystem.lockfile_patterns() {
            let lockfile_path = project_dir.join(pattern);
            if lockfile_path.exists() && lockfile_path.is_file() {
                debug!("Found {} lockfile: {}", ecosystem, lockfile_path.display());

                let hash = hash_file_contents(&lockfile_path)?;
                lockfiles.push(LockfileInfo {
                    ecosystem: *ecosystem,
                    path: lockfile_path,
                    hash,
                });

                // Only use first matching lockfile per ecosystem
                break;
            }
        }
    }

    debug!("Detected {} lockfiles", lockfiles.len());
    Ok(lockfiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn ecosystem_display() {
        assert_eq!(Ecosystem::Npm.to_string(), "npm");
        assert_eq!(Ecosystem::Cargo.to_string(), "cargo");
    }

    #[test]
    fn ecosystem_cache_dir() {
        assert_eq!(Ecosystem::Npm.cache_dir(), "npm");
        assert_eq!(Ecosystem::Yarn.cache_dir(), "npm");
        assert_eq!(Ecosystem::Cargo.cache_dir(), "cargo");
    }

    #[test]
    fn hash_deterministic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.lock");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(b"test content").unwrap();

        let hash1 = hash_file_contents(&path).unwrap();
        let hash2 = hash_file_contents(&path).unwrap();

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 12);
    }

    #[test]
    fn hash_different_content() {
        let dir = TempDir::new().unwrap();

        let path1 = dir.path().join("test1.lock");
        fs::write(&path1, b"content 1").unwrap();

        let path2 = dir.path().join("test2.lock");
        fs::write(&path2, b"content 2").unwrap();

        let hash1 = hash_file_contents(&path1).unwrap();
        let hash2 = hash_file_contents(&path2).unwrap();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn detect_npm_lockfile() {
        let dir = TempDir::new().unwrap();
        let lockfile = dir.path().join("package-lock.json");
        fs::write(&lockfile, r#"{"name": "test"}"#).unwrap();

        let lockfiles = detect_lockfiles(dir.path()).unwrap();

        assert_eq!(lockfiles.len(), 1);
        assert_eq!(lockfiles[0].ecosystem, Ecosystem::Npm);
        assert_eq!(lockfiles[0].path, lockfile);
    }

    #[test]
    fn detect_multiple_ecosystems() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        fs::write(dir.path().join("Cargo.lock"), "").unwrap();

        let lockfiles = detect_lockfiles(dir.path()).unwrap();

        assert_eq!(lockfiles.len(), 2);
        let ecosystems: Vec<_> = lockfiles.iter().map(|l| l.ecosystem).collect();
        assert!(ecosystems.contains(&Ecosystem::Npm));
        assert!(ecosystems.contains(&Ecosystem::Cargo));
    }

    #[test]
    fn lockfile_volume_name() {
        let info = LockfileInfo {
            ecosystem: Ecosystem::Npm,
            path: PathBuf::from("/test/package-lock.json"),
            hash: "a1b2c3d4e5f6".to_string(),
        };

        assert_eq!(info.volume_name(), "mino-cache-npm-a1b2c3d4e5f6");
    }

    #[test]
    fn detect_empty_dir() {
        let dir = TempDir::new().unwrap();
        let lockfiles = detect_lockfiles(dir.path()).unwrap();
        assert!(lockfiles.is_empty());
    }
}
