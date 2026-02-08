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
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{} {}", style("Error:").red().bold(), e);
            if let Some(hint) = e.hint() {
                eprintln!("{} {}", style("Hint:").yellow(), hint);
            }
            ExitCode::FAILURE
        }
    }
}

async fn run() -> MinoResult<()> {
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

    // Init command doesn't need config loading
    if let Commands::Init(args) = cli.command {
        return mino::cli::commands::init(args).await;
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

    let config = config_manager
        .load_merged(local_config_path.as_deref())
        .await?;

    // Ensure state directories exist
    ConfigManager::ensure_state_dirs().await?;

    // Dispatch to command
    match cli.command {
        Commands::Init(_) => unreachable!("Init handled above"),
        Commands::Run(args) => mino::cli::commands::run(args, &config).await,
        Commands::List(args) => mino::cli::commands::list(args, &config).await,
        Commands::Stop(args) => mino::cli::commands::stop(args, &config).await,
        Commands::Logs(args) => mino::cli::commands::logs(args, &config).await,
        Commands::Status => mino::cli::commands::status(&config).await,
        Commands::Setup(args) => mino::cli::commands::setup(args, &config).await,
        Commands::Config(args) => mino::cli::commands::config(args, &config).await,
        Commands::Cache(args) => mino::cli::commands::cache(args, &config).await,
    }
}
