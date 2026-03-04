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

    // Load existing local config or start with an empty document (preserves comments)
    let mut doc: toml_edit::DocumentMut = if local_path.exists() {
        let content = fs::read_to_string(&local_path)
            .await
            .map_err(|e| MinoError::io(format!("reading {}", local_path.display()), e))?;
        content
            .parse()
            .map_err(|e: toml_edit::TomlError| MinoError::ConfigInvalid {
                path: local_path.clone(),
                reason: e.to_string(),
            })?
    } else {
        toml_edit::DocumentMut::new()
    };

    // Set the key in the TOML tree
    set_toml_edit_value(&mut doc, key, value)?;

    // Write back preserving comments and formatting
    fs::write(&local_path, doc.to_string())
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

/// Set a dot-separated key in a toml_edit document, creating intermediate tables as needed.
/// Preserves comments and formatting in the original document.
fn set_toml_edit_value(doc: &mut toml_edit::DocumentMut, key: &str, value: &str) -> MinoResult<()> {
    let parts: Vec<&str> = key.split('.').collect();

    // Navigate/create intermediate tables
    let mut table = doc.as_table_mut();
    for &part in &parts[..parts.len() - 1] {
        if !table.contains_key(part) {
            table.insert(part, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        table = table[part]
            .as_table_mut()
            .ok_or_else(|| MinoError::User(format!("Expected table at key: {}", part)))?;
    }

    let leaf = *parts.last().unwrap();

    // Keys that store as arrays
    let is_list_key =
        key.ends_with("network_allow") || key.ends_with("layers") || key.ends_with("volumes");

    if is_list_key {
        let mut arr = toml_edit::Array::new();
        for item in value.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            arr.push(item);
        }
        table.insert(leaf, toml_edit::value(arr));
    } else if value == "true" || value == "false" {
        table.insert(leaf, toml_edit::value(parse_bool(value)?));
    } else if let Ok(n) = value.parse::<i64>() {
        table.insert(leaf, toml_edit::value(n));
    } else {
        table.insert(leaf, toml_edit::value(value));
    }

    Ok(())
}

fn parse_bool(value: &str) -> MinoResult<bool> {
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(MinoError::User(format!(
            "Invalid boolean value: {}. Use true/false",
            value
        ))),
    }
}

fn parse_u32(value: &str) -> MinoResult<u32> {
    value
        .parse()
        .map_err(|_| MinoError::User(format!("Invalid number: {}", value)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_toml_edit_value_preserves_comments() {
        let input = "# Top-level comment\n[container]\n# Network comment\nnetwork = \"none\"\n";
        let mut doc: toml_edit::DocumentMut = input.parse().unwrap();
        set_toml_edit_value(&mut doc, "container.image", "fedora:43").unwrap();
        let output = doc.to_string();
        assert!(output.contains("# Top-level comment"), "top comment lost");
        assert!(output.contains("# Network comment"), "inline comment lost");
        assert!(output.contains("network = \"none\""), "existing value lost");
        assert!(
            output.contains("image = \"fedora:43\""),
            "new value missing"
        );
    }

    #[test]
    fn set_toml_edit_value_creates_intermediate_tables() {
        let mut doc = toml_edit::DocumentMut::new();
        set_toml_edit_value(&mut doc, "credentials.aws.enabled", "true").unwrap();
        let output = doc.to_string();
        let parsed: toml::Value = output.parse().unwrap();
        assert!(parsed["credentials"]["aws"]["enabled"].as_bool().unwrap());
    }

    #[test]
    fn set_toml_edit_value_handles_list_keys() {
        let mut doc = toml_edit::DocumentMut::new();
        set_toml_edit_value(
            &mut doc,
            "container.network_allow",
            "github.com:443,npmjs.org:443",
        )
        .unwrap();
        let output = doc.to_string();
        let parsed: toml::Value = output.parse().unwrap();
        let arr = parsed["container"]["network_allow"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str().unwrap(), "github.com:443");
        assert_eq!(arr[1].as_str().unwrap(), "npmjs.org:443");
    }

    #[test]
    fn set_toml_edit_value_handles_integer() {
        let mut doc = toml_edit::DocumentMut::new();
        set_toml_edit_value(&mut doc, "session.auto_cleanup_hours", "48").unwrap();
        let output = doc.to_string();
        let parsed: toml::Value = output.parse().unwrap();
        assert_eq!(
            parsed["session"]["auto_cleanup_hours"]
                .as_integer()
                .unwrap(),
            48
        );
    }

    #[test]
    fn validate_config_key_rejects_unknown() {
        assert!(validate_config_key("container.nonexistent").is_err());
    }

    #[test]
    fn validate_config_key_accepts_known() {
        assert!(validate_config_key("container.network").is_ok());
        assert!(validate_config_key("credentials.aws.enabled").is_ok());
    }
}
