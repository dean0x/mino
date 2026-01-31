//! Run command - start a sandboxed session

use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::credentials::{AwsCredentials, AzureCredentials, CredentialCache, GcpCredentials, GithubCredentials};
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::{create_runtime, ContainerConfig, Platform};
use crate::session::{Session, SessionManager, SessionStatus};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Execute the run command
pub async fn execute(args: RunArgs, config: &Config) -> MinotaurResult<()> {
    let pb = create_progress_bar("Initializing sandbox...");

    // Create platform-appropriate runtime
    let runtime = create_runtime(config)?;
    debug!("Using runtime: {}", runtime.runtime_name());

    // Validate environment (platform-specific checks)
    pb.set_message(format!("Checking {}...", runtime.runtime_name()));
    validate_environment().await?;

    // Determine project directory
    let project_dir = resolve_project_dir(&args, config)?;
    debug!("Project directory: {}", project_dir.display());

    // Ensure runtime is ready
    pb.set_message(format!("Starting {}...", runtime.runtime_name()));
    runtime.ensure_ready().await?;

    // Collect credentials
    pb.set_message("Gathering credentials...");
    let credentials = gather_credentials(&args, config).await?;

    // Create session
    let session_name = args.name.clone().unwrap_or_else(generate_session_name);
    let manager = SessionManager::new().await?;

    if manager.get(&session_name).await?.is_some() {
        return Err(MinotaurError::SessionExists(session_name));
    }

    // Build container config
    let container_config = build_container_config(&args, config, &project_dir, credentials)?;

    // Determine command to run
    let command = if args.command.is_empty() {
        vec![config.session.shell.clone()]
    } else {
        args.command.clone()
    };

    // Create session record
    let session = Session::new(
        session_name.clone(),
        project_dir.clone(),
        command.clone(),
        SessionStatus::Starting,
    );
    manager.create(&session).await?;

    pb.set_message("Starting container...");

    // Start container
    let container_id = match runtime.run(&container_config, &command).await {
        Ok(id) => id,
        Err(e) => {
            manager.update_status(&session_name, SessionStatus::Failed).await?;
            return Err(e);
        }
    };

    // Update session with container ID
    manager.set_container_id(&session_name, &container_id).await?;
    manager.update_status(&session_name, SessionStatus::Running).await?;

    pb.finish_and_clear();

    if args.detach {
        println!(
            "{} Session {} started (container: {})",
            style("✓").green(),
            style(&session_name).cyan(),
            &container_id[..12]
        );
        println!("  Attach with: minotaur logs {}", session_name);
        println!("  Stop with:   minotaur stop {}", session_name);
    } else {
        info!("Attaching to container {}", &container_id[..12]);

        // Attach to container interactively
        let exit_code = runtime.attach(&container_id).await?;

        // Clean up session
        manager.update_status(&session_name, SessionStatus::Stopped).await?;

        if exit_code != 0 {
            println!(
                "{} Session exited with code {}",
                style("!").yellow(),
                exit_code
            );
        }
    }

    Ok(())
}

async fn validate_environment() -> MinotaurResult<()> {
    match Platform::detect() {
        Platform::MacOS => {
            // On macOS, check OrbStack
            use crate::orchestration::OrbStack;
            if !OrbStack::is_installed().await {
                return Err(MinotaurError::OrbStackNotFound);
            }
            if !OrbStack::is_running().await? {
                return Err(MinotaurError::OrbStackNotRunning);
            }
        }
        Platform::Linux => {
            // On Linux, basic checks are done in ensure_ready()
        }
        Platform::Unsupported => {
            return Err(MinotaurError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            ));
        }
    }
    Ok(())
}

fn resolve_project_dir(args: &RunArgs, config: &Config) -> MinotaurResult<PathBuf> {
    if let Some(ref path) = args.project {
        let canonical = path.canonicalize().map_err(|e| MinotaurError::io(
            format!("resolving project path {}", path.display()),
            e,
        ))?;
        return Ok(canonical);
    }

    if let Some(ref path) = config.session.default_project_dir {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    env::current_dir().map_err(|e| MinotaurError::io("getting current directory", e))
}

async fn gather_credentials(
    args: &RunArgs,
    config: &Config,
) -> MinotaurResult<HashMap<String, String>> {
    let mut env_vars = HashMap::new();
    let cache = CredentialCache::new().await?;

    let (use_aws, use_gcp, use_azure) = if args.all_clouds {
        (true, true, true)
    } else {
        (args.aws, args.gcp, args.azure)
    };

    // AWS credentials
    if use_aws {
        debug!("Fetching AWS credentials...");
        match AwsCredentials::get_session_token(&config.credentials.aws, &cache).await {
            Ok(creds) => {
                env_vars.insert("AWS_ACCESS_KEY_ID".to_string(), creds.access_key_id);
                env_vars.insert("AWS_SECRET_ACCESS_KEY".to_string(), creds.secret_access_key);
                if let Some(token) = creds.session_token {
                    env_vars.insert("AWS_SESSION_TOKEN".to_string(), token);
                }
                if let Some(region) = &config.credentials.aws.region {
                    env_vars.insert("AWS_REGION".to_string(), region.clone());
                    env_vars.insert("AWS_DEFAULT_REGION".to_string(), region.clone());
                }
                info!("AWS credentials loaded");
            }
            Err(e) => {
                eprintln!("{} AWS: {}", style("!").yellow(), e);
            }
        }
    }

    // GCP credentials
    if use_gcp {
        debug!("Fetching GCP credentials...");
        match GcpCredentials::get_access_token(&config.credentials.gcp, &cache).await {
            Ok(token) => {
                env_vars.insert("CLOUDSDK_AUTH_ACCESS_TOKEN".to_string(), token);
                if let Some(project) = &config.credentials.gcp.project {
                    env_vars.insert("CLOUDSDK_CORE_PROJECT".to_string(), project.clone());
                }
                info!("GCP credentials loaded");
            }
            Err(e) => {
                eprintln!("{} GCP: {}", style("!").yellow(), e);
            }
        }
    }

    // Azure credentials
    if use_azure {
        debug!("Fetching Azure credentials...");
        match AzureCredentials::get_access_token(&config.credentials.azure, &cache).await {
            Ok(token) => {
                env_vars.insert("AZURE_ACCESS_TOKEN".to_string(), token);
                info!("Azure credentials loaded");
            }
            Err(e) => {
                eprintln!("{} Azure: {}", style("!").yellow(), e);
            }
        }
    }

    // GitHub token
    if args.github {
        debug!("Fetching GitHub token...");
        match GithubCredentials::get_token(&config.credentials.github).await {
            Ok(token) => {
                env_vars.insert("GITHUB_TOKEN".to_string(), token);
                env_vars.insert("GH_TOKEN".to_string(), env_vars["GITHUB_TOKEN"].clone());
                info!("GitHub token loaded");
            }
            Err(e) => {
                debug!("GitHub token not available: {}", e);
            }
        }
    }

    // Add user-specified env vars
    for (key, value) in &args.env {
        env_vars.insert(key.clone(), value.clone());
    }

    Ok(env_vars)
}

fn build_container_config(
    args: &RunArgs,
    config: &Config,
    project_dir: &Path,
    env_vars: HashMap<String, String>,
) -> MinotaurResult<ContainerConfig> {
    let image = args.image.clone().unwrap_or_else(|| config.container.image.clone());

    let mut volumes = vec![
        // Mount project directory
        format!("{}:{}", project_dir.display(), config.container.workdir),
    ];

    // Add SSH agent socket if available and requested
    if args.ssh_agent {
        if let Ok(sock) = env::var("SSH_AUTH_SOCK") {
            volumes.push(format!("{}:/ssh-agent", sock));
        }
    }

    // Add user-specified volumes
    for vol in &args.volume {
        volumes.push(vol.clone());
    }

    // Add config volumes
    for vol in &config.container.volumes {
        volumes.push(vol.clone());
    }

    let mut final_env = config.container.env.clone();
    final_env.extend(env_vars);

    // Set SSH_AUTH_SOCK inside container
    if args.ssh_agent && env::var("SSH_AUTH_SOCK").is_ok() {
        final_env.insert("SSH_AUTH_SOCK".to_string(), "/ssh-agent".to_string());
    }

    Ok(ContainerConfig {
        image,
        workdir: config.container.workdir.clone(),
        volumes,
        env: final_env,
        network: config.container.network.clone(),
        interactive: !args.detach,
        tty: !args.detach,
    })
}

fn generate_session_name() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("session-{}", timestamp % 100000)
}

fn create_progress_bar(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}
