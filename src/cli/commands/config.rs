//! Config command - show or edit configuration

use crate::cli::args::{ConfigAction, ConfigArgs};
use crate::config::{Config, ConfigManager};
use crate::error::{MinoError, MinoResult};
use crate::ui::{self, UiContext};
use tokio::fs;

/// Execute the config command
pub async fn execute(args: ConfigArgs, config: &Config) -> MinoResult<()> {
    let manager = ConfigManager::new();

    match args.action {
        None | Some(ConfigAction::Show) => show_config(config),
        Some(ConfigAction::Path) => show_path(&manager),
        Some(ConfigAction::Init { force }) => init_config(&manager, force).await?,
        Some(ConfigAction::Set { key, value, local }) => {
            if local {
                set_local_value(&key, &value).await?
            } else {
                set_value(&manager, config, &key, &value).await?
            }
        }
    }

    Ok(())
}

fn show_config(config: &Config) {
    let toml =
        toml::to_string_pretty(config).unwrap_or_else(|_| "Error serializing config".to_string());
    println!("{}", toml);
}

fn show_path(manager: &ConfigManager) {
    println!("{}", manager.path().display());
}

async fn init_config(manager: &ConfigManager, force: bool) -> MinoResult<()> {
    let ctx = UiContext::detect();
    let path = manager.path();

    if path.exists() && !force {
        ui::step_warn_hint(
            &ctx,
            &format!("Config already exists at {}", path.display()),
            "Use --force to overwrite",
        );
        return Ok(());
    }

    let config = Config::default();
    manager.save(&config).await?;

    ui::step_ok_detail(
        &ctx,
        "Configuration initialized",
        &path.display().to_string(),
    );

    Ok(())
}

async fn set_value(
    manager: &ConfigManager,
    config: &Config,
    key: &str,
    value: &str,
) -> MinoResult<()> {
    let ctx = UiContext::detect();
    let mut config = config.clone();

    // Parse dot-separated key path
    let parts: Vec<&str> = key.split('.').collect();

    match parts.as_slice() {
        ["general", "verbose"] => config.general.verbose = parse_bool(value)?,
        ["general", "log_format"] => config.general.log_format = value.to_string(),
        ["general", "audit_log"] => config.general.audit_log = parse_bool(value)?,

        ["vm", "name"] => config.vm.name = value.to_string(),
        ["vm", "distro"] => config.vm.distro = value.to_string(),

        ["container", "image"] => config.container.image = value.to_string(),
        ["container", "network"] => config.container.network = value.to_string(),
        ["container", "workdir"] => config.container.workdir = value.to_string(),
        ["container", "network_allow"] => {
            config.container.network_allow = value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        ["credentials", "aws", "enabled"] => config.credentials.aws.enabled = parse_bool(value)?,
        ["credentials", "aws", "session_duration_secs"] => {
            config.credentials.aws.session_duration_secs = parse_u32(value)?
        }
        ["credentials", "aws", "role_arn"] => {
            config.credentials.aws.role_arn = Some(value.to_string())
        }
        ["credentials", "aws", "profile"] => {
            config.credentials.aws.profile = Some(value.to_string())
        }
        ["credentials", "aws", "region"] => config.credentials.aws.region = Some(value.to_string()),

        ["credentials", "gcp", "enabled"] => config.credentials.gcp.enabled = parse_bool(value)?,
        ["credentials", "gcp", "project"] => {
            config.credentials.gcp.project = Some(value.to_string())
        }

        ["credentials", "azure", "enabled"] => {
            config.credentials.azure.enabled = parse_bool(value)?
        }
        ["credentials", "azure", "subscription"] => {
            config.credentials.azure.subscription = Some(value.to_string())
        }
        ["credentials", "azure", "tenant"] => {
            config.credentials.azure.tenant = Some(value.to_string())
        }

        ["session", "shell"] => config.session.shell = value.to_string(),
        ["session", "auto_cleanup_hours"] => config.session.auto_cleanup_hours = parse_u32(value)?,

        _ => {
            ui::step_error_detail(&ctx, "Unknown config key", key);
            ui::remark(&ctx, "Valid keys:");
            print_valid_keys();
            return Ok(());
        }
    }

    manager.save(&config).await?;
    ui::step_ok(&ctx, &format!("Set {} = {}", key, value));

    Ok(())
}

async fn set_local_value(key: &str, value: &str) -> MinoResult<()> {
    let ctx = UiContext::detect();

    let cwd = std::env::current_dir().map_err(|e| MinoError::io("getting current directory", e))?;
    let local_path = cwd.join(".mino.toml");

    // Validate the key before touching the file
    validate_config_key(key)?;

    // Load existing local config or start with an empty TOML table
    let mut doc: toml::Value = if local_path.exists() {
        let content = fs::read_to_string(&local_path)
            .await
            .map_err(|e| MinoError::io(format!("reading {}", local_path.display()), e))?;
        content
            .parse()
            .map_err(|e: toml::de::Error| MinoError::ConfigInvalid {
                path: local_path.clone(),
                reason: e.to_string(),
            })?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    // Set the key in the TOML tree
    set_toml_value(&mut doc, key, value)?;

    // Write back only the keys the user has explicitly set
    let content = toml::to_string_pretty(&doc)?;
    fs::write(&local_path, content)
        .await
        .map_err(|e| MinoError::io(format!("writing {}", local_path.display()), e))?;

    ui::step_ok(
        &ctx,
        &format!("Set {} = {} in {}", key, value, local_path.display()),
    );

    Ok(())
}

/// Validate that a config key is one we recognise.
fn validate_config_key(key: &str) -> MinoResult<()> {
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["general", "verbose" | "log_format" | "audit_log"]
        | ["vm", "name" | "distro"]
        | ["container", "image" | "network" | "workdir" | "network_allow"]
        | ["credentials", "aws", "enabled" | "session_duration_secs" | "role_arn" | "profile" | "region"]
        | ["credentials", "gcp", "enabled" | "project"]
        | ["credentials", "azure", "enabled" | "subscription" | "tenant"]
        | ["session", "shell" | "auto_cleanup_hours"] => Ok(()),
        _ => Err(MinoError::User(format!("Unknown config key: {}", key))),
    }
}

/// Set a dot-separated key in a TOML value tree, creating intermediate tables as needed.
fn set_toml_value(doc: &mut toml::Value, key: &str, value: &str) -> MinoResult<()> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = doc;

    // Navigate/create intermediate tables
    for &part in &parts[..parts.len() - 1] {
        current = current
            .as_table_mut()
            .ok_or_else(|| MinoError::User(format!("Expected table at key: {}", part)))?
            .entry(part)
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    }

    let leaf = parts.last().unwrap();
    let table = current
        .as_table_mut()
        .ok_or_else(|| MinoError::User(format!("Expected table for key: {}", key)))?;

    // Keys that store as arrays
    let is_list_key = key.ends_with("network_allow")
        || key.ends_with("layers")
        || key.ends_with("volumes");

    let toml_value = if is_list_key {
        let items: Vec<toml::Value> = value
            .split(',')
            .map(|s| toml::Value::String(s.trim().to_string()))
            .filter(|v| v.as_str().map(|s| !s.is_empty()).unwrap_or(false))
            .collect();
        toml::Value::Array(items)
    } else if value == "true" || value == "false" {
        toml::Value::Boolean(value.parse().unwrap())
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else {
        toml::Value::String(value.to_string())
    };

    table.insert((*leaf).to_string(), toml_value);
    Ok(())
}

fn parse_bool(value: &str) -> MinoResult<bool> {
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(crate::error::MinoError::User(format!(
            "Invalid boolean value: {}. Use true/false",
            value
        ))),
    }
}

fn parse_u32(value: &str) -> MinoResult<u32> {
    value
        .parse()
        .map_err(|_| crate::error::MinoError::User(format!("Invalid number: {}", value)))
}

fn print_valid_keys() {
    let keys = [
        "general.verbose",
        "general.log_format",
        "general.audit_log",
        "vm.name",
        "vm.distro",
        "container.image",
        "container.network",
        "container.workdir",
        "container.network_allow",
        "credentials.aws.enabled",
        "credentials.aws.session_duration_secs",
        "credentials.aws.role_arn",
        "credentials.aws.profile",
        "credentials.aws.region",
        "credentials.gcp.enabled",
        "credentials.gcp.project",
        "credentials.azure.enabled",
        "credentials.azure.subscription",
        "credentials.azure.tenant",
        "session.shell",
        "session.auto_cleanup_hours",
    ];

    for key in keys {
        eprintln!("  {}", key);
    }
}
