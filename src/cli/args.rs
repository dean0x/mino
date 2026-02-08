//! CLI argument definitions using clap derive

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Minotaur - Secure AI Agent Sandbox
///
/// Wraps any command in rootless containers with temporary cloud
/// credentials and SSH agent forwarding.
#[derive(Parser, Debug)]
#[command(name = "minotaur")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Subcommand to execute
    #[command(subcommand)]
    pub command: Commands,

    /// Increase verbosity (-v info, -vv debug)
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Configuration file path
    #[arg(short, long, global = true, env = "MINOTAUR_CONFIG")]
    pub config: Option<PathBuf>,

    /// Skip local .minotaur.toml discovery
    #[arg(long, global = true)]
    pub no_local: bool,
}

/// Available commands
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start a sandboxed session
    Run(RunArgs),

    /// Initialize a project-local .minotaur.toml config
    Init(InitArgs),

    /// List active sessions
    List(ListArgs),

    /// Stop a running session
    Stop(StopArgs),

    /// View session logs
    Logs(LogsArgs),

    /// Check system health and dependencies
    Status,

    /// Interactive setup wizard - install prerequisites
    Setup(SetupArgs),

    /// Show or edit configuration
    Config(ConfigArgs),

    /// Manage dependency caches
    Cache(CacheArgs),
}

/// Arguments for the setup command
#[derive(Parser, Debug)]
pub struct SetupArgs {
    /// Auto-approve all installation prompts
    #[arg(short, long)]
    pub yes: bool,

    /// Check prerequisites only, don't install
    #[arg(long)]
    pub check: bool,

    /// Upgrade existing dependencies to latest versions
    #[arg(long)]
    pub upgrade: bool,
}

/// Arguments for the init command
#[derive(Parser, Debug)]
pub struct InitArgs {
    /// Overwrite existing .minotaur.toml
    #[arg(short, long)]
    pub force: bool,

    /// Target directory (defaults to current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,
}

/// Arguments for the run command
#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Session name (auto-generated if not provided)
    #[arg(short, long)]
    pub name: Option<String>,

    /// Project directory to mount (defaults to current directory)
    #[arg(short, long)]
    pub project: Option<PathBuf>,

    /// Include AWS credentials
    #[arg(long)]
    pub aws: bool,

    /// Include GCP credentials
    #[arg(long)]
    pub gcp: bool,

    /// Include Azure credentials
    #[arg(long)]
    pub azure: bool,

    /// Include all cloud credentials
    #[arg(long, conflicts_with_all = ["aws", "gcp", "azure"])]
    pub all_clouds: bool,

    /// Forward SSH agent
    #[arg(long, default_value = "true")]
    pub ssh_agent: bool,

    /// Include GitHub token
    #[arg(long, default_value = "true")]
    pub github: bool,

    /// Container image to use
    #[arg(long)]
    pub image: Option<String>,

    /// Composable layers to combine (comma-separated)
    #[arg(long, value_delimiter = ',', conflicts_with = "image")]
    pub layers: Vec<String>,

    /// Additional environment variables (KEY=VALUE)
    #[arg(short, long, value_parser = parse_env_var)]
    pub env: Vec<(String, String)>,

    /// Additional volume mounts (host:container)
    #[arg(long)]
    pub volume: Vec<String>,

    /// Run in detached mode
    #[arg(short, long)]
    pub detach: bool,

    /// Disable dependency caching for this session
    #[arg(long)]
    pub no_cache: bool,

    /// Force fresh cache (ignore existing caches)
    #[arg(long, conflicts_with = "no_cache")]
    pub cache_fresh: bool,

    /// Command and arguments to run (defaults to shell)
    #[arg(last = true)]
    pub command: Vec<String>,
}

/// Arguments for the list command
#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Show all sessions including stopped
    #[arg(short, long)]
    pub all: bool,

    /// Output format
    #[arg(short, long, default_value = "table")]
    pub format: OutputFormat,
}

/// Arguments for the stop command
#[derive(Parser, Debug)]
pub struct StopArgs {
    /// Session name or ID
    pub session: String,

    /// Force stop without cleanup
    #[arg(short, long)]
    pub force: bool,
}

/// Arguments for the logs command
#[derive(Parser, Debug)]
pub struct LogsArgs {
    /// Session name or ID
    pub session: String,

    /// Follow log output
    #[arg(short, long)]
    pub follow: bool,

    /// Number of lines to show (0 = all)
    #[arg(short, long, default_value = "100")]
    pub lines: u32,
}

/// Arguments for the config command
#[derive(Parser, Debug)]
pub struct ConfigArgs {
    /// Subcommand for config
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

/// Config subcommands
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Show current configuration
    Show,

    /// Show configuration file path
    Path,

    /// Initialize default configuration
    Init {
        /// Overwrite existing configuration
        #[arg(short, long)]
        force: bool,
    },

    /// Set a configuration value
    Set {
        /// Configuration key (e.g., vm.name)
        key: String,
        /// Value to set
        value: String,
        /// Write to project-local .minotaur.toml instead of global config
        #[arg(long)]
        local: bool,
    },
}

/// Output format for list command
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table
    Table,
    /// JSON output
    Json,
    /// Simple text (one per line)
    Plain,
}

/// Arguments for the cache command
#[derive(Parser, Debug)]
pub struct CacheArgs {
    /// Subcommand for cache
    #[command(subcommand)]
    pub action: CacheAction,
}

/// Cache subcommands
#[derive(Subcommand, Debug)]
pub enum CacheAction {
    /// List all cache volumes
    List {
        /// Output format
        #[arg(short, long, default_value = "table")]
        format: OutputFormat,
    },

    /// Show cache info for current project
    Info {
        /// Project directory (defaults to current directory)
        #[arg(short, long)]
        project: Option<PathBuf>,
    },

    /// Remove orphaned and old caches
    Gc {
        /// Remove caches older than N days (default: from config)
        #[arg(long)]
        days: Option<u32>,

        /// Dry run - show what would be removed
        #[arg(long)]
        dry_run: bool,
    },

    /// Clear caches
    Clear {
        /// Clear all cache volumes
        #[arg(long, required_unless_present = "images")]
        all: bool,

        /// Clear composed layer images only
        #[arg(long, required_unless_present = "all")]
        images: bool,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Parse environment variable in KEY=VALUE format
fn parse_env_var(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE format: no '=' found in '{s}'"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_var_valid() {
        let (k, v) = parse_env_var("FOO=bar").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "bar");
    }

    #[test]
    fn parse_env_var_with_equals() {
        let (k, v) = parse_env_var("FOO=bar=baz").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "bar=baz");
    }

    #[test]
    fn parse_env_var_invalid() {
        assert!(parse_env_var("FOO").is_err());
    }

    #[test]
    fn cli_parses_run() {
        let cli = Cli::parse_from(["minotaur", "run", "--aws", "--", "bash"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.aws);
                assert_eq!(args.command, vec!["bash"]);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn cli_parses_status() {
        let cli = Cli::parse_from(["minotaur", "status"]);
        assert!(matches!(cli.command, Commands::Status));
    }

    #[test]
    fn cli_parses_setup() {
        let cli = Cli::parse_from(["minotaur", "setup"]);
        match cli.command {
            Commands::Setup(args) => {
                assert!(!args.yes);
                assert!(!args.check);
            }
            _ => panic!("expected Setup command"),
        }
    }

    #[test]
    fn cli_parses_setup_with_flags() {
        let cli = Cli::parse_from(["minotaur", "setup", "--yes", "--check"]);
        match cli.command {
            Commands::Setup(args) => {
                assert!(args.yes);
                assert!(args.check);
                assert!(!args.upgrade);
            }
            _ => panic!("expected Setup command"),
        }
    }

    #[test]
    fn cli_parses_setup_upgrade() {
        let cli = Cli::parse_from(["minotaur", "setup", "--upgrade"]);
        match cli.command {
            Commands::Setup(args) => {
                assert!(!args.yes);
                assert!(!args.check);
                assert!(args.upgrade);
            }
            _ => panic!("expected Setup command"),
        }
    }

    #[test]
    fn cli_parses_init() {
        let cli = Cli::parse_from(["minotaur", "init"]);
        assert!(matches!(cli.command, Commands::Init(_)));
    }

    #[test]
    fn cli_parses_init_force() {
        let cli = Cli::parse_from(["minotaur", "init", "--force"]);
        match cli.command {
            Commands::Init(args) => assert!(args.force),
            _ => panic!("expected Init command"),
        }
    }

    #[test]
    fn cli_no_local_flag() {
        let cli = Cli::parse_from(["minotaur", "--no-local", "status"]);
        assert!(cli.no_local);
    }

    #[test]
    fn cli_verbose_levels() {
        let cli = Cli::parse_from(["minotaur", "status"]);
        assert_eq!(cli.verbose, 0);

        let cli = Cli::parse_from(["minotaur", "-v", "status"]);
        assert_eq!(cli.verbose, 1);

        let cli = Cli::parse_from(["minotaur", "-vv", "status"]);
        assert_eq!(cli.verbose, 2);
    }
}
