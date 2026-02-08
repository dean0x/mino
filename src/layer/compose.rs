//! Image composition
//!
//! Combines multiple resolved layers into a single container image
//! using content-addressed caching. The composed image tag is derived
//! from a SHA256 hash of the base image + all layer contents.

use crate::error::{MinotaurError, MinotaurResult};
use crate::layer::resolve::ResolvedLayer;
use crate::orchestration::ContainerRuntime;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

/// Result of composing an image from layers
#[derive(Debug)]
pub struct ComposedImageResult {
    /// Full image tag (e.g., "minotaur-composed-a1b2c3d4e5f6")
    pub image_tag: String,

    /// Merged environment variables from all layers
    pub env: HashMap<String, String>,

    /// Whether the image was already cached (no build needed)
    pub was_cached: bool,
}

/// Compose a container image from multiple layers.
///
/// Generates a Dockerfile that installs each layer in order, builds
/// the image with a content-addressed tag, and returns the result.
/// If the image already exists locally, the build is skipped.
pub async fn compose_image(
    runtime: &dyn ContainerRuntime,
    base_image: &str,
    layers: &[ResolvedLayer],
) -> MinotaurResult<ComposedImageResult> {
    // Compute content-addressed hash
    let image_tag = compute_image_tag(base_image, layers).await?;
    debug!("Composed image tag: {}", image_tag);

    // Merge environment variables for the Dockerfile (baked into image)
    let build_env = merge_env(layers);

    // Check if image already exists
    if runtime.image_exists(&image_tag).await.unwrap_or(false) {
        debug!("Composed image already cached: {}", image_tag);
        return Ok(ComposedImageResult {
            image_tag,
            // Env vars are baked into the image via Dockerfile ENV instructions.
            // Do NOT re-inject at runtime — ${PATH} expansion only works in Dockerfile.
            env: HashMap::new(),
            was_cached: true,
        });
    }

    // Build the image
    let build_dir = prepare_build_dir(base_image, layers, &build_env).await?;

    let result = runtime.build_image(&build_dir, &image_tag).await;

    // Clean up build directory (best-effort)
    let _ = tokio::fs::remove_dir_all(&build_dir).await;

    result?;

    Ok(ComposedImageResult {
        image_tag,
        // Env vars are baked into the image via Dockerfile ENV instructions.
        env: HashMap::new(),
        was_cached: false,
    })
}

/// Compute a deterministic image tag from the base image and layer contents.
///
/// Hash inputs are sorted by layer name for determinism regardless of
/// CLI argument order. The install order follows the user's specified order.
async fn compute_image_tag(base_image: &str, layers: &[ResolvedLayer]) -> MinotaurResult<String> {
    let mut hasher = Sha256::new();

    hasher.update(base_image.as_bytes());

    // Sort by name for deterministic hash
    let mut sorted: Vec<&ResolvedLayer> = layers.iter().collect();
    sorted.sort_by_key(|l| &l.manifest.layer.name);

    for layer in sorted {
        hasher.update(layer.manifest.layer.name.as_bytes());

        let script_content = layer.install_script.content().await?;
        hasher.update(script_content.as_bytes());

        // Include manifest version in hash for cache invalidation
        hasher.update(layer.manifest.layer.version.as_bytes());
    }

    let hash = hex::encode(hasher.finalize());
    let short_hash = &hash[..12];

    Ok(format!("minotaur-composed-{}", short_hash))
}

/// Merge environment variables from all layers.
/// Last layer in the list wins for conflicting keys.
/// PATH prepends are accumulated from all layers.
fn merge_env(layers: &[ResolvedLayer]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let mut path_dirs: Vec<String> = Vec::new();

    for layer in layers {
        // Add flat env vars
        env.extend(layer.manifest.env_vars());

        // Collect PATH prepend dirs
        if let Some(prepend) = layer.manifest.path_prepend_str() {
            for dir in prepend.split(':') {
                if !path_dirs.contains(&dir.to_string()) {
                    path_dirs.push(dir.to_string());
                }
            }
        }
    }

    // Build composed PATH value (will be prepended to existing PATH)
    if !path_dirs.is_empty() {
        let path_value = format!("{}:${{PATH}}", path_dirs.join(":"));
        env.insert("PATH".to_string(), path_value);
    }

    env
}

/// Prepare a build directory with Dockerfile and install scripts.
///
/// Uses `~/.local/share/minotaur/builds/` so that OrbStack can access it
/// on macOS (OrbStack auto-mounts user home).
async fn prepare_build_dir(
    base_image: &str,
    layers: &[ResolvedLayer],
    env: &HashMap<String, String>,
) -> MinotaurResult<PathBuf> {
    let state_dir = state_dir()?;
    let builds_dir = state_dir.join("builds");
    tokio::fs::create_dir_all(&builds_dir)
        .await
        .map_err(|e| MinotaurError::io("creating builds directory", e))?;

    // Use a unique temp dir under builds/
    let build_id = uuid::Uuid::new_v4().to_string();
    let build_dir = builds_dir.join(&build_id);
    tokio::fs::create_dir_all(&build_dir)
        .await
        .map_err(|e| MinotaurError::io("creating build directory", e))?;

    // Write install scripts
    for layer in layers {
        let script_name = format!("install-{}.sh", layer.manifest.layer.name);
        let script_content = layer.install_script.content().await?;
        let script_path = build_dir.join(&script_name);
        tokio::fs::write(&script_path, &script_content)
            .await
            .map_err(|e| MinotaurError::io(format!("writing {}", script_name), e))?;
    }

    // Generate and write Dockerfile
    let dockerfile = generate_dockerfile(base_image, layers, env);
    tokio::fs::write(build_dir.join("Dockerfile"), &dockerfile)
        .await
        .map_err(|e| MinotaurError::io("writing Dockerfile", e))?;

    Ok(build_dir)
}

/// Generate a Dockerfile that composes all layers.
///
/// Each layer gets its own RUN instruction for Podman build cache
/// granularity. ENV vars are set after all layers are installed.
fn generate_dockerfile(
    base_image: &str,
    layers: &[ResolvedLayer],
    env: &HashMap<String, String>,
) -> String {
    let mut lines = Vec::new();

    lines.push(format!("FROM {}", base_image));
    lines.push(String::new());

    // Install each layer (order follows user's specified order)
    for layer in layers {
        let name = &layer.manifest.layer.name;
        let script_name = format!("install-{}.sh", name);

        lines.push(format!("# Layer: {}", name));
        lines.push("USER root".to_string());
        lines.push(format!("COPY {} /tmp/{}", script_name, script_name));
        lines.push(format!(
            "RUN chmod +x /tmp/{script_name} && /tmp/{script_name} && rm /tmp/{script_name}"
        ));
        lines.push(String::new());
    }

    // Switch to developer user
    lines.push("USER developer".to_string());

    // Set merged environment variables
    let mut env_keys: Vec<&String> = env.keys().collect();
    env_keys.sort();

    for key in env_keys {
        let value = &env[key];
        lines.push(format!("ENV {}={}", key, dockerfile_quote(value)));
    }

    lines.push(String::new());
    lines.push("WORKDIR /workspace".to_string());
    lines.push("CMD [\"/bin/zsh\"]".to_string());

    lines.join("\n")
}

/// Quote a value for Dockerfile ENV instruction.
/// Values containing $ (variable references) must be quoted properly.
/// Embedded double quotes and backslashes are escaped to prevent injection.
fn dockerfile_quote(value: &str) -> String {
    if value.contains('$') || value.contains(' ') || value.contains('"') || value.contains('\\') {
        // Escape backslashes first, then double quotes
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{}\"", escaped)
    } else {
        value.to_string()
    }
}

/// Get the minotaur state directory (`~/.local/share/minotaur/`)
fn state_dir() -> MinotaurResult<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| MinotaurError::Internal("Could not determine data directory".to_string()))?
        .join("minotaur");
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::manifest::LayerManifest;
    use crate::layer::resolve::{LayerScript, LayerSource, ResolvedLayer};

    fn make_layer(manifest_toml: &str, script: &'static str) -> ResolvedLayer {
        ResolvedLayer {
            manifest: LayerManifest::parse(manifest_toml).unwrap(),
            install_script: LayerScript::Embedded(script),
            source: LayerSource::BuiltIn,
        }
    }

    fn rust_layer() -> ResolvedLayer {
        make_layer(
            r#"
[layer]
name = "rust"
description = "Rust"
version = "1"

[env]
CARGO_HOME = "/cache/cargo"
RUSTUP_HOME = "/opt/rustup"

[env.path_prepend]
dirs = ["/opt/cargo/bin"]

[cache]
paths = ["/cache/cargo"]
"#,
            "#!/bin/bash\necho rust",
        )
    }

    fn ts_layer() -> ResolvedLayer {
        make_layer(
            r#"
[layer]
name = "typescript"
description = "TypeScript"
version = "1"

[env]
PNPM_HOME = "/cache/pnpm"
npm_config_cache = "/cache/npm"

[env.path_prepend]
dirs = ["/cache/pnpm"]

[cache]
paths = ["/cache/pnpm"]
"#,
            "#!/bin/bash\necho ts",
        )
    }

    #[test]
    fn merge_env_last_wins() {
        let layer_a = make_layer(
            r#"
[layer]
name = "a"
description = "A"
version = "1"
[env]
SHARED = "from_a"
ONLY_A = "a_val"
"#,
            "",
        );

        let layer_b = make_layer(
            r#"
[layer]
name = "b"
description = "B"
version = "1"
[env]
SHARED = "from_b"
ONLY_B = "b_val"
"#,
            "",
        );

        let env = merge_env(&[layer_a, layer_b]);
        assert_eq!(env.get("SHARED").unwrap(), "from_b");
        assert_eq!(env.get("ONLY_A").unwrap(), "a_val");
        assert_eq!(env.get("ONLY_B").unwrap(), "b_val");
    }

    #[test]
    fn merge_env_accumulates_path() {
        let layers = vec![rust_layer(), ts_layer()];
        let env = merge_env(&layers);

        let path = env.get("PATH").unwrap();
        assert!(path.contains("/opt/cargo/bin"));
        assert!(path.contains("/cache/pnpm"));
        assert!(path.contains("${PATH}"));
    }

    #[test]
    fn generate_dockerfile_structure() {
        let layers = vec![rust_layer(), ts_layer()];
        let env = merge_env(&layers);
        let dockerfile = generate_dockerfile("ghcr.io/dean0x/minotaur-base:latest", &layers, &env);

        assert!(dockerfile.contains("FROM ghcr.io/dean0x/minotaur-base:latest"));
        assert!(dockerfile.contains("# Layer: rust"));
        assert!(dockerfile.contains("COPY install-rust.sh /tmp/install-rust.sh"));
        assert!(dockerfile.contains("# Layer: typescript"));
        assert!(dockerfile.contains("COPY install-typescript.sh /tmp/install-typescript.sh"));
        assert!(dockerfile.contains("USER developer"));
        assert!(dockerfile.contains("ENV CARGO_HOME=/cache/cargo"));
        assert!(dockerfile.contains("ENV PNPM_HOME=/cache/pnpm"));
        assert!(dockerfile.contains("WORKDIR /workspace"));

        // Rust should come before TypeScript (user-specified order)
        let rust_pos = dockerfile.find("# Layer: rust").unwrap();
        let ts_pos = dockerfile.find("# Layer: typescript").unwrap();
        assert!(rust_pos < ts_pos);
    }

    #[tokio::test]
    async fn hash_is_deterministic() {
        let layers_a = vec![rust_layer(), ts_layer()];
        let layers_b = vec![rust_layer(), ts_layer()];

        let tag_a = compute_image_tag("base:latest", &layers_a).await.unwrap();
        let tag_b = compute_image_tag("base:latest", &layers_b).await.unwrap();

        assert_eq!(tag_a, tag_b);
    }

    #[tokio::test]
    async fn hash_is_order_independent() {
        // Hash should be the same regardless of layer order
        let layers_rt = vec![rust_layer(), ts_layer()];
        let layers_tr = vec![ts_layer(), rust_layer()];

        let tag_rt = compute_image_tag("base:latest", &layers_rt).await.unwrap();
        let tag_tr = compute_image_tag("base:latest", &layers_tr).await.unwrap();

        assert_eq!(tag_rt, tag_tr);
    }

    #[tokio::test]
    async fn hash_changes_with_base_image() {
        let layers = vec![rust_layer()];

        let tag_a = compute_image_tag("base:v1", &layers).await.unwrap();
        let tag_b = compute_image_tag("base:v2", &layers).await.unwrap();

        assert_ne!(tag_a, tag_b);
    }

    #[test]
    fn dockerfile_quote_simple() {
        assert_eq!(dockerfile_quote("/cache/cargo"), "/cache/cargo");
    }

    #[test]
    fn dockerfile_quote_with_variable() {
        assert_eq!(
            dockerfile_quote("/opt/cargo/bin:${PATH}"),
            "\"/opt/cargo/bin:${PATH}\""
        );
    }

    #[test]
    fn dockerfile_quote_escapes_embedded_quotes() {
        assert_eq!(
            dockerfile_quote("value with \"quotes\""),
            "\"value with \\\"quotes\\\"\""
        );
    }

    #[test]
    fn dockerfile_quote_escapes_backslashes() {
        assert_eq!(
            dockerfile_quote("path\\with\\backslashes"),
            "\"path\\\\with\\\\backslashes\""
        );
    }
}
