//! Image and layer resolution

use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::error::MinoResult;
use crate::layer::{compose_image, resolve_layers};
use crate::orchestration::ContainerRuntime;
use crate::ui::{BuildProgress, TaskSpinner, UiContext};
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

use super::ImageResolution;

/// Image registry prefix for mino images
const IMAGE_REGISTRY: &str = "ghcr.io/dean0x";

/// Default base image for layer composition (requires developer user, zsh, etc.)
pub(super) const LAYER_BASE_IMAGE: &str = "ghcr.io/dean0x/mino-base:latest";

/// Parse a comma-separated layer string into a list of layer names.
///
/// Trims whitespace and filters empty segments.
pub(super) fn parse_layers_env(val: &str) -> Vec<String> {
    val.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Determine which layers to compose (if any).
///
/// Returns None for single-image mode, Some(names) for layer composition.
///
/// Precedence:
/// 1. CLI `--layers` → compose from layers
/// 2. CLI `--image` → use single image (overrides config layers)
/// 3. `MINO_LAYERS` env var (comma-separated) → compose from env layers
/// 4. Config `container.layers` (non-empty) → compose from config layers
/// 5. Config `container.image` / default → use single image
pub(super) fn resolve_layer_names(args: &RunArgs, config: &Config) -> Option<Vec<String>> {
    if !args.layers.is_empty() {
        return Some(args.layers.clone());
    }
    if args.image.is_some() {
        return None;
    }
    if let Ok(val) = std::env::var("MINO_LAYERS") {
        let layers = parse_layers_env(&val);
        if !layers.is_empty() {
            return Some(layers);
        }
    }
    if !config.container.layers.is_empty() {
        return Some(config.container.layers.clone());
    }
    None
}

/// Map image alias names to layer names for composition.
///
/// Language aliases (typescript, rust, etc.) are redirected to the layer
/// composition system instead of pulling pre-built GHCR images.
pub(super) fn image_alias_to_layer(image: &str) -> Option<&str> {
    match image {
        "typescript" | "ts" | "node" => Some("typescript"),
        "rust" | "cargo" => Some("rust"),
        "python" | "py" => Some("python"),
        _ => None,
    }
}

/// Resolve image aliases to full registry paths.
///
/// Only `base` is a direct image alias. Language aliases (typescript, rust)
/// are handled by `image_alias_to_layer()` and redirected to layer composition.
///
/// Full image paths (containing `/` or `:`) are passed through unchanged.
pub(super) fn resolve_image_alias(image: &str) -> String {
    if image.contains('/') || image.contains(':') {
        return image.to_string();
    }

    match image {
        "base" => format!("{}/mino-base:latest", IMAGE_REGISTRY),
        other => other.to_string(),
    }
}

/// Check if no explicit image was provided and config uses the default image.
pub(super) fn is_default_image(args: &RunArgs, config: &Config) -> bool {
    args.image.is_none() && config.container.image == "fedora:43"
}

/// Resolve the final image when no layer composition is needed.
///
/// Handles two cases:
/// - `base_only=true`: use `LAYER_BASE_IMAGE` with empty env (user selected "Base only")
/// - `base_only=false`: resolve the raw image alias to a full path
///
/// Returns `(ImageResolution, using_layers)`.
pub(super) fn resolve_final_image(
    raw_image: &str,
    base_only: bool,
) -> (ImageResolution, bool) {
    let using_layers = base_only;

    let resolution = if base_only {
        debug!("Using base image without layers: {}", LAYER_BASE_IMAGE);
        ImageResolution {
            image: LAYER_BASE_IMAGE.to_string(),
            layer_env: HashMap::new(),
        }
    } else {
        ImageResolution {
            image: resolve_image_alias(raw_image),
            layer_env: HashMap::new(),
        }
    };

    (resolution, using_layers)
}

/// Resolve the image to use, handling layers, aliases, and interactive prompts.
///
/// Returns `(ImageResolution, using_layers)`.
pub(super) async fn resolve_image(
    args: &RunArgs,
    config: &Config,
    ctx: &UiContext,
    spinner: &mut TaskSpinner,
    runtime: &dyn ContainerRuntime,
    project_dir: &Path,
) -> MinoResult<(ImageResolution, bool)> {
    let raw_image = args
        .image
        .clone()
        .unwrap_or_else(|| config.container.image.clone());

    // Resolve layers from CLI/config, then check image alias redirect
    // (e.g., --image typescript -> layer composition)
    let layer_names = resolve_layer_names(args, config)
        .or_else(|| image_alias_to_layer(&raw_image).map(|name| vec![name.to_string()]));

    // Track whether the interactive prompt selected "Base only" (no layers but use mino-base)
    let (layer_names, base_only) =
        if layer_names.is_none() && ctx.is_interactive() && is_default_image(args, config) {
            spinner.clear();
            match super::prompts::prompt_layer_selection(ctx, project_dir).await? {
                Some(selected) => {
                    spinner.start("Initializing sandbox...");
                    (Some(selected), false)
                }
                None => (None, true),
            }
        } else {
            (layer_names, false)
        };

    // base_only uses mino-base with zsh, same as layer composition
    let using_layers = layer_names.is_some() || base_only;

    let resolution = if let Some(names) = layer_names {
        let mut resolved = Vec::new();
        for name in &names {
            spinner.message(&format!("Resolving layer: {}...", name));
            let mut layers = resolve_layers(std::slice::from_ref(name), project_dir).await?;
            resolved.append(&mut layers);
        }

        spinner.clear();

        let label = names.join(", ");
        let progress = BuildProgress::new(ctx, &label);
        let result = compose_image(
            runtime,
            LAYER_BASE_IMAGE,
            &resolved,
            Some(&|line: String| progress.on_line(line)),
        )
        .await;
        progress.finish();
        let result = result?;

        let action = if result.was_cached { "cached" } else { "built" };
        debug!("Using {} composed image: {}", action, result.image_tag);

        ImageResolution {
            image: result.image_tag,
            layer_env: result.env,
        }
    } else {
        let (res, _) = resolve_final_image(&raw_image, base_only);
        res
    };

    Ok((resolution, using_layers))
}
