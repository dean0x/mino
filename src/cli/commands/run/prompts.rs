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

    // Offer to save
    prompt_save_network(ctx, &choice, preset_name, project_dir).await?;

    Ok(mode)
}

/// Save network selection to config.
async fn prompt_save_network(
    ctx: &UiContext,
    choice: &NetworkChoice,
    preset_name: Option<&str>,
    project_dir: &Path,
) -> MinoResult<()> {
    let options: Vec<(SaveTarget, &str, &str)> = vec![
        (SaveTarget::Local, "Save to .mino.toml", "this project only"),
        (
            SaveTarget::Global,
            "Save to global config",
            "~/.config/mino/config.toml",
        ),
        (SaveTarget::None, "Don't save", "prompt again next time"),
    ];

    let target = ui::select(ctx, "Save this network setting?", &options).await?;

    if target == SaveTarget::None {
        return Ok(());
    }

    let path = match target {
        SaveTarget::Local => project_dir.join(".mino.toml"),
        SaveTarget::Global => ConfigManager::default_config_path(),
        SaveTarget::None => unreachable!(),
    };

    let (key, toml_value): (&str, toml_edit::Value) = if let Some(preset) = preset_name {
        ("network_preset", preset.to_string().into())
    } else {
        let net = match choice {
            NetworkChoice::Host => "host",
            NetworkChoice::None => "none",
            _ => "bridge",
        };
        ("network", net.to_string().into())
    };

    upsert_container_toml_key(&path, key, toml_value).await?;
    println!("  {} Saved to {}", style("✓").green(), path.display());

    Ok(())
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
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            MinoError::io(format!("creating config directory {}", parent.display()), e)
        })?;
    }

    // Attempt to read existing file; NotFound means create new
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

    // Navigate to or create [container] table
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

/// Prompt user to select development tool layers interactively.
/// Returns Some(layer_names) if layers selected, None for bare container.
pub(super) async fn prompt_layer_selection(
    ctx: &UiContext,
    project_dir: &Path,
    config: &Config,
) -> MinoResult<Option<Vec<String>>> {
    let available = list_available_layers(project_dir).await?;
    if available.is_empty() {
        return Ok(None);
    }

    let options: Vec<(String, String, String)> = available
        .iter()
        .map(|l| (l.name.clone(), l.name.clone(), l.description.clone()))
        .collect();
    let option_refs: Vec<(String, &str, &str)> = options
        .iter()
        .map(|(v, l, h)| (v.clone(), l.as_str(), h.as_str()))
        .collect();

    let selected = ui::multiselect(
        ctx,
        "Select development tools (space to toggle, enter to confirm)",
        &option_refs,
        false,
    )
    .await?;

    if selected.is_empty() {
        return Ok(None);
    }

    // Ask where to save
    prompt_save_config(ctx, &selected, project_dir, config).await?;

    Ok(Some(selected))
}

/// Prompt user to save selected layers to config.
async fn prompt_save_config(
    ctx: &UiContext,
    layers: &[String],
    project_dir: &Path,
    _config: &Config,
) -> MinoResult<()> {
    let options: Vec<(SaveTarget, &str, &str)> = vec![
        (SaveTarget::Local, "Save to .mino.toml", "this project only"),
        (
            SaveTarget::Global,
            "Save to global config",
            "~/.config/mino/config.toml",
        ),
        (SaveTarget::None, "Don't save", "one-time, no persistence"),
    ];

    let target = ui::select(ctx, "Save this configuration?", &options).await?;

    let path = match target {
        SaveTarget::Local => project_dir.join(".mino.toml"),
        SaveTarget::Global => ConfigManager::default_config_path(),
        SaveTarget::None => return Ok(()),
    };

    let mut layers_arr = toml_edit::Array::new();
    for l in layers {
        layers_arr.push(l.as_str());
    }
    upsert_container_toml_key(&path, "layers", toml_edit::Value::Array(layers_arr)).await?;
    println!("  {} Saved to {}", style("✓").green(), path.display());

    Ok(())
}
