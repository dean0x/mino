//! Layer manifest parsing
//!
//! Each layer has a `layer.toml` manifest describing its metadata,
//! environment variables, and cache paths.

use crate::error::{MinotaurError, MinotaurResult};
use serde::Deserialize;
use std::path::Path;

/// Parsed layer manifest from layer.toml
#[derive(Debug, Clone, Deserialize)]
pub struct LayerManifest {
    /// Layer metadata
    pub layer: LayerMeta,

    /// Environment variables to set
    #[serde(default)]
    pub env: LayerEnv,

    /// Cache configuration
    #[serde(default)]
    pub cache: LayerCache,
}

/// Layer metadata section
#[derive(Debug, Clone, Deserialize)]
pub struct LayerMeta {
    /// Layer name (must match directory name)
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// Schema version for forward compatibility
    pub version: String,
}

/// Environment variables section
///
/// Flat keys are env vars (e.g., `CARGO_HOME = "/cache/cargo"`).
/// The `path_prepend` sub-table lists directories to prepend to PATH.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LayerEnv {
    /// Directories to prepend to PATH
    #[serde(default)]
    pub path_prepend: PathPrepend,

    /// Flat env vars — collected by flattening the TOML table
    /// and excluding known sub-tables (path_prepend)
    #[serde(flatten)]
    pub vars: std::collections::HashMap<String, toml::Value>,
}

/// PATH prepend configuration
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PathPrepend {
    /// Directories to prepend to PATH
    #[serde(default)]
    pub dirs: Vec<String>,
}

/// Cache configuration section
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LayerCache {
    /// Paths that should be persisted in cache volumes
    #[serde(default)]
    pub paths: Vec<String>,
}

impl LayerManifest {
    /// Parse a manifest from a TOML file on disk
    pub async fn from_file(path: &Path) -> MinotaurResult<Self> {
        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            MinotaurError::io(format!("reading layer manifest {}", path.display()), e)
        })?;
        Self::parse(&content)
    }

    /// Parse a manifest from a TOML string (for embedded built-in layers)
    pub fn parse(content: &str) -> MinotaurResult<Self> {
        toml::from_str(content).map_err(|e| MinotaurError::ConfigInvalid {
            path: "layer.toml".into(),
            reason: e.to_string(),
        })
    }

    /// Extract flat environment variables (excludes path_prepend)
    pub fn env_vars(&self) -> std::collections::HashMap<String, String> {
        self.env
            .vars
            .iter()
            .filter_map(|(k, v)| {
                // Only include string values, skip sub-tables
                v.as_str().map(|s| (k.clone(), s.to_string()))
            })
            .collect()
    }

    /// Get the full PATH prepend string (dirs joined with ":")
    pub fn path_prepend_str(&self) -> Option<String> {
        if self.env.path_prepend.dirs.is_empty() {
            None
        } else {
            Some(self.env.path_prepend.dirs.join(":"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST_MANIFEST: &str = r#"
[layer]
name = "rust"
description = "Rust stable toolchain + cargo tools"
version = "1"

[env]
CARGO_HOME = "/cache/cargo"
RUSTUP_HOME = "/opt/rustup"
RUSTC_WRAPPER = "sccache"
SCCACHE_DIR = "/cache/sccache"

[env.path_prepend]
dirs = ["/opt/cargo/bin"]

[cache]
paths = ["/cache/cargo", "/cache/sccache"]
"#;

    const TS_MANIFEST: &str = r#"
[layer]
name = "typescript"
description = "Node.js + pnpm + TypeScript toolchain"
version = "1"

[env]
PNPM_HOME = "/cache/pnpm"
npm_config_cache = "/cache/npm"
NODE_ENV = "development"

[env.path_prepend]
dirs = ["/cache/pnpm"]

[cache]
paths = ["/cache/pnpm", "/cache/npm"]
"#;

    #[test]
    fn parse_rust_manifest() {
        let manifest = LayerManifest::parse(RUST_MANIFEST).unwrap();
        assert_eq!(manifest.layer.name, "rust");
        assert_eq!(manifest.layer.version, "1");

        let vars = manifest.env_vars();
        assert_eq!(vars.get("CARGO_HOME").unwrap(), "/cache/cargo");
        assert_eq!(vars.get("RUSTUP_HOME").unwrap(), "/opt/rustup");
        assert_eq!(vars.get("RUSTC_WRAPPER").unwrap(), "sccache");
        assert_eq!(vars.get("SCCACHE_DIR").unwrap(), "/cache/sccache");

        assert_eq!(
            manifest.path_prepend_str(),
            Some("/opt/cargo/bin".to_string())
        );
        assert_eq!(manifest.cache.paths, vec!["/cache/cargo", "/cache/sccache"]);
    }

    #[test]
    fn parse_typescript_manifest() {
        let manifest = LayerManifest::parse(TS_MANIFEST).unwrap();
        assert_eq!(manifest.layer.name, "typescript");

        let vars = manifest.env_vars();
        assert_eq!(vars.get("PNPM_HOME").unwrap(), "/cache/pnpm");
        assert_eq!(vars.get("npm_config_cache").unwrap(), "/cache/npm");
        assert_eq!(vars.get("NODE_ENV").unwrap(), "development");

        assert_eq!(manifest.path_prepend_str(), Some("/cache/pnpm".to_string()));
    }

    #[test]
    fn missing_required_fields_errors() {
        let bad_toml = r#"
[layer]
name = "broken"
"#;
        let result = LayerManifest::parse(bad_toml);
        assert!(result.is_err());
    }

    #[test]
    fn empty_optional_fields() {
        let minimal = r#"
[layer]
name = "minimal"
description = "Minimal layer"
version = "1"
"#;
        let manifest = LayerManifest::parse(minimal).unwrap();
        assert!(manifest.env_vars().is_empty());
        assert!(manifest.path_prepend_str().is_none());
        assert!(manifest.cache.paths.is_empty());
    }
}
