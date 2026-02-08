//! Layer resolution
//!
//! Resolves layer names to their manifests and install scripts by searching:
//! 1. Project-local: `{project_dir}/.minotaur/layers/{name}/`
//! 2. User-global: `~/.config/minotaur/layers/{name}/`
//! 3. Built-in: compiled into the binary via `include_str!`

use crate::error::{MinotaurError, MinotaurResult};
use crate::layer::manifest::LayerManifest;
use std::path::{Path, PathBuf};

// Built-in layers embedded at compile time
const BUILTIN_RUST_MANIFEST: &str = include_str!("../../images/rust/layer.toml");
const BUILTIN_RUST_INSTALL: &str = include_str!("../../images/rust/install.sh");
const BUILTIN_TS_MANIFEST: &str = include_str!("../../images/typescript/layer.toml");
const BUILTIN_TS_INSTALL: &str = include_str!("../../images/typescript/install.sh");

/// A fully resolved layer ready for composition
#[derive(Debug)]
pub struct ResolvedLayer {
    /// Parsed manifest
    pub manifest: LayerManifest,

    /// Install script content or path
    pub install_script: LayerScript,

    /// Where this layer was found
    pub source: LayerSource,
}

/// Install script reference
#[derive(Debug)]
pub enum LayerScript {
    /// File on disk (user-defined layer)
    Path(PathBuf),

    /// Embedded content (built-in layer)
    Embedded(&'static str),
}

impl LayerScript {
    /// Read the script content (from disk or embedded)
    pub async fn content(&self) -> MinotaurResult<String> {
        match self {
            Self::Path(path) => tokio::fs::read_to_string(path).await.map_err(|e| {
                MinotaurError::io(format!("reading install script {}", path.display()), e)
            }),
            Self::Embedded(content) => Ok((*content).to_string()),
        }
    }
}

/// Where a layer was resolved from
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerSource {
    /// `.minotaur/layers/{name}/` in the project directory
    ProjectLocal,

    /// `~/.config/minotaur/layers/{name}/`
    UserGlobal,

    /// Compiled into the binary
    BuiltIn,
}

/// Resolve a list of layer names to their manifests and scripts.
///
/// Resolution chain (first match wins per layer):
/// 1. `{project_dir}/.minotaur/layers/{name}/`
/// 2. `~/.config/minotaur/layers/{name}/`
/// 3. Built-in embedded layers
pub async fn resolve_layers(
    names: &[String],
    project_dir: &Path,
) -> MinotaurResult<Vec<ResolvedLayer>> {
    let mut resolved = Vec::with_capacity(names.len());

    for name in names {
        let layer = resolve_single(name, project_dir).await?;
        resolved.push(layer);
    }

    Ok(resolved)
}

/// Validate that a layer name is safe (no path traversal, no special characters).
fn validate_layer_name(name: &str) -> MinotaurResult<()> {
    if name.is_empty() {
        return Err(MinotaurError::User("Layer name cannot be empty".to_string()));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
        return Err(MinotaurError::User(format!(
            "Invalid layer name '{}': must not contain path separators or '..'",
            name
        )));
    }
    // Only allow alphanumeric, hyphens, underscores
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(MinotaurError::User(format!(
            "Invalid layer name '{}': must contain only alphanumeric characters, hyphens, or underscores",
            name
        )));
    }
    Ok(())
}

async fn resolve_single(name: &str, project_dir: &Path) -> MinotaurResult<ResolvedLayer> {
    validate_layer_name(name)?;

    let project_layer_dir = project_dir.join(".minotaur").join("layers").join(name);
    let global_layer_dir = dirs::config_dir()
        .map(|d| d.join("minotaur").join("layers").join(name));

    // 1. Project-local
    if let Some(layer) = try_resolve_from_dir(&project_layer_dir, LayerSource::ProjectLocal).await?
    {
        return Ok(layer);
    }

    // 2. User-global
    if let Some(ref dir) = global_layer_dir {
        if let Some(layer) = try_resolve_from_dir(dir, LayerSource::UserGlobal).await? {
            return Ok(layer);
        }
    }

    // 3. Built-in
    if let Some(layer) = resolve_builtin(name)? {
        return Ok(layer);
    }

    // Build the searched paths string for the error
    let mut searched = vec![project_layer_dir.display().to_string()];
    if let Some(ref dir) = global_layer_dir {
        searched.push(dir.display().to_string());
    }
    searched.push("built-in layers".to_string());

    Err(MinotaurError::LayerNotFound {
        name: name.to_string(),
        searched: searched.join(", "),
    })
}

/// Try to resolve a layer from a directory on disk.
/// Returns None if the directory doesn't exist.
/// Returns Err if the directory exists but is invalid (missing files).
async fn try_resolve_from_dir(
    dir: &Path,
    source: LayerSource,
) -> MinotaurResult<Option<ResolvedLayer>> {
    let manifest_path = dir.join("layer.toml");
    let script_path = dir.join("install.sh");

    if !manifest_path.exists() {
        return Ok(None);
    }

    // Manifest exists but script is missing — that's an error, not a miss
    if !script_path.exists() {
        return Err(MinotaurError::LayerScriptMissing(
            script_path.display().to_string(),
        ));
    }

    let manifest = LayerManifest::from_file(&manifest_path).await?;

    Ok(Some(ResolvedLayer {
        manifest,
        install_script: LayerScript::Path(script_path),
        source,
    }))
}

/// Resolve a built-in layer by name
fn resolve_builtin(name: &str) -> MinotaurResult<Option<ResolvedLayer>> {
    let (manifest_str, install_str) = match name {
        "rust" | "cargo" => (BUILTIN_RUST_MANIFEST, BUILTIN_RUST_INSTALL),
        "typescript" | "ts" | "node" => (BUILTIN_TS_MANIFEST, BUILTIN_TS_INSTALL),
        _ => return Ok(None),
    };

    let manifest = LayerManifest::parse(manifest_str)?;

    Ok(Some(ResolvedLayer {
        manifest,
        install_script: LayerScript::Embedded(install_str),
        source: LayerSource::BuiltIn,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_builtin_rust() {
        let layer = resolve_builtin("rust").unwrap().unwrap();
        assert_eq!(layer.manifest.layer.name, "rust");
        assert!(matches!(layer.source, LayerSource::BuiltIn));
        assert!(matches!(layer.install_script, LayerScript::Embedded(_)));
    }

    #[test]
    fn resolve_builtin_typescript() {
        let layer = resolve_builtin("typescript").unwrap().unwrap();
        assert_eq!(layer.manifest.layer.name, "typescript");
    }

    #[test]
    fn resolve_builtin_aliases() {
        assert!(resolve_builtin("cargo").unwrap().is_some());
        assert!(resolve_builtin("ts").unwrap().is_some());
        assert!(resolve_builtin("node").unwrap().is_some());
    }

    #[test]
    fn resolve_builtin_unknown() {
        assert!(resolve_builtin("python").unwrap().is_none());
    }

    #[tokio::test]
    async fn resolve_project_local_layer() {
        let temp = TempDir::new().unwrap();
        let layer_dir = temp.path().join(".minotaur").join("layers").join("custom");
        std::fs::create_dir_all(&layer_dir).unwrap();

        let manifest = r#"
[layer]
name = "custom"
description = "Custom layer"
version = "1"

[env]
MY_VAR = "/custom/path"
"#;
        std::fs::write(layer_dir.join("layer.toml"), manifest).unwrap();
        std::fs::write(layer_dir.join("install.sh"), "#!/bin/bash\necho ok").unwrap();

        let layers = resolve_layers(&["custom".to_string()], temp.path())
            .await
            .unwrap();

        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].manifest.layer.name, "custom");
        assert_eq!(layers[0].source, LayerSource::ProjectLocal);
        assert!(matches!(layers[0].install_script, LayerScript::Path(_)));
    }

    #[tokio::test]
    async fn resolve_missing_layer_errors() {
        let temp = TempDir::new().unwrap();
        let result = resolve_layers(&["nonexistent".to_string()], temp.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"));
    }

    #[tokio::test]
    async fn resolve_missing_script_errors() {
        let temp = TempDir::new().unwrap();
        let layer_dir = temp.path().join(".minotaur").join("layers").join("broken");
        std::fs::create_dir_all(&layer_dir).unwrap();

        let manifest = r#"
[layer]
name = "broken"
description = "Broken layer"
version = "1"
"#;
        std::fs::write(layer_dir.join("layer.toml"), manifest).unwrap();
        // No install.sh!

        let result = resolve_layers(&["broken".to_string()], temp.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("install script missing"));
    }

    #[tokio::test]
    async fn project_local_overrides_builtin() {
        let temp = TempDir::new().unwrap();
        let layer_dir = temp.path().join(".minotaur").join("layers").join("rust");
        std::fs::create_dir_all(&layer_dir).unwrap();

        let manifest = r#"
[layer]
name = "rust"
description = "Custom Rust"
version = "99"
"#;
        std::fs::write(layer_dir.join("layer.toml"), manifest).unwrap();
        std::fs::write(layer_dir.join("install.sh"), "#!/bin/bash\necho custom").unwrap();

        let layers = resolve_layers(&["rust".to_string()], temp.path())
            .await
            .unwrap();

        assert_eq!(layers[0].manifest.layer.version, "99");
        assert_eq!(layers[0].source, LayerSource::ProjectLocal);
    }

    #[tokio::test]
    async fn embedded_script_content() {
        let layer = resolve_builtin("rust").unwrap().unwrap();
        let content = layer.install_script.content().await.unwrap();
        assert!(content.contains("rustup"));
        assert!(content.contains("cargo-binstall"));
    }

    #[test]
    fn validate_layer_name_rejects_traversal() {
        assert!(validate_layer_name("../etc").is_err());
        assert!(validate_layer_name("foo/bar").is_err());
        assert!(validate_layer_name("foo\\bar").is_err());
        assert!(validate_layer_name("..").is_err());
    }

    #[test]
    fn validate_layer_name_rejects_empty() {
        assert!(validate_layer_name("").is_err());
    }

    #[test]
    fn validate_layer_name_rejects_special_chars() {
        assert!(validate_layer_name("rust!").is_err());
        assert!(validate_layer_name("hello world").is_err());
        assert!(validate_layer_name("layer.name").is_err());
    }

    #[test]
    fn validate_layer_name_accepts_valid() {
        assert!(validate_layer_name("rust").is_ok());
        assert!(validate_layer_name("typescript").is_ok());
        assert!(validate_layer_name("my-layer").is_ok());
        assert!(validate_layer_name("my_layer_v2").is_ok());
    }

    #[tokio::test]
    async fn resolve_rejects_traversal_name() {
        let temp = TempDir::new().unwrap();
        let result = resolve_layers(&["../evil".to_string()], temp.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid layer name"));
    }
}
