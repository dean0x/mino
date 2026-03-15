//! Interactive prompts for network and layer selection

use crate::cli::args::RunArgs;
use crate::config::{Config, ConfigManager};
use crate::error::{MinoError, MinoResult};
use crate::layer::list_available_layers;
use crate::network::{resolve_preset, NetworkMode};
use crate::ui::{self, UiContext};
use console::style;
use std::path::Path;

/// Network mode selection for the interactive prompt
#[derive(Clone, PartialEq, Eq)]
enum NetworkChoice {
    Bridge,
    Host,
    AllowDev,
    AllowRegistries,
    None,
}

/// Where to save configuration
#[derive(Clone, PartialEq, Eq)]
enum SaveTarget {
    Local,
    Global,
    None,
}

/// Check if network is at defaults (no explicit CLI or config override).
pub(super) fn is_default_network(args: &RunArgs, config: &Config) -> bool {
    args.network.is_none()
        && args.network_allow.is_empty()
        && args.network_preset.is_none()
        && config.container.network == "bridge"
        && config.container.network_allow.is_empty()
        && config.container.network_preset.is_none()
}

/// Prompt user to select network mode interactively.
/// Returns the resolved `NetworkMode`.
pub(super) async fn prompt_network_selection(
    ctx: &UiContext,
    project_dir: &Path,
) -> MinoResult<NetworkMode> {
    let options: Vec<(NetworkChoice, &str, &str)> = vec![
        (
            NetworkChoice::Bridge,
            "Bridge (recommended)",
            "full internet, isolated from host services",
        ),
        (
            NetworkChoice::Host,
            "Host",
            "full host network (local databases, APIs)",
        ),
        (
            NetworkChoice::AllowDev,
            "Allowlist: dev",
            "GitHub, npm, crates.io, PyPI, AI APIs only",
        ),
        (
            NetworkChoice::AllowRegistries,
            "Allowlist: registries",
            "package registries only (most restrictive)",
        ),
        (NetworkChoice::None, "None", "no network (air-gapped)"),
    ];

    let choice = ui::select(ctx, "Select network mode", &options).await?;

    let (mode, preset_name) = match choice {
        NetworkChoice::Bridge => (NetworkMode::Bridge, None),
        NetworkChoice::Host => (NetworkMode::Host, None),
        NetworkChoice::AllowDev => (NetworkMode::Allow(resolve_preset("dev")?), Some("dev")),
        NetworkChoice::AllowRegistries => (
            NetworkMode::Allow(resolve_preset("registries")?),
            Some("registries"),
        ),
        NetworkChoice::None => (NetworkMode::None, None),
    };

    prompt_save_network(ctx, &choice, preset_name, project_dir).await?;

    Ok(mode)
}

/// Prompt user to choose where to save a config key, then persist it.
///
/// Returns early (with `Ok(())`) if the user chooses "Don't save".
async fn prompt_and_save(
    ctx: &UiContext,
    prompt: &str,
    skip_hint: &str,
    project_dir: &Path,
    key: &str,
    value: toml_edit::Value,
) -> MinoResult<()> {
    let options: Vec<(SaveTarget, &str, &str)> = vec![
        (SaveTarget::Local, "Save to .mino.toml", "this project only"),
        (
            SaveTarget::Global,
            "Save to global config",
            "~/.config/mino/config.toml",
        ),
        (SaveTarget::None, "Don't save", skip_hint),
    ];

    let target = ui::select(ctx, prompt, &options).await?;

    let path = match target {
        SaveTarget::Local => project_dir.join(".mino.toml"),
        SaveTarget::Global => ConfigManager::default_config_path(),
        SaveTarget::None => return Ok(()),
    };

    upsert_container_toml_key(&path, key, value).await?;
    println!("  {} Saved to {}", style("✓").green(), path.display());

    Ok(())
}

/// Save network selection to config.
async fn prompt_save_network(
    ctx: &UiContext,
    choice: &NetworkChoice,
    preset_name: Option<&str>,
    project_dir: &Path,
) -> MinoResult<()> {
    let (key, value): (&str, toml_edit::Value) = if let Some(preset) = preset_name {
        ("network_preset", preset.to_string().into())
    } else {
        let net = match choice {
            NetworkChoice::Host => "host",
            NetworkChoice::None => "none",
            _ => "bridge",
        };
        ("network", net.to_string().into())
    };

    prompt_and_save(
        ctx,
        "Save this network setting?",
        "prompt again next time",
        project_dir,
        key,
        value,
    )
    .await
}

/// Insert or update a key under [container] in a TOML config file.
///
/// Creates the file (and parent directories) if it does not exist.
/// Uses `toml_edit` for round-trip preservation of comments and formatting.
pub(super) async fn upsert_container_toml_key(
    path: &Path,
    key: &str,
    value: toml_edit::Value,
) -> MinoResult<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            MinoError::io(format!("creating config directory {}", parent.display()), e)
        })?;
    }

    let existing = match tokio::fs::read_to_string(path).await {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(MinoError::io(format!("reading {}", path.display()), e)),
    };

    let mut doc: toml_edit::DocumentMut = if let Some(content) = existing {
        content
            .parse()
            .map_err(|e: toml_edit::TomlError| MinoError::ConfigInvalid {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?
    } else {
        toml_edit::DocumentMut::new()
    };

    if !doc.contains_key("container") {
        doc.insert("container", toml_edit::Item::Table(toml_edit::Table::new()));
    }

    let container = doc["container"]
        .as_table_mut()
        .ok_or_else(|| MinoError::ConfigInvalid {
            path: path.to_path_buf(),
            reason: "'container' key exists but is not a table".to_string(),
        })?;

    container.insert(key, toml_edit::value(value));

    tokio::fs::write(path, doc.to_string())
        .await
        .map_err(|e| MinoError::io(format!("writing {}", path.display()), e))?;

    Ok(())
}

/// Sentinel value for the "Base only" multiselect option.
pub(super) const BASE_ONLY: &str = "__base__";

/// Prompt user to select development tool layers interactively.
/// Returns Some(layer_names) if layers selected, None for base-only container.
pub(super) async fn prompt_layer_selection(
    ctx: &UiContext,
    project_dir: &Path,
) -> MinoResult<Option<Vec<String>>> {
    let available = list_available_layers(project_dir).await?;

    // Build options: "Base only" first, then available layers
    let mut options: Vec<(String, String, String)> = vec![(
        BASE_ONLY.to_string(),
        "Base only".to_string(),
        "Claude Code, zsh, git — no extra language tools".to_string(),
    )];

    options.extend(
        available
            .iter()
            .map(|l| (l.name.clone(), l.name.clone(), l.description.clone())),
    );

    let option_refs: Vec<(String, &str, &str)> = options
        .iter()
        .map(|(v, l, h)| (v.clone(), l.as_str(), h.as_str()))
        .collect();

    let selected = ui::multiselect(
        ctx,
        "Select development tools (space to toggle, enter to confirm)",
        &option_refs,
        true,
    )
    .await?;

    // Filter out the sentinel — remaining entries are real layer names
    let layer_names: Vec<String> = selected.into_iter().filter(|s| s != BASE_ONLY).collect();

    if layer_names.is_empty() {
        // User selected "Base only" (or only "Base only") — offer to persist
        prompt_save_base_only(ctx, project_dir).await?;
        return Ok(None);
    }

    prompt_save_config(ctx, &layer_names, project_dir).await?;

    Ok(Some(layer_names))
}

/// Prompt user to save "Base only" selection to config.
///
/// Saves `image = "base"` under `[container]`, which `resolve_image_alias`
/// maps to `ghcr.io/dean0x/mino-base:latest`. On next run, `is_default_image`
/// returns false (image != "fedora:43"), skipping the layer prompt entirely.
async fn prompt_save_base_only(ctx: &UiContext, project_dir: &Path) -> MinoResult<()> {
    prompt_and_save(
        ctx,
        "Save this configuration?",
        "prompt again next time",
        project_dir,
        "image",
        "base".into(),
    )
    .await
}

/// Prompt user to save selected layers to config.
async fn prompt_save_config(
    ctx: &UiContext,
    layers: &[String],
    project_dir: &Path,
) -> MinoResult<()> {
    let mut layers_arr = toml_edit::Array::new();
    for l in layers {
        layers_arr.push(l.as_str());
    }

    prompt_and_save(
        ctx,
        "Save this configuration?",
        "prompt again next time",
        project_dir,
        "layers",
        toml_edit::Value::Array(layers_arr),
    )
    .await
}
