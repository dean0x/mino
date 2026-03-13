//! Mino - Secure AI Agent Sandbox Wrapper
//!
//! CLI entry point that dispatches to subcommands.

use clap::Parser;
use console::style;
use mino::cli::{Cli, Commands};
use mino::config::ConfigManager;
use mino::error::MinoResult;
use std::process::ExitCode;
use tracing::debug;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {}", style("Error:").red().bold(), e);
            if let Some(hint) = e.hint() {
                eprintln!("{} {}", style("Hint:").yellow(), hint);
            }
            ExitCode::FAILURE
        }
    }
}

async fn run() -> MinoResult<ExitCode> {
    let cli = Cli::parse();

    // Initialize logging: 0 = warn (spinners only), 1 = info, 2+ = debug
    let filter = match cli.verbose {
        0 => EnvFilter::new("mino=warn"),
        1 => EnvFilter::new("mino=info"),
        _ => EnvFilter::new("mino=debug"),
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();

    // Commands that don't need config loading
    if let Commands::Init(args) = cli.command {
        mino::cli::commands::init(args).await?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Commands::Completions(args) = cli.command {
        mino::cli::commands::completions(args).await?;
        return Ok(ExitCode::SUCCESS);
    }

    // Load configuration
    let config_manager = if let Some(ref path) = cli.config {
        ConfigManager::with_path(path.clone())
    } else {
        ConfigManager::new()
    };

    // Find local config unless --no-local is set
    let local_config_path = if cli.no_local {
        debug!("Local config discovery disabled (--no-local)");
        None
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| mino::error::MinoError::io("getting current directory", e))?;
        let found = ConfigManager::find_local_config(&cwd);
        if let Some(ref path) = found {
            debug!("Found local config: {}", path.display());
        }
        found
    };

    // Trust gate: verify local config before merging
    let local_config_path = match local_config_path {
        Some(path) => {
            let ctx = mino::ui::UiContext::detect();
            mino::config::trust::verify_local_config(&path, &ctx, cli.trust_local).await?
        }
        None => None,
    };

    let config = config_manager
        .load_merged(local_config_path.as_deref())
        .await?;

    // Ensure state directories exist
    ConfigManager::ensure_state_dirs().await?;

    // Dispatch to command
    match cli.command {
        Commands::Init(_) | Commands::Completions(_) => unreachable!("handled above"),
        Commands::Exec(args) => mino::cli::commands::exec(args, &config).await,
        Commands::Run(args) => { mino::cli::commands::run(args, &config).await?; Ok(ExitCode::SUCCESS) }
        Commands::List(args) => { mino::cli::commands::list(args, &config).await?; Ok(ExitCode::SUCCESS) }
        Commands::Stop(args) => { mino::cli::commands::stop(args, &config).await?; Ok(ExitCode::SUCCESS) }
        Commands::Logs(args) => { mino::cli::commands::logs(args, &config).await?; Ok(ExitCode::SUCCESS) }
        Commands::Status => { mino::cli::commands::status(&config).await?; Ok(ExitCode::SUCCESS) }
        Commands::Setup(args) => { mino::cli::commands::setup(args, &config).await?; Ok(ExitCode::SUCCESS) }
        Commands::Config(args) => { mino::cli::commands::config(args, &config).await?; Ok(ExitCode::SUCCESS) }
        Commands::Cache(args) => { mino::cli::commands::cache(args, &config).await?; Ok(ExitCode::SUCCESS) }
    }
}
