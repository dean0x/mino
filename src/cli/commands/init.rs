//! Init command - create project-local .mino.toml

use crate::cli::args::InitArgs;
use crate::error::{MinoError, MinoResult};
use crate::ui::{self, UiContext};
use std::path::Path;
use tokio::fs;

/// Template for project-local config
const INIT_TEMPLATE: &str = r#"# Mino project configuration
# Settings here override your global config (~/.config/mino/config.toml)
# Docs: https://github.com/dean0x/mino

[container]
# image = "typescript"
# layers = ["rust", "typescript"]
# network = "host"                   # host, none, bridge
# network_allow = ["github.com:443"] # implies bridge + iptables
# workdir = "/workspace"

# [credentials.aws]
# enabled = true
# region = "us-west-2"
# profile = "default"

# [credentials.gcp]
# enabled = true
# project = "my-project"

# [credentials.azure]
# enabled = true

[session]
# shell = "/bin/zsh"
"#;

/// Execute the init command
pub async fn execute(args: InitArgs) -> MinoResult<()> {
    let ctx = UiContext::detect();

    let target_dir = match args.path {
        Some(ref p) => p.clone(),
        None => {
            std::env::current_dir().map_err(|e| MinoError::io("getting current directory", e))?
        }
    };

    let config_path = target_dir.join(".mino.toml");

    if config_path.exists() && !args.force {
        return Err(MinoError::User(format!(
            "{} already exists. Use --force to overwrite.",
            config_path.display()
        )));
    }

    ensure_dir(&target_dir).await?;

    fs::write(&config_path, INIT_TEMPLATE)
        .await
        .map_err(|e| MinoError::io(format!("writing {}", config_path.display()), e))?;

    ui::step_ok_detail(
        &ctx,
        "Created project config",
        &config_path.display().to_string(),
    );

    Ok(())
}

async fn ensure_dir(dir: &Path) -> MinoResult<()> {
    if !dir.exists() {
        fs::create_dir_all(dir)
            .await
            .map_err(|e| MinoError::io(format!("creating directory {}", dir.display()), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn init_creates_config() {
        let temp = TempDir::new().unwrap();
        let args = InitArgs {
            force: false,
            path: Some(temp.path().to_path_buf()),
        };
        execute(args).await.unwrap();

        let content = std::fs::read_to_string(temp.path().join(".mino.toml")).unwrap();
        assert!(content.contains("[container]"));
        assert!(content.contains("credentials.aws"));
        assert!(content.contains("[session]"));
    }

    #[tokio::test]
    async fn init_refuses_overwrite_without_force() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join(".mino.toml"), "existing").unwrap();

        let args = InitArgs {
            force: false,
            path: Some(temp.path().to_path_buf()),
        };
        let result = execute(args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("already exists"));
    }

    #[tokio::test]
    async fn init_overwrites_with_force() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join(".mino.toml"), "old content").unwrap();

        let args = InitArgs {
            force: true,
            path: Some(temp.path().to_path_buf()),
        };
        execute(args).await.unwrap();

        let content = std::fs::read_to_string(temp.path().join(".mino.toml")).unwrap();
        assert!(content.contains("[container]"));
    }

    #[test]
    fn template_is_valid_toml() {
        // The template has commented-out lines; uncommented lines must parse
        let _: toml::Value = toml::from_str(INIT_TEMPLATE).unwrap();
    }
}
