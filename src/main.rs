//! Minotaur - Secure AI Agent Sandbox Wrapper
//!
//! CLI entry point that dispatches to subcommands.

use clap::Parser;
use console::style;
use minotaur::cli::{Cli, Commands};
use minotaur::config::ConfigManager;
use minotaur::error::MinotaurResult;
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

async fn run() -> MinotaurResult<()> {
    let cli = Cli::parse();

    // Initialize logging: 0 = warn (spinners only), 1 = info, 2+ = debug
    let filter = match cli.verbose {
        0 => EnvFilter::new("minotaur=warn"),
        1 => EnvFilter::new("minotaur=info"),
        _ => EnvFilter::new("minotaur=debug"),
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();

    // Init command doesn't need config loading
    if let Commands::Init(args) = cli.command {
        return minotaur::cli::commands::init(args).await;
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
        let cwd =
            std::env::current_dir().map_err(|e| minotaur::error::MinotaurError::io("getting current directory", e))?;
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
        Commands::Run(args) => minotaur::cli::commands::run(args, &config).await,
        Commands::List(args) => minotaur::cli::commands::list(args, &config).await,
        Commands::Stop(args) => minotaur::cli::commands::stop(args, &config).await,
        Commands::Logs(args) => minotaur::cli::commands::logs(args, &config).await,
        Commands::Status => minotaur::cli::commands::status(&config).await,
        Commands::Setup(args) => minotaur::cli::commands::setup(args, &config).await,
        Commands::Config(args) => minotaur::cli::commands::config(args, &config).await,
        Commands::Cache(args) => minotaur::cli::commands::cache(args, &config).await,
    }
}
