//! Minotaur - Secure AI Agent Sandbox Wrapper
//!
//! CLI entry point that dispatches to subcommands.

use clap::Parser;
use console::style;
use minotaur::cli::{Cli, Commands};
use minotaur::config::ConfigManager;
use minotaur::error::MinotaurResult;
use std::process::ExitCode;
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

    // Initialize logging
    let filter = if cli.verbose {
        EnvFilter::new("minotaur=debug")
    } else {
        EnvFilter::new("minotaur=info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();

    // Load configuration
    let config_manager = if let Some(ref path) = cli.config {
        ConfigManager::with_path(path.clone())
    } else {
        ConfigManager::new()
    };

    let config = config_manager.load().await?;

    // Ensure state directories exist
    ConfigManager::ensure_state_dirs().await?;

    // Dispatch to command
    match cli.command {
        Commands::Run(args) => {
            minotaur::cli::commands::run(args, &config).await
        }
        Commands::List(args) => {
            minotaur::cli::commands::list(args, &config).await
        }
        Commands::Stop(args) => {
            minotaur::cli::commands::stop(args, &config).await
        }
        Commands::Logs(args) => {
            minotaur::cli::commands::logs(args, &config).await
        }
        Commands::Status => {
            minotaur::cli::commands::status(&config).await
        }
        Commands::Config(args) => {
            minotaur::cli::commands::config(args, &config).await
        }
        Commands::Cache(args) => {
            minotaur::cli::commands::cache(args, &config).await
        }
    }
}
