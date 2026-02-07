//! Config command - show or edit configuration

use crate::cli::args::{ConfigAction, ConfigArgs};
use crate::config::{Config, ConfigManager};
use crate::error::MinotaurResult;
use crate::ui::{self, UiContext};

/// Execute the config command
pub async fn execute(args: ConfigArgs, config: &Config) -> MinotaurResult<()> {
    let manager = ConfigManager::new();

    match args.action {
        None | Some(ConfigAction::Show) => show_config(config),
        Some(ConfigAction::Path) => show_path(&manager),
        Some(ConfigAction::Init { force }) => init_config(&manager, force).await?,
        Some(ConfigAction::Set { key, value }) => set_value(&manager, config, &key, &value).await?,
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

async fn init_config(manager: &ConfigManager, force: bool) -> MinotaurResult<()> {
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
) -> MinotaurResult<()> {
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

        ["credentials", "gcp", "project"] => {
            config.credentials.gcp.project = Some(value.to_string())
        }

        ["credentials", "azure", "subscription"] => {
            config.credentials.azure.subscription = Some(value.to_string())
        }
        ["credentials", "azure", "tenant"] => {
            config.credentials.azure.tenant = Some(value.to_string())
        }

        ["session", "shell"] => config.session.shell = value.to_string(),
        ["session", "auto_cleanup_hours"] => {
            config.session.auto_cleanup_hours = parse_u32(value)?
        }

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

fn parse_bool(value: &str) -> MinotaurResult<bool> {
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(crate::error::MinotaurError::User(format!(
            "Invalid boolean value: {}. Use true/false",
            value
        ))),
    }
}

fn parse_u32(value: &str) -> MinotaurResult<u32> {
    value
        .parse()
        .map_err(|_| crate::error::MinotaurError::User(format!("Invalid number: {}", value)))
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
        "credentials.aws.session_duration_secs",
        "credentials.aws.role_arn",
        "credentials.aws.profile",
        "credentials.aws.region",
        "credentials.gcp.project",
        "credentials.azure.subscription",
        "credentials.azure.tenant",
        "session.shell",
        "session.auto_cleanup_hours",
    ];

    for key in keys {
        eprintln!("  {}", key);
    }
}
