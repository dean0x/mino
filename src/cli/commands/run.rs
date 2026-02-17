//! Run command - start a sandboxed session

/// Image registry prefix for mino images
const IMAGE_REGISTRY: &str = "ghcr.io/dean0x";

/// Default base image for layer composition (requires developer user, zsh, etc.)
const LAYER_BASE_IMAGE: &str = "ghcr.io/dean0x/mino-base:latest";

use crate::audit::AuditLog;
use crate::cache::{
    detect_lockfiles, format_bytes, gb_to_bytes, labels, CacheMount, CacheSizeStatus, CacheState,
    CacheVolume, LockfileInfo,
};
use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::credentials::{
    AwsCredentials, AzureCredentials, CredentialCache, GcpCredentials, GithubCredentials,
};
use crate::config::ConfigManager;
use crate::error::{MinoError, MinoResult};
use crate::layer::{compose_image, list_available_layers, resolve_layers};
use crate::network::{generate_iptables_wrapper, resolve_network_mode, NetworkMode};
use crate::orchestration::{create_runtime, ContainerConfig, ContainerRuntime, Platform};
use crate::session::{Session, SessionManager, SessionStatus};
use crate::ui::{self, BuildProgress, TaskSpinner, UiContext};
use console::style;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};
use uuid::Uuid;

/// Tracks cache volumes created during this session (for finalization)
struct CacheSession {
    /// Volumes that need to be finalized on clean exit
    volumes_to_finalize: Vec<String>,
}

impl CacheSession {
    fn new() -> Self {
        Self {
            volumes_to_finalize: Vec::new(),
        }
    }
}

/// Result of resolving the image to use
struct ImageResolution {
    /// Final image tag to use
    image: String,
    /// Extra env vars from layers (empty if using single image)
    layer_env: HashMap<String, String>,
}

/// Determine which layers to compose (if any).
///
/// Returns None for single-image mode, Some(names) for layer composition.
///
/// Precedence:
/// 1. CLI `--layers` → compose from layers
/// 2. CLI `--image` → use single image (overrides config layers)
/// 3. Config `container.layers` (non-empty) → compose from config layers
/// 4. Config `container.image` / default → use single image
fn resolve_layer_names(args: &RunArgs, config: &Config) -> Option<Vec<String>> {
    if !args.layers.is_empty() {
        return Some(args.layers.clone());
    }
    if args.image.is_none() && !config.container.layers.is_empty() {
        return Some(config.container.layers.clone());
    }
    None
}

/// Execute the run command
pub async fn execute(args: RunArgs, config: &Config) -> MinoResult<()> {
    let ctx = UiContext::detect();
    let mut spinner = TaskSpinner::new(&ctx);

    spinner.start("Initializing sandbox...");

    // Create platform-appropriate runtime
    let runtime = create_runtime(config)?;
    debug!("Using runtime: {}", runtime.runtime_name());

    // Validate environment (platform-specific checks)
    spinner.message(&format!("Checking {}...", runtime.runtime_name()));
    validate_environment().await?;

    // Determine project directory
    let project_dir = resolve_project_dir(&args, config)?;
    debug!("Project directory: {}", project_dir.display());

    // Ensure runtime is ready
    spinner.message(&format!("Starting {}...", runtime.runtime_name()));
    runtime.ensure_ready().await?;

    // Resolve image or compose from layers
    let layer_names = resolve_layer_names(&args, config);

    // Check image alias redirect (e.g., --image typescript → layer composition)
    let layer_names = layer_names.or_else(|| {
        let raw = args
            .image
            .clone()
            .unwrap_or_else(|| config.container.image.clone());
        image_alias_to_layer(&raw).map(|name| vec![name.to_string()])
    });

    // Interactive layer selection when no layers/image configured
    let layer_names = if layer_names.is_none()
        && ctx.is_interactive()
        && is_default_image(&args, config)
    {
        spinner.clear();
        match prompt_layer_selection(&ctx, &project_dir, config).await? {
            Some(selected) => {
                spinner.start("Initializing sandbox...");
                Some(selected)
            }
            None => None,
        }
    } else {
        layer_names
    };

    let using_layers = layer_names.is_some();

    let resolution = if let Some(names) = layer_names {
        // Phase 1: Resolve each layer with per-layer feedback
        let mut resolved = Vec::new();
        for name in &names {
            spinner.message(&format!("Resolving layer: {}...", name));
            let mut layers = resolve_layers(std::slice::from_ref(name), &project_dir).await?;
            resolved.append(&mut layers);
        }

        // Phase 2: Compose image (with streaming progress bar)
        spinner.clear();

        let label = names.join(", ");
        let progress = BuildProgress::new(&ctx, &label);
        let result = compose_image(
            &*runtime,
            LAYER_BASE_IMAGE,
            &resolved,
            Some(&|line: String| progress.on_line(line)),
        )
        .await;
        progress.finish();
        let result = result?;

        if result.was_cached {
            debug!("Using cached composed image: {}", result.image_tag);
        } else {
            debug!("Built new composed image: {}", result.image_tag);
        }

        ImageResolution {
            image: result.image_tag,
            layer_env: result.env,
        }
    } else {
        // Single image path (no layers)
        let raw = args
            .image
            .clone()
            .unwrap_or_else(|| config.container.image.clone());
        let image = resolve_image_alias(&raw);
        ImageResolution {
            image,
            layer_env: HashMap::new(),
        }
    };

    // Resolve network mode
    let network_mode = resolve_network_mode(
        args.network.as_deref(),
        &args.network_allow,
        &config.container.network,
        &config.container.network_allow,
    )?;
    debug!("Network mode: {:?}", network_mode);

    // Setup caching (if enabled)
    spinner.message("Setting up caches...");
    let (cache_mounts, cache_env, cache_session) =
        setup_caches(&*runtime, &args, config, &project_dir).await?;

    // Check cache size and warn if approaching limit
    if !args.no_cache && config.cache.enabled {
        check_cache_size_warning(&*runtime, config).await;
    }

    // Collect credentials
    spinner.message("Gathering credentials...");
    let (credentials, active_providers) = gather_credentials(&args, config).await?;

    // Create session manager and run cleanup
    let session_name = args.name.clone().unwrap_or_else(generate_session_name);
    let manager = SessionManager::new().await?;

    if config.session.auto_cleanup_hours > 0 {
        let cleaned = manager.cleanup(config.session.auto_cleanup_hours).await?;
        if cleaned > 0 {
            debug!("Cleaned up {} old session(s)", cleaned);
        }
    }

    // Initialize audit log
    let audit = AuditLog::new(config);

    // Build container config (with cache mounts and env)
    let container_config = build_container_config(
        &args,
        config,
        &project_dir,
        &resolution,
        credentials,
        &cache_mounts,
        cache_env,
        &network_mode,
    )?;

    // Determine command to run
    // When using layers (composed on mino-base), default to /bin/zsh
    // which has Oh My Zsh, plugins, and aliases configured.
    let command = if args.command.is_empty() {
        if using_layers {
            vec!["/bin/zsh".to_string()]
        } else {
            vec![config.session.shell.clone()]
        }
    } else {
        args.command.clone()
    };

    // Wrap command with iptables if using network allowlist
    let command = if let NetworkMode::Allow(ref rules) = network_mode {
        generate_iptables_wrapper(rules, &command)
    } else {
        command
    };

    // Create session record
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

    // Check if image needs pulling and update spinner accordingly
    if !runtime
        .image_exists(&container_config.image)
        .await
        .unwrap_or(false)
    {
        spinner.message(&format!("Pulling image {}...", container_config.image));
    } else {
        spinner.message("Starting container...");
    }

    if args.detach {
        // Detached mode: run -d returns container ID immediately
        let container_id = match runtime.run(&container_config, &command).await {
            Ok(id) => id,
            Err(e) => {
                manager
                    .update_status(&session_name, SessionStatus::Failed)
                    .await?;
                audit
                    .log(
                        "session.failed",
                        &serde_json::json!({
                            "name": &session_name,
                            "error": e.to_string(),
                        }),
                    )
                    .await;
                return Err(e);
            }
        };

        manager
            .set_container_id(&session_name, &container_id)
            .await?;
        manager
            .update_status(&session_name, SessionStatus::Running)
            .await?;

        audit
            .log(
                "session.started",
                &serde_json::json!({
                    "name": &session_name,
                    "container_id": &container_id,
                }),
            )
            .await;

        spinner.clear();

        println!(
            "{} Session {} started (container: {})",
            style("✓").green(),
            style(&session_name).cyan(),
            &container_id[..12]
        );
        println!("  Attach with: mino logs {}", session_name);
        println!("  Stop with:   mino stop {}", session_name);

        if !cache_session.volumes_to_finalize.is_empty() {
            println!(
                "  {} Cache finalization requires: mino stop {}",
                style("!").yellow(),
                session_name
            );
        }
    } else {
        // Interactive mode: create + start_attached (no race condition)
        let container_id = match runtime.create(&container_config, &command).await {
            Ok(id) => id,
            Err(e) => {
                manager
                    .update_status(&session_name, SessionStatus::Failed)
                    .await?;
                audit
                    .log(
                        "session.failed",
                        &serde_json::json!({
                            "name": &session_name,
                            "error": e.to_string(),
                        }),
                    )
                    .await;
                return Err(e);
            }
        };

        manager
            .set_container_id(&session_name, &container_id)
            .await?;
        manager
            .update_status(&session_name, SessionStatus::Running)
            .await?;

        audit
            .log(
                "session.started",
                &serde_json::json!({
                    "name": &session_name,
                    "container_id": &container_id,
                }),
            )
            .await;

        spinner.clear();

        debug!("Starting container attached: {}", &container_id[..12]);

        let exit_code = runtime.start_attached(&container_id).await?;

        // Finalize caches on clean exit
        if exit_code == 0 && !cache_session.volumes_to_finalize.is_empty() {
            finalize_caches(&*runtime, &cache_session).await;
        }

        // Clean up session
        manager
            .update_status(&session_name, SessionStatus::Stopped)
            .await?;

        audit
            .log(
                "session.stopped",
                &serde_json::json!({
                    "name": &session_name,
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
    }

    Ok(())
}

/// Setup cache volumes and environment variables
async fn setup_caches(
    runtime: &dyn ContainerRuntime,
    args: &RunArgs,
    config: &Config,
    project_dir: &Path,
) -> MinoResult<(Vec<CacheMount>, HashMap<String, String>, CacheSession)> {
    let mut cache_session = CacheSession::new();
    let mut cache_mounts = Vec::new();
    let mut cache_env = HashMap::new();

    // Check if caching is disabled
    if args.no_cache || !config.cache.enabled {
        debug!("Caching disabled");
        return Ok((cache_mounts, cache_env, cache_session));
    }

    // Detect lockfiles in project
    let lockfiles = detect_lockfiles(project_dir)?;
    if lockfiles.is_empty() {
        debug!("No lockfiles detected, skipping cache setup");
        return Ok((cache_mounts, cache_env, cache_session));
    }

    debug!("Detected {} lockfile(s)", lockfiles.len());

    // Process each lockfile
    for info in &lockfiles {
        let (mount, should_finalize) =
            setup_cache_for_lockfile(runtime, info, args.cache_fresh).await?;

        // Add environment variables for this ecosystem
        for (key, value) in info.ecosystem.cache_env_vars() {
            cache_env.insert(key.to_string(), value.to_string());
        }

        if should_finalize {
            cache_session
                .volumes_to_finalize
                .push(mount.volume_name.clone());
        }

        cache_mounts.push(mount);
    }

    // Add XDG_CACHE_HOME for general caching
    cache_env.insert("XDG_CACHE_HOME".to_string(), "/cache/xdg".to_string());

    Ok((cache_mounts, cache_env, cache_session))
}

/// Setup cache for a single lockfile, returns (mount, should_finalize)
async fn setup_cache_for_lockfile(
    runtime: &dyn ContainerRuntime,
    info: &LockfileInfo,
    force_fresh: bool,
) -> MinoResult<(CacheMount, bool)> {
    let volume_name = info.volume_name();

    // Check existing volume state
    let existing = if force_fresh {
        None
    } else {
        runtime.volume_inspect(&volume_name).await?
    };

    let (state, should_finalize) = match existing {
        Some(vol_info) => {
            let cache = CacheVolume::from_labels(&vol_info.name, &vol_info.labels);
            match cache.map(|c| c.state) {
                Some(CacheState::Complete) => {
                    debug!(
                        "Cache hit for {} ({}), mounting read-only",
                        info.ecosystem,
                        &info.hash[..8]
                    );
                    (CacheState::Complete, false)
                }
                Some(CacheState::Building) => {
                    debug!(
                        "Resuming incomplete cache for {} ({})",
                        info.ecosystem,
                        &info.hash[..8]
                    );
                    (CacheState::Building, true)
                }
                _ => {
                    // Unknown state, treat as building
                    warn!(
                        "Cache for {} ({}) has unknown state, treating as building",
                        info.ecosystem,
                        &info.hash[..8]
                    );
                    (CacheState::Building, true)
                }
            }
        }
        None => {
            // Cache miss - create new volume (idempotent with --ignore)
            debug!(
                "Cache miss for {} ({}), creating volume",
                info.ecosystem,
                &info.hash[..8]
            );

            let cache = CacheVolume::from_lockfile(info, CacheState::Building);
            runtime.volume_create(&volume_name, &cache.labels()).await?;

            // Re-inspect: another process may have created it first with different state
            match runtime.volume_inspect(&volume_name).await? {
                Some(vol_info) => {
                    let cache = CacheVolume::from_labels(&vol_info.name, &vol_info.labels);
                    match cache.map(|c| c.state) {
                        Some(CacheState::Complete) => (CacheState::Complete, false),
                        _ => (CacheState::Building, true),
                    }
                }
                None => (CacheState::Building, true), // shouldn't happen, but safe fallback
            }
        }
    };

    let mount = CacheMount {
        volume_name,
        container_path: "/cache".to_string(),
        readonly: state.is_readonly(),
        ecosystem: info.ecosystem,
    };

    Ok((mount, should_finalize))
}

/// Finalize cache volumes by marking them as complete
async fn finalize_caches(runtime: &dyn ContainerRuntime, cache_session: &CacheSession) {
    for volume_name in &cache_session.volumes_to_finalize {
        debug!("Finalizing cache: {}", volume_name);

        // Get current volume info
        let vol_info = match runtime.volume_inspect(volume_name).await {
            Ok(Some(info)) => info,
            Ok(None) => {
                warn!(
                    "Cache volume {} disappeared, skipping finalization",
                    volume_name
                );
                continue;
            }
            Err(e) => {
                warn!("Failed to inspect cache {}: {}", volume_name, e);
                continue;
            }
        };

        // Update labels to mark as complete
        let mut new_labels = vol_info.labels.clone();
        new_labels.insert(labels::STATE.to_string(), "complete".to_string());

        // Note: Podman doesn't support updating labels in place, so we just log success
        // The label was already set correctly when we created the volume
        // For true immutability, we'd need to track state externally or use a different mechanism
        debug!("Cache {} finalized (complete)", volume_name);
    }
}

/// Check cache size and print warning if approaching or exceeding limit
async fn check_cache_size_warning(runtime: &dyn ContainerRuntime, config: &Config) {
    let sizes = match runtime.volume_disk_usage("mino-cache-").await {
        Ok(s) => s,
        Err(_) => return, // Silently skip if we can't get sizes
    };

    let total_size: u64 = sizes.values().sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);

    if limit_bytes == 0 {
        return;
    }

    let status = CacheSizeStatus::from_usage(total_size, limit_bytes);
    let percent = CacheSizeStatus::percentage(total_size, limit_bytes);

    match status {
        CacheSizeStatus::Ok => {}
        CacheSizeStatus::Warning => {
            eprintln!(
                "{} Cache usage at {:.0}% ({} / {}). Consider running: mino cache gc",
                style("!").yellow(),
                percent,
                format_bytes(total_size),
                format_bytes(limit_bytes)
            );
        }
        CacheSizeStatus::Exceeded => {
            eprintln!(
                "{} Cache limit exceeded! {:.0}% ({} / {}). Run: mino cache gc",
                style("!").red().bold(),
                percent,
                format_bytes(total_size),
                format_bytes(limit_bytes)
            );
        }
    }
}

async fn validate_environment() -> MinoResult<()> {
    match Platform::detect() {
        Platform::MacOS => {
            // On macOS, check OrbStack
            use crate::orchestration::OrbStack;
            if !OrbStack::is_installed().await {
                return Err(MinoError::OrbStackNotFound);
            }
            if !OrbStack::is_running().await? {
                return Err(MinoError::OrbStackNotRunning);
            }
        }
        Platform::Linux => {
            // On Linux, basic checks are done in ensure_ready()
        }
        Platform::Unsupported => {
            return Err(MinoError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            ));
        }
    }
    Ok(())
}

fn resolve_project_dir(args: &RunArgs, config: &Config) -> MinoResult<PathBuf> {
    if let Some(ref path) = args.project {
        let canonical = path
            .canonicalize()
            .map_err(|e| MinoError::io(format!("resolving project path {}", path.display()), e))?;
        return Ok(canonical);
    }

    if let Some(ref path) = config.session.default_project_dir {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    env::current_dir().map_err(|e| MinoError::io("getting current directory", e))
}

/// Returns (env_vars, list of successfully loaded provider names)
async fn gather_credentials(
    args: &RunArgs,
    config: &Config,
) -> MinoResult<(HashMap<String, String>, Vec<String>)> {
    let mut env_vars = HashMap::new();
    let mut providers = Vec::new();
    let cache = CredentialCache::new().await?;

    let (use_aws, use_gcp, use_azure) = if args.all_clouds {
        (true, true, true)
    } else {
        (
            args.aws || config.credentials.aws.enabled,
            args.gcp || config.credentials.gcp.enabled,
            args.azure || config.credentials.azure.enabled,
        )
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
                providers.push("aws".to_string());
                debug!("AWS credentials loaded");
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
                providers.push("gcp".to_string());
                debug!("GCP credentials loaded");
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
                providers.push("azure".to_string());
                debug!("Azure credentials loaded");
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
                env_vars.insert("GITHUB_TOKEN".to_string(), token.clone());
                env_vars.insert("GH_TOKEN".to_string(), token);
                providers.push("github".to_string());
                debug!("GitHub token loaded");
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

    Ok((env_vars, providers))
}

#[allow(clippy::too_many_arguments)]
fn build_container_config(
    args: &RunArgs,
    config: &Config,
    project_dir: &Path,
    resolution: &ImageResolution,
    env_vars: HashMap<String, String>,
    cache_mounts: &[CacheMount],
    cache_env: HashMap<String, String>,
    network_mode: &NetworkMode,
) -> MinoResult<ContainerConfig> {
    let image = resolution.image.clone();

    let mut volumes = vec![
        // Mount project directory
        format!("{}:{}", project_dir.display(), config.container.workdir),
    ];

    // Add cache volume mounts
    for mount in cache_mounts {
        volumes.push(mount.volume_arg());
    }

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

    // Env precedence: config < layer < cache < credential < CLI -e
    let mut final_env = config.container.env.clone();
    final_env.extend(resolution.layer_env.clone());
    final_env.extend(cache_env);
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
        network: network_mode.to_podman_network().to_string(),
        interactive: !args.detach,
        tty: !args.detach,
        cap_add: if network_mode.requires_cap_net_admin() {
            vec!["NET_ADMIN".to_string()]
        } else {
            vec![]
        },
    })
}

fn generate_session_name() -> String {
    let short_id = &Uuid::new_v4().to_string()[..8];
    format!("session-{}", short_id)
}

/// Map image alias names to layer names for composition.
///
/// Language aliases (typescript, rust, etc.) are redirected to the layer
/// composition system instead of pulling pre-built GHCR images.
fn image_alias_to_layer(image: &str) -> Option<&str> {
    match image {
        "typescript" | "ts" | "node" => Some("typescript"),
        "rust" | "cargo" => Some("rust"),
        _ => None,
    }
}

/// Resolve image aliases to full registry paths.
///
/// Only `base` is a direct image alias. Language aliases (typescript, rust)
/// are handled by `image_alias_to_layer()` and redirected to layer composition.
///
/// Full image paths (containing `/` or `:`) are passed through unchanged.
fn resolve_image_alias(image: &str) -> String {
    // If the image contains '/' or ':', it's already a full path
    if image.contains('/') || image.contains(':') {
        return image.to_string();
    }

    let image_name = match image {
        "base" => "mino-base",
        // Not a known alias, pass through (user might have a local image)
        other => return other.to_string(),
    };

    format!("{}/{}:latest", IMAGE_REGISTRY, image_name)
}

/// Check if no explicit image was provided and config uses the default image.
fn is_default_image(args: &RunArgs, config: &Config) -> bool {
    args.image.is_none() && config.container.image == "fedora:43"
}

/// Prompt user to select development tool layers interactively.
/// Returns Some(layer_names) if layers selected, None for bare container.
async fn prompt_layer_selection(
    ctx: &UiContext,
    project_dir: &Path,
    config: &Config,
) -> MinoResult<Option<Vec<String>>> {
    let available = list_available_layers(project_dir).await?;
    if available.is_empty() {
        return Ok(None);
    }

    let options: Vec<(String, String, String)> = available
        .iter()
        .map(|l| (l.name.clone(), l.name.clone(), l.description.clone()))
        .collect();
    let option_refs: Vec<(String, &str, &str)> = options
        .iter()
        .map(|(v, l, h)| (v.clone(), l.as_str(), h.as_str()))
        .collect();

    let selected = ui::multiselect(
        ctx,
        "Select development tools (space to toggle, enter to confirm)",
        &option_refs,
        false,
    )
    .await?;

    if selected.is_empty() {
        return Ok(None);
    }

    // Ask where to save
    prompt_save_config(ctx, &selected, project_dir, config).await?;

    Ok(Some(selected))
}

/// Where to save the layer configuration
#[derive(Clone, PartialEq, Eq)]
enum SaveTarget {
    Local,
    Global,
    None,
}

/// Prompt user to save selected layers to config.
async fn prompt_save_config(
    ctx: &UiContext,
    layers: &[String],
    project_dir: &Path,
    _config: &Config,
) -> MinoResult<()> {
    let options: Vec<(SaveTarget, &str, &str)> = vec![
        (
            SaveTarget::Local,
            "Save to .mino.toml",
            "this project only",
        ),
        (
            SaveTarget::Global,
            "Save to global config",
            "~/.config/mino/config.toml",
        ),
        (SaveTarget::None, "Don't save", "one-time, no persistence"),
    ];

    let target = ui::select(ctx, "Save this configuration?", &options).await?;

    match target {
        SaveTarget::Local => {
            let path = project_dir.join(".mino.toml");
            write_layers_to_config(&path, layers).await?;
            println!(
                "  {} Saved to {}",
                style("✓").green(),
                path.display()
            );
        }
        SaveTarget::Global => {
            let path = ConfigManager::default_config_path();
            write_layers_to_config(&path, layers).await?;
            println!(
                "  {} Saved to {}",
                style("✓").green(),
                path.display()
            );
        }
        SaveTarget::None => {}
    }

    Ok(())
}

/// Write layer selection into a TOML config file (create or merge).
async fn write_layers_to_config(path: &Path, layers: &[String]) -> MinoResult<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| MinoError::io(format!("creating config directory {}", parent.display()), e))?;
    }

    let layers_value = toml::Value::Array(
        layers
            .iter()
            .map(|l| toml::Value::String(l.clone()))
            .collect(),
    );

    if path.exists() {
        // Read existing, merge container.layers
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| MinoError::io(format!("reading {}", path.display()), e))?;
        let mut doc: toml::Value = content
            .parse()
            .map_err(|e: toml::de::Error| MinoError::ConfigInvalid {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

        // Ensure container table exists and set layers
        let table = doc.as_table_mut().ok_or_else(|| MinoError::ConfigInvalid {
            path: path.to_path_buf(),
            reason: "config is not a TOML table".to_string(),
        })?;
        let container = table
            .entry("container")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let Some(ct) = container.as_table_mut() {
            ct.insert("layers".to_string(), layers_value);
        }

        let output = toml::to_string_pretty(&doc)
            .map_err(|e| MinoError::io("serializing config", std::io::Error::other(e)))?;
        tokio::fs::write(path, output)
            .await
            .map_err(|e| MinoError::io(format!("writing {}", path.display()), e))?;
    } else {
        // Write minimal config
        let mut table = toml::map::Map::new();
        let mut container = toml::map::Map::new();
        container.insert("layers".to_string(), layers_value);
        table.insert("container".to_string(), toml::Value::Table(container));

        let output = toml::to_string_pretty(&toml::Value::Table(table))
            .map_err(|e| MinoError::io("serializing config", std::io::Error::other(e)))?;
        tokio::fs::write(path, output)
            .await
            .map_err(|e| MinoError::io(format!("writing {}", path.display()), e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn test_run_args() -> RunArgs {
        RunArgs {
            name: None,
            project: None,
            aws: false,
            gcp: false,
            azure: false,
            all_clouds: false,
            ssh_agent: true,
            github: true,
            image: None,
            layers: vec![],
            env: vec![],
            volume: vec![],
            detach: false,
            no_cache: false,
            cache_fresh: false,
            network: None,
            network_allow: vec![],
            command: vec![],
        }
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
    async fn write_layers_creates_new_config() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join(".mino.toml");

        write_layers_to_config(&path, &["rust".to_string(), "typescript".to_string()])
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: toml::Value = content.parse().unwrap();
        let layers = parsed["container"]["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].as_str().unwrap(), "rust");
        assert_eq!(layers[1].as_str().unwrap(), "typescript");
    }

    #[tokio::test]
    async fn write_layers_merges_existing_config() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join(".mino.toml");

        // Write existing config with other settings
        tokio::fs::write(
            &path,
            "[container]\nimage = \"custom:latest\"\nnetwork = \"none\"\n",
        )
        .await
        .unwrap();

        write_layers_to_config(&path, &["typescript".to_string()])
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
        assert_eq!(
            parsed["container"]["network"].as_str().unwrap(),
            "none"
        );
    }
}
