//! Image composition
//!
//! Combines multiple resolved layers into a single container image
//! using content-addressed caching. The composed image tag is derived
//! from a SHA256 hash of the base image + all layer contents.

use crate::error::{MinoError, MinoResult};
use crate::layer::resolve::ResolvedLayer;
use crate::orchestration::ContainerRuntime;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

/// Result of composing an image from layers
#[derive(Debug)]
pub struct ComposedImageResult {
    /// Full image tag (e.g., "mino-composed-a1b2c3d4e5f6")
    pub image_tag: String,

    /// Merged environment variables from all layers
    pub env: HashMap<String, String>,

    /// Whether the image was already cached (no build needed)
    pub was_cached: bool,
}

/// Check if any layers require a Dockerfile build step.
///
/// Returns `false` when all layers are pure user-install (handled by bootstrap).
/// Returns `true` when at least one layer has a root-level install script
/// or `root_install.packages`.
pub fn needs_compose_build(layers: &[ResolvedLayer]) -> bool {
    layers
        .iter()
        .any(|l| l.install_script.has_content() || l.manifest.has_root_install())
}

/// Merge environment variables from all layers (public for use when compose is skipped).
///
/// Last layer in the list wins for conflicting keys.
/// PATH prepends are accumulated from all layers.
///
/// When `for_dockerfile` is true, PATH uses `${PATH}` expansion (Dockerfile ENV).
/// When false, PATH dirs are returned via `MINO_PATH_PREPEND` env var
/// (for shell-level expansion in mino.zsh).
pub(crate) fn merge_layer_env(
    layers: &[ResolvedLayer],
    for_dockerfile: bool,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let mut path_dirs: Vec<String> = Vec::new();

    for layer in layers {
        env.extend(layer.manifest.env_vars());

        if let Some(prepend) = layer.manifest.path_prepend_str() {
            for dir in prepend.split(':') {
                if !path_dirs.contains(&dir.to_string()) {
                    path_dirs.push(dir.to_string());
                }
            }
        }
    }

    if !path_dirs.is_empty() {
        if for_dockerfile {
            let path_value = format!("{}:${{PATH}}", path_dirs.join(":"));
            env.insert("PATH".to_string(), path_value);
        } else {
            env.insert("MINO_PATH_PREPEND".to_string(), path_dirs.join(":"));
        }
    }

    env
}

/// Compose a container image from multiple layers.
///
/// Generates a Dockerfile that installs each layer in order, builds
/// the image with a content-addressed tag, and returns the result.
/// If the image already exists locally, the build is skipped.
///
/// When `on_build_output` is provided, build output is streamed line-by-line
/// through the callback for progress reporting. Otherwise uses batch build.
pub async fn compose_image(
    runtime: &dyn ContainerRuntime,
    base_image: &str,
    layers: &[ResolvedLayer],
    on_build_output: Option<&(dyn Fn(String) + Send + Sync)>,
) -> MinoResult<ComposedImageResult> {
    // Compute content-addressed hash
    let image_tag = compute_image_tag(base_image, layers).await?;
    debug!("Composed image tag: {}", image_tag);

    // Merge environment variables for the Dockerfile (baked into image)
    let build_env = merge_layer_env(layers, true);

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

    let result = if let Some(callback) = on_build_output {
        runtime
            .build_image_with_progress(&build_dir, &image_tag, callback)
            .await
    } else {
        runtime.build_image(&build_dir, &image_tag).await
    };

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
async fn compute_image_tag(base_image: &str, layers: &[ResolvedLayer]) -> MinoResult<String> {
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

    Ok(format!("mino-composed-{}", short_hash))
}

/// Prepare a build directory with Dockerfile and install scripts.
///
/// Uses `~/.local/share/mino/builds/` so that OrbStack can access it
/// on macOS (OrbStack auto-mounts user home).
async fn prepare_build_dir(
    base_image: &str,
    layers: &[ResolvedLayer],
    env: &HashMap<String, String>,
) -> MinoResult<PathBuf> {
    let state_dir = state_dir()?;
    let builds_dir = state_dir.join("builds");
    tokio::fs::create_dir_all(&builds_dir)
        .await
        .map_err(|e| MinoError::io("creating builds directory", e))?;

    // Use a unique temp dir under builds/
    let build_id = uuid::Uuid::new_v4().to_string();
    let build_dir = builds_dir.join(&build_id);
    tokio::fs::create_dir_all(&build_dir)
        .await
        .map_err(|e| MinoError::io("creating build directory", e))?;

    // Write install scripts (skip layers with no compose-time script)
    for layer in layers {
        if !layer.install_script.has_content() {
            continue;
        }
        let script_name = format!("install-{}.sh", layer.manifest.layer.name);
        let script_content = layer.install_script.content().await?;
        let script_path = build_dir.join(&script_name);
        tokio::fs::write(&script_path, &script_content)
            .await
            .map_err(|e| MinoError::io(format!("writing {}", script_name), e))?;
    }

    // Generate and write Dockerfile
    let dockerfile = generate_dockerfile(base_image, layers, env);
    tokio::fs::write(build_dir.join("Dockerfile"), &dockerfile)
        .await
        .map_err(|e| MinoError::io("writing Dockerfile", e))?;

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

    // Install each layer that has a compose-time script (skip user-install-only layers)
    for layer in layers {
        if !layer.install_script.has_content() {
            continue;
        }
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

    // Auto-generate dnf install step for layers with root_install.packages
    let root_packages: Vec<String> = layers
        .iter()
        .filter(|l| l.manifest.has_root_install())
        .flat_map(|l| l.manifest.root_install.packages.clone())
        .collect();

    if !root_packages.is_empty() {
        lines.push("# Root-level packages from layer manifests".to_string());
        lines.push("USER root".to_string());
        lines.push(format!(
            "RUN dnf install -y --setopt=install_weak_deps=False {} && dnf clean all",
            root_packages.join(" ")
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
    // NOTE: ENTRYPOINT inherited from base image (mino-entrypoint → bootstrap)
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

/// Get the mino state directory (`~/.local/share/mino/`)
fn state_dir() -> MinoResult<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| MinoError::Internal("Could not determine data directory".to_string()))?
        .join("mino");
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

        let env = merge_layer_env(&[layer_a, layer_b], true);
        assert_eq!(env.get("SHARED").unwrap(), "from_b");
        assert_eq!(env.get("ONLY_A").unwrap(), "a_val");
        assert_eq!(env.get("ONLY_B").unwrap(), "b_val");
    }

    #[test]
    fn merge_env_accumulates_path() {
        let layers = vec![rust_layer(), ts_layer()];
        let env = merge_layer_env(&layers, true);

        let path = env.get("PATH").unwrap();
        assert!(path.contains("/opt/cargo/bin"));
        assert!(path.contains("/cache/pnpm"));
        assert!(path.contains("${PATH}"));
    }

    #[test]
    fn generate_dockerfile_structure() {
        let layers = vec![rust_layer(), ts_layer()];
        let env = merge_layer_env(&layers, true);
        let dockerfile = generate_dockerfile("ghcr.io/dean0x/mino-base:latest", &layers, &env);

        assert!(dockerfile.contains("FROM ghcr.io/dean0x/mino-base:latest"));
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

    #[test]
    fn needs_compose_build_with_install_scripts() {
        let layers = vec![rust_layer(), ts_layer()];
        assert!(needs_compose_build(&layers));
    }

    #[test]
    fn needs_compose_build_pure_user_install() {
        let layer = ResolvedLayer {
            manifest: LayerManifest::parse(
                r#"
[layer]
name = "user-only"
description = "User only"
version = "1"

[user_install]
runtime = "nvm"
npm_globals = ["pnpm"]
"#,
            )
            .unwrap(),
            install_script: LayerScript::None,
            source: LayerSource::BuiltIn,
        };
        assert!(!needs_compose_build(&[layer]));
    }

    #[test]
    fn needs_compose_build_with_root_install() {
        let layer = ResolvedLayer {
            manifest: LayerManifest::parse(
                r#"
[layer]
name = "with-root"
description = "Has root packages"
version = "1"

[root_install]
packages = ["python3"]

[user_install]
runtime = "uv"
"#,
            )
            .unwrap(),
            install_script: LayerScript::None,
            source: LayerSource::BuiltIn,
        };
        assert!(needs_compose_build(&[layer]));
    }

    #[test]
    fn merge_layer_env_runtime_mode_uses_mino_path_prepend() {
        let layers = vec![rust_layer()];
        let env = merge_layer_env(&layers, false);
        assert!(env.get("PATH").is_none());
        assert!(env
            .get("MINO_PATH_PREPEND")
            .unwrap()
            .contains("/opt/cargo/bin"));
    }

    #[test]
    fn merge_layer_env_dockerfile_mode_uses_path() {
        let layers = vec![rust_layer()];
        let env = merge_layer_env(&layers, true);
        assert!(env.get("PATH").unwrap().contains("${PATH}"));
        assert!(env.get("MINO_PATH_PREPEND").is_none());
    }

    #[test]
    fn generate_dockerfile_skips_none_scripts() {
        let user_only = ResolvedLayer {
            manifest: LayerManifest::parse(
                r#"
[layer]
name = "user-only"
description = "User only"
version = "1"

[user_install]
runtime = "nvm"
"#,
            )
            .unwrap(),
            install_script: LayerScript::None,
            source: LayerSource::BuiltIn,
        };
        let layers = vec![rust_layer(), user_only];
        let env = merge_layer_env(&layers, true);
        let dockerfile = generate_dockerfile("base:latest", &layers, &env);

        // rust layer should be in Dockerfile
        assert!(dockerfile.contains("# Layer: rust"));
        // user-only layer should NOT have a COPY/RUN
        assert!(!dockerfile.contains("# Layer: user-only"));
    }

    #[test]
    fn generate_dockerfile_auto_root_install() {
        let layer = ResolvedLayer {
            manifest: LayerManifest::parse(
                r#"
[layer]
name = "python"
description = "Python"
version = "2"

[root_install]
packages = ["python3", "python3-devel"]

[user_install]
runtime = "uv"
"#,
            )
            .unwrap(),
            install_script: LayerScript::None,
            source: LayerSource::BuiltIn,
        };
        let env = merge_layer_env(&[layer], true);
        // Need to pass a slice reference, re-create layer
        let layer2 = ResolvedLayer {
            manifest: LayerManifest::parse(
                r#"
[layer]
name = "python"
description = "Python"
version = "2"

[root_install]
packages = ["python3", "python3-devel"]

[user_install]
runtime = "uv"
"#,
            )
            .unwrap(),
            install_script: LayerScript::None,
            source: LayerSource::BuiltIn,
        };
        let dockerfile = generate_dockerfile("base:latest", &[layer2], &env);

        assert!(dockerfile
            .contains("dnf install -y --setopt=install_weak_deps=False python3 python3-devel"));
        assert!(dockerfile.contains("dnf clean all"));
    }
}
