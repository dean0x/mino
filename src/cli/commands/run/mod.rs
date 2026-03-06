//! Run command - start a sandboxed session

mod cache;
mod container;
mod credentials;
mod image;
mod prompts;

use self::cache::{check_cache_size_warning, finalize_caches, setup_caches};
use self::container::{build_container_config, ContainerBuildParams};
use self::credentials::gather_credentials;
use self::image::resolve_image;
use self::prompts::{is_default_network, prompt_network_selection};

use crate::audit::AuditLog;
use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::network::{
    generate_iptables_wrapper, resolve_network_mode, NetworkMode, NetworkResolutionInput,
};
use crate::orchestration::{create_runtime, ContainerConfig, ContainerRuntime, Platform};
use crate::session::{Session, SessionManager, SessionStatus};
use crate::ui::{self, TaskSpinner, UiContext};
use console::style;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, warn};
use uuid::Uuid;

/// Tracks cache volumes created during this session (for finalization)
#[derive(Default)]
struct CacheSession {
    volumes_to_finalize: Vec<String>,
}

/// Result of resolving the image to use
struct ImageResolution {
    /// Final image tag to use
    image: String,
    /// Extra env vars from layers (empty if using single image)
    layer_env: HashMap<String, String>,
}

/// Execute the run command
pub async fn execute(args: RunArgs, config: &Config) -> MinoResult<()> {
    let ctx = UiContext::detect();
    let mut spinner = TaskSpinner::new(&ctx);

    spinner.start("Initializing sandbox...");

    let runtime: Arc<dyn ContainerRuntime> = Arc::from(create_runtime(config)?);
    debug!("Using runtime: {}", runtime.runtime_name());

    spinner.message(&format!("Checking {}...", runtime.runtime_name()));
    validate_environment().await?;

    let project_dir = resolve_project_dir(&args, config)?;
    debug!("Project directory: {}", project_dir.display());

    spinner.message(&format!("Starting {}...", runtime.runtime_name()));
    runtime.ensure_ready().await?;

    let (resolution, using_layers) =
        resolve_image(&args, config, &ctx, &mut spinner, &*runtime, &project_dir).await?;

    let network_mode = if is_default_network(&args, config) && ctx.is_interactive() {
        spinner.clear();
        let mode = prompt_network_selection(&ctx, &project_dir).await?;
        spinner.start("Initializing sandbox...");
        mode
    } else {
        resolve_network_mode(&NetworkResolutionInput {
            cli_network: args.network.as_deref(),
            cli_allow_rules: &args.network_allow,
            cli_preset: args.network_preset.as_deref(),
            config_network: &config.container.network,
            config_network_allow: &config.container.network_allow,
            config_preset: config.container.network_preset.as_deref(),
        })?
    };
    debug!("Network mode: {:?}", network_mode);

    spinner.message("Setting up caches...");
    let (cache_mounts, cache_env, cache_session) =
        setup_caches(&*runtime, &args, config, &project_dir).await?;

    if !args.no_cache && config.cache.enabled {
        check_cache_size_warning(&*runtime, config).await;
    }

    spinner.message("Gathering credentials...");
    let (credentials, active_providers, cred_failures) = gather_credentials(&args, config).await?;
    if !cred_failures.is_empty() {
        spinner.stop("Credentials");
        for (provider, error) in &cred_failures {
            ui::step_warn(&ctx, &format!("{}: {}", provider, error));
        }
        if args.strict_credentials {
            return Err(MinoError::User(format!(
                "Credential loading failed for: {}. Remove --strict-credentials to continue anyway.",
                cred_failures
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        spinner.start("Initializing sandbox...");
    }

    let session_name = args.name.clone().unwrap_or_else(generate_session_name);
    let manager = SessionManager::new().await?;

    if config.session.auto_cleanup_hours > 0 {
        let cleaned = manager.cleanup(config.session.auto_cleanup_hours).await?;
        if cleaned > 0 {
            debug!("Cleaned up {} old session(s)", cleaned);
        }
    }

    let audit = AuditLog::new(config);

    let container_config = build_container_config(&ContainerBuildParams {
        args: &args,
        config,
        project_dir: &project_dir,
        resolution: &resolution,
        env_vars: credentials,
        cache_mounts: &cache_mounts,
        cache_env,
        network_mode: &network_mode,
    })?;

    // Layers compose on mino-base which has Oh My Zsh configured
    let command = if args.command.is_empty() {
        if using_layers {
            vec!["/bin/zsh".to_string()]
        } else {
            vec![config.session.shell.clone()]
        }
    } else {
        args.command.clone()
    };

    let command = if let NetworkMode::Allow(ref rules) = network_mode {
        generate_iptables_wrapper(rules, &command)
    } else {
        command
    };

    let session = Session::new(
        session_name.clone(),
        project_dir.clone(),
        command.clone(),
        SessionStatus::Starting,
    );
    manager.create(&session).await?;

    audit
        .log(
            "session.created",
            &serde_json::json!({
                "name": &session_name,
                "project_dir": project_dir.display().to_string(),
                "image": &container_config.image,
                "command": &command,
                "network": format!("{:?}", network_mode),
            }),
        )
        .await;

    if !active_providers.is_empty() {
        audit
            .log(
                "credentials.injected",
                &serde_json::json!({
                    "session_name": &session_name,
                    "providers": &active_providers,
                }),
            )
            .await;
    }

    if !runtime
        .image_exists(&container_config.image)
        .await
        .unwrap_or(false)
    {
        spinner.message(&format!("Pulling image {}...", container_config.image));
    } else {
        spinner.message("Starting container...");
    }

    let mut run_ctx = RunContext {
        runtime: &runtime,
        container_config: &container_config,
        command: &command,
        session_name: &session_name,
        manager: &manager,
        audit: &audit,
        spinner: &mut spinner,
    };

    if args.detach {
        run_detached(&mut run_ctx, cache_session).await?;
    } else {
        run_interactive(&mut run_ctx, cache_session).await?;
    }

    Ok(())
}

struct RunContext<'a> {
    runtime: &'a Arc<dyn ContainerRuntime>,
    container_config: &'a ContainerConfig,
    command: &'a [String],
    session_name: &'a str,
    manager: &'a SessionManager,
    audit: &'a AuditLog,
    spinner: &'a mut TaskSpinner,
}

impl RunContext<'_> {
    /// Record a container creation failure in session state and audit log, then return the error.
    async fn record_failure<T>(&self, error: MinoError) -> MinoResult<T> {
        self.manager
            .update_status(self.session_name, SessionStatus::Failed)
            .await?;
        self.audit
            .log(
                "session.failed",
                &serde_json::json!({
                    "name": self.session_name,
                    "error": error.to_string(),
                }),
            )
            .await;
        Err(error)
    }

    /// Record a successful container start in session state and audit log.
    async fn record_start(&self, container_id: &str) -> MinoResult<()> {
        self.manager
            .set_container_id(self.session_name, container_id)
            .await?;
        self.manager
            .update_status(self.session_name, SessionStatus::Running)
            .await?;
        self.audit
            .log(
                "session.started",
                &serde_json::json!({
                    "name": self.session_name,
                    "container_id": container_id,
                }),
            )
            .await;
        Ok(())
    }
}

/// Run container in detached mode with background cache finalization.
async fn run_detached(ctx: &mut RunContext<'_>, cache_session: CacheSession) -> MinoResult<()> {
    let container_id = match ctx.runtime.run(ctx.container_config, ctx.command).await {
        Ok(id) => id,
        Err(e) => return ctx.record_failure(e).await,
    };

    ctx.record_start(&container_id).await?;

    ctx.spinner.clear();

    println!(
        "{} Session {} started (container: {})",
        style("✓").green(),
        style(ctx.session_name).cyan(),
        &container_id[..12]
    );
    println!("  Attach with: mino logs {}", ctx.session_name);
    println!("  Stop with:   mino stop {}", ctx.session_name);

    // Spawn background monitor: waits for container exit, then finalizes caches
    if !cache_session.volumes_to_finalize.is_empty() {
        let bg_runtime = Arc::clone(ctx.runtime);
        let bg_container_id = container_id.clone();
        let bg_cache_session = cache_session;

        tokio::spawn(async move {
            let short_id = &bg_container_id[..12.min(bg_container_id.len())];
            debug!("Background monitor started for container {}", short_id);

            match bg_runtime.get_container_exit_code(&bg_container_id).await {
                Ok(Some(0)) => {
                    debug!("Container {} exited cleanly, finalizing caches", short_id);
                    finalize_caches(&bg_cache_session).await;
                }
                Ok(Some(code)) => {
                    debug!(
                        "Container {} exited with code {}, skipping cache finalization",
                        short_id, code
                    );
                }
                Ok(None) => {
                    warn!(
                        "Container {} exit code unknown, skipping cache finalization",
                        short_id
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to wait for container {}: {}, skipping cache finalization",
                        short_id, e
                    );
                }
            }
        });
    }

    Ok(())
}

/// Run container in interactive mode with synchronous cache finalization.
async fn run_interactive(ctx: &mut RunContext<'_>, cache_session: CacheSession) -> MinoResult<()> {
    let container_id = match ctx.runtime.create(ctx.container_config, ctx.command).await {
        Ok(id) => id,
        Err(e) => return ctx.record_failure(e).await,
    };

    ctx.record_start(&container_id).await?;

    ctx.spinner.clear();

    debug!("Starting container attached: {}", &container_id[..12]);

    let exit_code = ctx.runtime.start_attached(&container_id).await?;

    // Finalize caches on clean exit
    if exit_code == 0 && !cache_session.volumes_to_finalize.is_empty() {
        finalize_caches(&cache_session).await;
    }

    // Clean up session
    ctx.manager
        .update_status(ctx.session_name, SessionStatus::Stopped)
        .await?;

    // Remove stopped container to prevent credential persistence in `podman inspect`
    if let Err(e) = ctx.runtime.remove(&container_id).await {
        warn!(
            "Failed to remove container {}: {}",
            &container_id[..12.min(container_id.len())],
            e
        );
    }

    ctx.audit
        .log(
            "session.stopped",
            &serde_json::json!({
                "name": ctx.session_name,
                "exit_code": exit_code,
            }),
        )
        .await;

    if exit_code != 0 {
        println!(
            "{} Session exited with code {}",
            style("!").yellow(),
            exit_code
        );
    }

    Ok(())
}

async fn validate_environment() -> MinoResult<()> {
    match Platform::detect() {
        Platform::MacOS => {
            use crate::orchestration::OrbStack;
            if !OrbStack::is_installed().await {
                return Err(MinoError::OrbStackNotFound);
            }
            if !OrbStack::is_running().await? {
                return Err(MinoError::OrbStackNotRunning);
            }
        }
        Platform::Linux => {} // Checked in ensure_ready()
        Platform::Unsupported => {
            return Err(MinoError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            ));
        }
    }
    Ok(())
}

fn resolve_project_dir(args: &RunArgs, _config: &Config) -> MinoResult<PathBuf> {
    if let Some(ref path) = args.project {
        let canonical = path
            .canonicalize()
            .map_err(|e| MinoError::io(format!("resolving project path {}", path.display()), e))?;
        return Ok(canonical);
    }

    env::current_dir().map_err(|e| MinoError::io("getting current directory", e))
}

fn generate_session_name() -> String {
    let short_id = &Uuid::new_v4().to_string()[..8];
    format!("session-{}", short_id)
}

#[cfg(test)]
mod tests {
    use self::image::*;
    use self::prompts::{is_default_network, upsert_container_toml_key};
    use super::*;

    fn test_run_args() -> RunArgs {
        RunArgs {
            name: None,
            project: None,
            aws: false,
            gcp: false,
            azure: false,
            all_clouds: false,
            no_ssh_agent: false,
            no_github: false,
            strict_credentials: false,
            image: None,
            layers: vec![],
            env: vec![],
            volume: vec![],
            detach: false,
            no_cache: false,
            cache_fresh: false,
            network: None,
            network_allow: vec![],
            network_preset: None,
            command: vec![],
        }
    }

    #[test]
    fn image_alias_to_layer_typescript() {
        assert_eq!(image_alias_to_layer("typescript"), Some("typescript"));
        assert_eq!(image_alias_to_layer("ts"), Some("typescript"));
        assert_eq!(image_alias_to_layer("node"), Some("typescript"));
    }

    #[test]
    fn image_alias_to_layer_rust() {
        assert_eq!(image_alias_to_layer("rust"), Some("rust"));
        assert_eq!(image_alias_to_layer("cargo"), Some("rust"));
    }

    #[test]
    fn image_alias_to_layer_unknown() {
        assert_eq!(image_alias_to_layer("base"), None);
        assert_eq!(image_alias_to_layer("fedora"), None);
        assert_eq!(image_alias_to_layer("ghcr.io/foo/bar:latest"), None);
    }

    #[test]
    fn resolve_image_alias_base() {
        assert_eq!(
            resolve_image_alias("base"),
            "ghcr.io/dean0x/mino-base:latest"
        );
    }

    #[test]
    fn resolve_image_alias_passthrough_full_path() {
        assert_eq!(
            resolve_image_alias("ghcr.io/custom/image:v1"),
            "ghcr.io/custom/image:v1"
        );
        assert_eq!(
            resolve_image_alias("docker.io/library/fedora:43"),
            "docker.io/library/fedora:43"
        );
    }

    #[test]
    fn resolve_image_alias_passthrough_local() {
        assert_eq!(resolve_image_alias("my-local-image"), "my-local-image");
        assert_eq!(resolve_image_alias("fedora"), "fedora");
    }

    #[test]
    fn is_default_image_with_defaults() {
        let args = test_run_args();
        let config = Config::default();
        assert!(is_default_image(&args, &config));
    }

    #[test]
    fn is_default_image_with_custom_image_arg() {
        let mut args = test_run_args();
        args.image = Some("custom:latest".to_string());
        let config = Config::default();
        assert!(!is_default_image(&args, &config));
    }

    #[test]
    fn is_default_image_with_custom_config() {
        let args = test_run_args();
        let mut config = Config::default();
        config.container.image = "ubuntu:24.04".to_string();
        assert!(!is_default_image(&args, &config));
    }

    #[tokio::test]
    async fn upsert_creates_new_config() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join(".mino.toml");

        let mut layers = toml_edit::Array::new();
        layers.push("rust");
        layers.push("typescript");
        upsert_container_toml_key(&path, "layers", toml_edit::Value::Array(layers))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: toml::Value = content.parse().unwrap();
        let layers = parsed["container"]["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].as_str().unwrap(), "rust");
        assert_eq!(layers[1].as_str().unwrap(), "typescript");
    }

    #[test]
    fn parse_layers_env_basic() {
        assert_eq!(
            parse_layers_env("rust,typescript"),
            vec!["rust", "typescript"]
        );
    }

    #[test]
    fn parse_layers_env_whitespace() {
        assert_eq!(
            parse_layers_env(" rust , typescript "),
            vec!["rust", "typescript"]
        );
    }

    #[test]
    fn parse_layers_env_empty_segments() {
        assert_eq!(
            parse_layers_env("rust,,typescript,"),
            vec!["rust", "typescript"]
        );
    }

    #[test]
    fn parse_layers_env_empty_string() {
        let result = parse_layers_env("");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_layers_env_single() {
        assert_eq!(parse_layers_env("rust"), vec!["rust"]);
    }

    #[test]
    fn resolve_layer_names_cli_wins() {
        let mut args = test_run_args();
        args.layers = vec!["typescript".to_string()];
        let mut config = Config::default();
        config.container.layers = vec!["rust".to_string()];
        assert_eq!(
            resolve_layer_names(&args, &config),
            Some(vec!["typescript".to_string()])
        );
    }

    #[test]
    fn resolve_layer_names_image_blocks_all() {
        let mut args = test_run_args();
        args.image = Some("fedora:43".to_string());
        let mut config = Config::default();
        config.container.layers = vec!["rust".to_string()];
        assert_eq!(resolve_layer_names(&args, &config), None);
    }

    #[test]
    fn resolve_layer_names_config_layers() {
        let args = test_run_args();
        let mut config = Config::default();
        config.container.layers = vec!["rust".to_string()];
        // Clear MINO_LAYERS to avoid interference from environment
        std::env::remove_var("MINO_LAYERS");
        assert_eq!(
            resolve_layer_names(&args, &config),
            Some(vec!["rust".to_string()])
        );
    }

    #[test]
    fn resolve_layer_names_none_when_empty() {
        let args = test_run_args();
        let config = Config::default();
        std::env::remove_var("MINO_LAYERS");
        assert_eq!(resolve_layer_names(&args, &config), None);
    }

    #[tokio::test]
    async fn upsert_merges_existing_config() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join(".mino.toml");

        // Write existing config with other settings
        tokio::fs::write(
            &path,
            "[container]\nimage = \"custom:latest\"\nnetwork = \"none\"\n",
        )
        .await
        .unwrap();

        let mut layers = toml_edit::Array::new();
        layers.push("typescript");
        upsert_container_toml_key(&path, "layers", toml_edit::Value::Array(layers))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: toml::Value = content.parse().unwrap();
        // Layers added
        let layers = parsed["container"]["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].as_str().unwrap(), "typescript");
        // Existing fields preserved
        assert_eq!(
            parsed["container"]["image"].as_str().unwrap(),
            "custom:latest"
        );
        assert_eq!(parsed["container"]["network"].as_str().unwrap(), "none");
    }

    #[tokio::test]
    async fn upsert_errors_on_non_table_container() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("bad.toml");

        // Write config where container is a string, not a table
        tokio::fs::write(&path, "container = \"not-a-table\"\n")
            .await
            .unwrap();

        let result = upsert_container_toml_key(&path, "network", "bridge".into()).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not a table"),
            "expected 'not a table' in error, got: {}",
            err
        );
    }

    #[test]
    fn is_default_network_with_defaults() {
        let args = test_run_args();
        let config = Config::default();
        assert!(is_default_network(&args, &config));
    }

    #[test]
    fn is_default_network_with_cli_network() {
        let mut args = test_run_args();
        args.network = Some("host".to_string());
        let config = Config::default();
        assert!(!is_default_network(&args, &config));
    }

    #[test]
    fn is_default_network_with_cli_preset() {
        let mut args = test_run_args();
        args.network_preset = Some("dev".to_string());
        let config = Config::default();
        assert!(!is_default_network(&args, &config));
    }

    #[test]
    fn is_default_network_with_cli_allow() {
        let mut args = test_run_args();
        args.network_allow = vec!["github.com:443".to_string()];
        let config = Config::default();
        assert!(!is_default_network(&args, &config));
    }

    #[test]
    fn is_default_network_with_config_preset() {
        let args = test_run_args();
        let mut config = Config::default();
        config.container.network_preset = Some("dev".to_string());
        assert!(!is_default_network(&args, &config));
    }

    #[test]
    fn is_default_network_with_config_allow() {
        let args = test_run_args();
        let mut config = Config::default();
        config.container.network_allow = vec!["github.com:443".to_string()];
        assert!(!is_default_network(&args, &config));
    }
}
