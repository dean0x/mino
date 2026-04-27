//! Native sandbox run flow
//!
//! Parallel to the container run flow, but uses kernel-level process isolation
//! instead of Podman containers. Shares credential gathering and session
//! management with the container path.

use crate::audit::AuditLog;
use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::network::{resolve_network_mode, NetworkMode, NetworkResolutionInput};
use crate::sandbox::config::{
    resolve_sandbox_network, validate_sandbox_paths, DEFAULT_ENV_PASSTHROUGH,
};
use crate::sandbox::dotfiles;
use crate::sandbox::fs_copy;
use crate::sandbox::native::{create_sandbox_platform, SandboxPlatform, SandboxSpawnConfig};
use crate::sandbox::process::SandboxProcess;
use crate::session::{Session, SessionManager, SessionStatus};
use crate::ui::{self, TaskSpinner, UiContext};
use console::style;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Result of credential gathering, bundled for passing between phases.
struct CredentialResult {
    env: HashMap<String, String>,
    providers: Vec<String>,
}

/// Session context created during session setup.
struct SessionContext {
    session_name: String,
    manager: SessionManager,
    audit: AuditLog,
}

/// Execute a run command using native sandbox mode
pub async fn execute_native(args: RunArgs, config: &Config) -> MinoResult<()> {
    #[cfg(unix)]
    let _terminal_guard = crate::terminal::TerminalGuard::save();

    let ctx = UiContext::detect();
    let mut spinner = TaskSpinner::new(&ctx);
    spinner.start("Initializing native sandbox...");

    // Phase 1: Validate prerequisites and resolve configuration
    let platform = create_sandbox_platform()?;
    let (project_dir, network_mode, home_dir) =
        validate_and_resolve(&args, config, &*platform, &mut spinner).await?;

    // Phase 2: Gather credentials and build environment
    let cred_result =
        gather_credentials_and_env(&args, config, &ctx, &mut spinner, &project_dir).await?;

    // Phase 3: Start proxy (if needed), prepare dotfiles, create session
    let mut env = cred_result.env;
    let (_proxy_handle, _denial_task) =
        start_proxy_if_needed(&network_mode, &mut env, config, &mut spinner).await?;
    let dotfile_dir = prepare_dotfiles(config, &project_dir).await?;
    let command = if args.command.is_empty() {
        let shell = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        vec![shell.to_string()]
    } else {
        args.command.clone()
    };
    let session_ctx = create_session_and_audit(
        &args,
        config,
        &project_dir,
        &command,
        &cred_result.providers,
        &network_mode,
    )
    .await?;

    // Auto-mount user-configured host directories (read-only) when present.
    // This runs after validate_and_resolve so we re-validate with the augmented paths.
    let mut sandbox_config = config.sandbox.clone();
    for dir_name in &config.sandbox.auto_passthrough_dirs {
        let dir = home_dir.join(dir_name);
        if dir.is_dir() {
            tracing::info!(dir = %dir.display(), "auto-mounting passthrough directory");
            sandbox_config
                .passthrough_paths
                .push(dir.to_string_lossy().to_string());
        }
    }
    // Re-validate after augmenting passthrough_paths so auto-mounted dirs
    // are subject to the same sensitive-path checks as user-specified paths.
    validate_sandbox_paths(&sandbox_config, &home_dir)?;

    // Phase 4: Spawn sandbox and monitor
    let spawn_config = SandboxSpawnConfig {
        session_id: session_ctx.session_name.clone(),
        project_dir: project_dir.clone(),
        command,
        env,
        network_mode,
        sandbox_config,
        dotfile_dir: dotfile_dir.clone(),
        interactive: !args.detach,
    };

    spawn_and_monitor(
        SpawnMonitorCtx {
            platform: &*platform,
            config,
            ui_ctx: &ctx,
            spinner: &mut spinner,
        },
        spawn_config,
        session_ctx,
        dotfile_dir,
        args.detach,
    )
    .await
}

/// Validate native sandbox prerequisites and resolve project dir, network mode, and home dir.
async fn validate_and_resolve(
    args: &RunArgs,
    config: &Config,
    platform: &dyn SandboxPlatform,
    _spinner: &mut TaskSpinner,
) -> MinoResult<(PathBuf, NetworkMode, PathBuf)> {
    platform.validate_setup().await?;
    validate_native_flags(args)?;

    let project_dir = resolve_project_dir(args)?;
    debug!("Project directory: {}", project_dir.display());

    let (cfg_network, cfg_allow, cfg_preset) =
        resolve_sandbox_network(&config.sandbox, &config.container);
    let network_mode = resolve_network_mode(&NetworkResolutionInput {
        cli_network: args.network.as_deref(),
        cli_allow_rules: &args.network_allow,
        cli_preset: args.network_preset.as_deref(),
        config_network: cfg_network,
        config_network_allow: cfg_allow,
        config_preset: cfg_preset,
    })?;
    debug!("Network mode: {:?}", network_mode);

    let home_dir = dirs::home_dir()
        .ok_or_else(|| MinoError::User("Cannot determine home directory".to_string()))?;
    validate_sandbox_paths(&config.sandbox, &home_dir)?;

    Ok((project_dir, network_mode, home_dir))
}

/// Gather cloud credentials and build the sandbox environment variables.
async fn gather_credentials_and_env(
    args: &RunArgs,
    config: &Config,
    ctx: &UiContext,
    spinner: &mut TaskSpinner,
    _project_dir: &Path,
) -> MinoResult<CredentialResult> {
    spinner.message("Gathering credentials...");
    let (credentials, active_providers, cred_failures) =
        super::credentials::gather_credentials(args, config).await?;

    if !cred_failures.is_empty() {
        spinner.stop("Credentials");
        for (provider, error) in &cred_failures {
            ui::step_warn(ctx, &format!("{}: {}", provider, error));
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
        spinner.start("Initializing native sandbox...");
    }

    let env = build_sandbox_env(config, &credentials);

    Ok(CredentialResult {
        env,
        providers: active_providers,
    })
}

/// Start the filtering proxy if network mode is Allow.
///
/// Returns the proxy handle (must outlive the sandbox) and the denial log task.
async fn start_proxy_if_needed(
    network_mode: &NetworkMode,
    env: &mut HashMap<String, String>,
    config: &Config,
    spinner: &mut TaskSpinner,
) -> MinoResult<(
    Option<crate::sandbox::proxy::ProxyHandle>,
    Option<tokio::task::JoinHandle<()>>,
)> {
    if let NetworkMode::Allow(ref rules) = network_mode {
        spinner.message("Starting network proxy...");

        let (denial_tx, mut denial_rx) = tokio::sync::mpsc::unbounded_channel::<(String, u16)>();
        let handle = crate::sandbox::proxy::start_proxy(rules.clone(), Some(denial_tx)).await?;
        debug!("Network proxy started on {}", handle.addr);

        for (key, value) in handle.proxy_env_vars() {
            env.insert(key, value);
        }

        let denial_audit = AuditLog::new(config);
        let denial_task = tokio::spawn(async move {
            while let Some((host, port)) = denial_rx.recv().await {
                denial_audit
                    .log(
                        "sandbox.network_denied",
                        &serde_json::json!({ "host": host, "port": port }),
                    )
                    .await;
            }
        });

        Ok((Some(handle), Some(denial_task)))
    } else {
        Ok((None, None))
    }
}

/// Create the session, write audit logs, and return the session context.
async fn create_session_and_audit(
    args: &RunArgs,
    config: &Config,
    project_dir: &Path,
    command: &[String],
    active_providers: &[String],
    network_mode: &NetworkMode,
) -> MinoResult<SessionContext> {
    let session_name = args
        .name
        .clone()
        .unwrap_or_else(super::generate_session_name);
    let manager = SessionManager::new().await?;

    if config.session.auto_cleanup_hours > 0 {
        let cleaned = manager.cleanup(config.session.auto_cleanup_hours).await?;
        if cleaned > 0 {
            debug!("Cleaned up {} old session(s)", cleaned);
        }
    }

    tokio::spawn(async {
        match crate::cli::commands::status::cleanup_stale_native_sessions().await {
            Ok(n) if n > 0 => debug!("Cleaned up {} stale native session(s)", n),
            Err(e) => debug!("Stale session cleanup failed (non-fatal): {}", e),
            _ => {}
        }
    });

    let mut session = Session::new(
        session_name.clone(),
        project_dir.to_path_buf(),
        command.to_vec(),
        SessionStatus::Starting,
    );
    session.runtime_mode = Some(crate::sandbox::RuntimeMode::Native);
    session.sandbox_user = Some(config.sandbox.sandbox_user.clone());
    manager.create(&session).await?;

    let audit = AuditLog::new(config);
    audit
        .log(
            "sandbox.spawn",
            &serde_json::json!({
                "session_id": session_name,
                "runtime_mode": "native",
                "project_dir": project_dir.display().to_string(),
                "command": command,
                "network_mode": format!("{:?}", network_mode),
            }),
        )
        .await;

    if !active_providers.is_empty() {
        audit
            .log(
                "credentials.injected",
                &serde_json::json!({
                    "session_name": &session_name,
                    "providers": active_providers,
                }),
            )
            .await;
    }

    Ok(SessionContext {
        session_name,
        manager,
        audit,
    })
}

/// Context for spawn_and_monitor, bundling references that were previously
/// passed as individual arguments.
struct SpawnMonitorCtx<'a> {
    platform: &'a dyn SandboxPlatform,
    config: &'a Config,
    ui_ctx: &'a UiContext,
    spinner: &'a mut TaskSpinner,
}

/// Spawn the sandbox process and monitor it (blocking for foreground, background for detach).
async fn spawn_and_monitor(
    ctx: SpawnMonitorCtx<'_>,
    spawn_config: SandboxSpawnConfig,
    session_ctx: SessionContext,
    dotfile_dir: Option<PathBuf>,
    detach: bool,
) -> MinoResult<()> {
    let SpawnMonitorCtx {
        platform,
        config,
        ui_ctx,
        spinner,
    } = ctx;

    let SessionContext {
        session_name,
        manager,
        audit,
    } = session_ctx;

    spinner.message("Starting native sandbox (setting permissions)...");

    let mut process = match platform.spawn(spawn_config).await {
        Ok(p) => p,
        Err(e) => {
            cleanup_dotfile_dir(&dotfile_dir).await;
            manager
                .update_status(&session_name, SessionStatus::Failed)
                .await?;
            audit
                .log(
                    "session.failed",
                    &serde_json::json!({
                        "name": session_name,
                        "error": e.to_string(),
                    }),
                )
                .await;
            return Err(e);
        }
    };

    // Update session with PID
    if let Some(pid) = process.pid() {
        if let Some(mut s) = manager.get(&session_name).await? {
            s.process_id = Some(pid);
            s.status = SessionStatus::Running;
            s.save().await?;
        }
    }

    if detach {
        return handle_detach(process, &session_name, &manager, spinner, ui_ctx).await;
    }

    spinner.stop(&format!(
        "Session {} started (native sandbox)",
        style(&session_name).cyan()
    ));

    let exit_code = wait_with_signal_forwarding(&mut process).await?;
    cleanup_dotfile_dir(&dotfile_dir).await;

    let final_status = if exit_code == 0 {
        SessionStatus::Stopped
    } else {
        SessionStatus::Failed
    };
    manager.update_status(&session_name, final_status).await?;

    audit
        .log(
            "session.stopped",
            &serde_json::json!({
                "name": session_name,
                "exit_code": exit_code,
                "runtime_mode": "native",
            }),
        )
        .await;

    if let Some(update) = crate::version::load_cached_update(config).await {
        let method = crate::version::detect_install_method();
        let hint = crate::version::update_hint(&method);
        println!(
            "\n  {} Mino v{} available (current: v{}). {}",
            style("\u{2139}").cyan(),
            update.latest,
            update.current,
            hint
        );
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Handle detached mode: set up log file, spawn background monitor, return.
async fn handle_detach(
    mut process: SandboxProcess,
    session_name: &str,
    manager: &SessionManager,
    spinner: &mut TaskSpinner,
    _ui_ctx: &UiContext,
) -> MinoResult<()> {
    let log_dir = crate::config::ConfigManager::state_dir().join("logs");
    tokio::fs::create_dir_all(&log_dir)
        .await
        .map_err(|e| MinoError::io("creating log directory", e))?;
    let log_path = log_dir.join(format!("{}.log", session_name));

    if let Some(mut s) = manager.get(session_name).await? {
        s.log_file = Some(log_path.clone());
        s.save().await?;
    }

    spinner.stop(&format!(
        "Session {} started (native sandbox, detached)",
        style(session_name).cyan()
    ));
    println!("  View logs: mino logs {}", session_name);
    println!("  Stop with: mino stop {}", session_name);

    let bg_session_name = session_name.to_string();
    tokio::spawn(async move {
        let exit_code = process.wait().await.unwrap_or(1);
        let status = if exit_code == 0 {
            SessionStatus::Stopped
        } else {
            SessionStatus::Failed
        };
        if let Ok(manager) = SessionManager::new().await {
            let _ = manager.update_status(&bg_session_name, status).await;
        }
    });

    Ok(())
}

/// Validate that no container-only flags are set
fn validate_native_flags(args: &RunArgs) -> MinoResult<()> {
    if args.image.is_some() {
        return Err(MinoError::NativeUnsupported {
            feature: "custom images (--image)".to_string(),
        });
    }
    if args.read_only {
        return Err(MinoError::NativeUnsupported {
            feature: "read-only filesystem (--read-only)".to_string(),
        });
    }
    if args.cache_fresh {
        return Err(MinoError::NativeUnsupported {
            feature: "cache management (--cache-fresh)".to_string(),
        });
    }
    if !args.layers.is_empty() {
        tracing::warn!("--layers ignored in native mode (using host tools)");
    }
    Ok(())
}

/// Build sandbox environment variables.
///
/// Resolution order (last writer wins):
/// 1. Fixed sandbox identity vars (`HOME`, `USER`, `MINO_SANDBOX`, `PATH`)
/// 2. Host env passthrough: keys from `sandbox.env_passthrough`
///    (falls back to [`crate::sandbox::config::DEFAULT_ENV_PASSTHROUGH`] when unset)
/// 3. Credential env vars (from credential providers)
/// 4. Explicit env vars: `sandbox.env` if set, else `container.env`
fn build_sandbox_env(
    config: &Config,
    credentials: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    // HOME is set to /home/agent for both platforms. On Linux, this becomes
    // the real home inside the namespace. On macOS, the helper binary overrides
    // this to /tmp/mino-home-{session_id} in build_child_env().
    env.insert("HOME".to_string(), "/home/agent".to_string());
    env.insert("USER".to_string(), config.sandbox.sandbox_user.clone());
    env.insert("MINO_SANDBOX".to_string(), "native".to_string());

    // Inherit keys from the host environment. Uses the configured list when set,
    // or the default list (locale vars + ANTHROPIC_API_KEY).
    // Users can add other provider keys (OPENAI_API_KEY, GROQ_API_KEY, etc.) via
    // `sandbox.env_passthrough` in config without requiring a code change.
    let default_passthrough = DEFAULT_ENV_PASSTHROUGH;
    let passthrough_keys: Vec<&str> = config
        .sandbox
        .env_passthrough
        .as_deref()
        .map(|v| v.iter().map(|s| s.as_str()).collect())
        .unwrap_or_else(|| default_passthrough.to_vec());

    for key in &passthrough_keys {
        if let Ok(val) = std::env::var(key) {
            env.insert((*key).to_string(), val);
        }
    }

    // PATH: Homebrew + system paths (toolchain paths added later based on passthrough mounts)
    env.insert(
        "PATH".to_string(),
        crate::sandbox::helper::SANDBOX_PATH.to_string(),
    );

    // Credential env vars
    env.extend(credentials.clone());

    // User-specified env vars from config (sandbox-specific overrides container)
    let effective_env = config.sandbox.env.as_ref().unwrap_or(&config.container.env);
    env.extend(effective_env.clone());

    env
}

/// Resolve project directory from CLI args or current directory
fn resolve_project_dir(args: &RunArgs) -> MinoResult<PathBuf> {
    let dir = match &args.project {
        Some(p) => p.clone(),
        None => {
            std::env::current_dir().map_err(|e| MinoError::io("getting current directory", e))?
        }
    };
    if !dir.is_dir() {
        return Err(MinoError::PathNotFound(dir));
    }
    Ok(dir)
}

/// Clean up the dotfile temp directory after sandbox exit.
/// Best-effort: logs a warning on failure but does not propagate errors.
async fn cleanup_dotfile_dir(dir: &Option<PathBuf>) {
    if let Some(path) = dir {
        if path.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(path).await {
                tracing::warn!("Failed to clean up dotfile dir {}: {}", path.display(), e);
            }
        }
    }
}

/// Wait for the sandboxed process to exit, forwarding signals on Unix.
///
/// On Unix, SIGINT and SIGTERM are caught and forwarded to the sandboxed
/// process via `terminate()`. This ensures that Ctrl-C properly stops
/// the sandbox rather than killing only the mino wrapper.
///
/// Signal handling is integration-tested; the individual components
/// (`terminate`, `wait`) are unit-tested independently in the process module.
#[cfg(unix)]
async fn wait_with_signal_forwarding(process: &mut SandboxProcess) -> MinoResult<i32> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| MinoError::io("setting up SIGINT handler", e))?;
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| MinoError::io("setting up SIGTERM handler", e))?;

    tokio::select! {
        exit_code = process.wait() => {
            exit_code
        }
        _ = sigint.recv() => {
            debug!("Received SIGINT, forwarding to sandbox process");
            process.terminate().await.ok();
            // Wait for the process to exit after receiving the signal
            process.wait().await
        }
        _ = sigterm.recv() => {
            debug!("Received SIGTERM, forwarding to sandbox process");
            process.terminate().await.ok();
            process.wait().await
        }
    }
}

/// Non-Unix fallback: just wait for the process.
#[cfg(not(unix))]
async fn wait_with_signal_forwarding(process: &mut SandboxProcess) -> MinoResult<i32> {
    process.wait().await
}

/// Collect and deduplicate dotfile names from defaults and user config.
fn collect_dotfile_names(config_dotfiles: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for name in dotfiles::DEFAULT_DOTFILES
        .iter()
        .map(|s| s.to_string())
        .chain(config_dotfiles.iter().cloned())
    {
        if seen.insert(name.clone()) {
            result.push(name);
        }
    }
    result
}

/// Stage sanitized dotfiles from `home_dir` into `staging`.
///
/// Reads each dotfile, strips secrets via [`dotfiles::prepare_dotfile_content`],
/// and writes the cleaned content. Warns on known-risky files. Skips files that
/// do not exist on the host.
async fn write_sanitized_dotfiles(
    staging: PathBuf,
    home_dir: PathBuf,
    dotfile_names: Vec<String>,
) -> MinoResult<()> {
    for dotfile in dotfile_names {
        if dotfiles::is_risky_dotfile(&dotfile) {
            tracing::warn!(
                "{} may contain auth tokens. Secrets will be accessible to the agent.",
                dotfile
            );
        }
        let source = home_dir.join(&dotfile);
        if !source.exists() {
            continue;
        }

        let content = tokio::fs::read_to_string(&source)
            .await
            .map_err(|e| MinoError::io(format!("reading {}", dotfile), e))?;
        let cleaned = dotfiles::prepare_dotfile_content(&dotfile, &content);

        let dest = staging.join(&dotfile);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MinoError::io("creating dotfile subdir", e))?;
        }
        tokio::fs::write(&dest, cleaned)
            .await
            .map_err(|e| MinoError::io(format!("writing {}", dotfile), e))?;
    }
    Ok(())
}

/// Create passthrough symlinks in `staging` pointing to host directories.
///
/// For each directory in `passthrough_dirs` that exists on the host, a symlink
/// is created in the staging directory. The helper binary recreates these links
/// inside the sandbox home so shell configs (`.oh-my-zsh`, `.nvm`, etc.) resolve.
///
/// Multi-segment entries (e.g., `.config/gh`) are supported: the parent directory
/// is created in the staging area via `create_dir_all` before the symlink is placed.
#[cfg(unix)]
async fn create_passthrough_symlinks(
    staging: PathBuf,
    home_dir: PathBuf,
    passthrough_dirs: Vec<String>,
) -> MinoResult<()> {
    for dir_name in passthrough_dirs {
        let host_dir = home_dir.join(&dir_name);
        if host_dir.is_dir() {
            let link_path = staging.join(&dir_name);
            // Ensure parent directories exist in the staging tree for multi-segment
            // entries (e.g., staging/.config/ must exist before placing staging/.config/gh).
            if let Some(parent) = link_path.parent() {
                if parent != staging {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        MinoError::io("creating staging parent dir for passthrough", e)
                    })?;
                }
            }
            if let Err(e) = tokio::fs::symlink(&host_dir, &link_path).await {
                tracing::debug!("Failed to create symlink for {}: {}", dir_name, e);
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
async fn create_passthrough_symlinks(
    _staging: PathBuf,
    _home_dir: PathBuf,
    _passthrough_dirs: Vec<String>,
) -> MinoResult<()> {
    Ok(())
}

/// Copy user-configured directories into the sandbox staging area.
///
/// For `.claude`, uses the allowlist-based [`crate::sandbox::dotfiles::copy_claude_config_dir`]
/// to avoid multi-GB state directories. All other entries use generic
/// [`crate::sandbox::fs_copy::copy_dir_recursive`].
async fn copy_auto_dirs(
    staging: PathBuf,
    home_dir: PathBuf,
    copy_dirs: Vec<String>,
    project_dir: PathBuf,
) -> MinoResult<()> {
    for dir_name in copy_dirs {
        let host_dir = home_dir.join(&dir_name);
        if host_dir.is_dir() {
            let dest_dir = staging.join(&dir_name);
            tracing::info!(dir = %dir_name, "auto-copying directory into sandbox");
            if dir_name == ".claude" {
                dotfiles::copy_claude_config_dir(&host_dir, &dest_dir, &project_dir).await?;
            } else {
                fs_copy::copy_dir_recursive(host_dir, dest_dir).await?;
            }
        }
    }
    Ok(())
}

/// Prepare dotfiles for the native sandbox by building a staging directory containing:
///
/// 1. **Sanitized dotfiles** — files listed in `config.sandbox.dotfiles` are read from the
///    host home directory, stripped of sensitive content (tokens, credentials) via
///    [`dotfiles::prepare_dotfile_content`], and written into a process-scoped temp dir.
///    The temp dir is created with mode `0o700` to prevent TOCTOU symlink attacks.
///
/// 2. **Passthrough symlinks** — for each directory in `config.sandbox.auto_passthrough_dirs`
///    that exists on the host, a symlink pointing to the host path is created inside the
///    staging dir. The helper binary re-creates these links inside the sandbox home so shell
///    configs resolve correctly. Empty by default (opt-in via config).
///
/// 3. **Copied directories** — directories in `config.sandbox.auto_copy_dirs` are copied
///    rather than symlinked because the agent needs a writable, sandbox-local version.
///    `.claude` uses an allowlist-based copy to avoid pulling in large state directories
///    such as `sessions/` or `file-history/`. Empty by default (opt-in).
///
/// All three stages run concurrently via [`tokio::try_join!`]. The three helper functions
/// take ownership of their inputs (cloned from `config`) so there are no shared references.
/// The stages are safe to run in parallel because `config.sandbox.validate()` (called during
/// config load) ensures no overlap between the sets.
///
/// # Partial failure
/// If one stage fails after another has already written files, the staging directory will be
/// in a partial state. The caller is responsible for cleanup via [`cleanup_dotfile_dir`].
///
/// Returns `None` if the host home directory cannot be determined.
async fn prepare_dotfiles(config: &Config, project_dir: &Path) -> MinoResult<Option<PathBuf>> {
    let home_dir = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(None),
    };

    let tmp_dir = std::env::temp_dir().join(format!("mino-dotfiles-{}", std::process::id()));
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| MinoError::io("creating dotfile temp dir", e))?;

    // Restrict permissions so other users cannot replace files with symlinks
    // between creation and the helper binary's copy (TOCTOU hardening).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        tokio::fs::set_permissions(&tmp_dir, perms)
            .await
            .map_err(|e| MinoError::io("setting dotfile temp dir permissions", e))?;
    }

    // Clone inputs once so each stage owns its data (required by tokio::try_join!)
    let dotfile_names = collect_dotfile_names(&config.sandbox.dotfiles);
    let passthrough_dirs = config.sandbox.auto_passthrough_dirs.clone();
    let copy_dirs = config.sandbox.auto_copy_dirs.clone();
    let project_dir_owned = project_dir.to_path_buf();

    // Run the three independent stages concurrently.
    // Disjointness of staging entries is guaranteed by SandboxConfig::validate().
    tokio::try_join!(
        write_sanitized_dotfiles(tmp_dir.clone(), home_dir.clone(), dotfile_names),
        create_passthrough_symlinks(tmp_dir.clone(), home_dir.clone(), passthrough_dirs),
        copy_auto_dirs(tmp_dir.clone(), home_dir, copy_dirs, project_dir_owned),
    )?;

    Ok(Some(tmp_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::RunArgs;
    use serial_test::serial;

    /// Guard that restores environment variables on drop, even on panic.
    ///
    /// Used by `#[serial]` tests that modify process environment: the Drop impl
    /// ensures cleanup runs on any exit path so subsequent tests see a clean env.
    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        /// Capture the current values of the given keys, then set each to the provided value.
        ///
        /// The Drop impl restores every captured key to its original value (or removes it
        /// if it was absent), so cleanup runs even if a test assertion panics.
        fn set(pairs: &[(&str, &str)]) -> Self {
            let vars = pairs
                .iter()
                .map(|(k, v)| {
                    let prev = std::env::var(k).ok();
                    unsafe { std::env::set_var(k, v) };
                    (k.to_string(), prev)
                })
                .collect();
            Self { vars }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, prev) in &self.vars {
                match prev {
                    Some(v) => unsafe { std::env::set_var(k, v) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

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
            read_only: false,
            no_cache: false,
            no_home: false,
            cache_fresh: false,
            network: None,
            network_allow: vec![],
            network_preset: None,
            runtime: None,
            command: vec![],
        }
    }

    // ---- validate_native_flags tests ----

    #[test]
    fn validate_native_flags_image_returns_error() {
        let mut args = test_run_args();
        args.image = Some("custom:latest".to_string());
        let err = validate_native_flags(&args).unwrap_err();
        assert!(err.to_string().contains("custom images (--image)"));
        assert!(err.to_string().contains("not supported in native sandbox"));
    }

    #[test]
    fn validate_native_flags_read_only_returns_error() {
        let mut args = test_run_args();
        args.read_only = true;
        let err = validate_native_flags(&args).unwrap_err();
        assert!(err.to_string().contains("read-only filesystem"));
    }

    #[test]
    fn validate_native_flags_cache_fresh_returns_error() {
        let mut args = test_run_args();
        args.cache_fresh = true;
        let err = validate_native_flags(&args).unwrap_err();
        assert!(err.to_string().contains("cache management"));
    }

    #[test]
    fn validate_native_flags_no_flags_is_ok() {
        let args = test_run_args();
        assert!(validate_native_flags(&args).is_ok());
    }

    #[test]
    fn validate_native_flags_layers_is_ok() {
        // Layers are warned about but not rejected
        let mut args = test_run_args();
        args.layers = vec!["rust".to_string()];
        assert!(validate_native_flags(&args).is_ok());
    }

    // ---- build_sandbox_env tests ----

    #[test]
    fn build_sandbox_env_includes_basic_vars_and_path() {
        let env = build_sandbox_env(&Config::default(), &HashMap::new());

        assert_eq!(env.get("HOME").unwrap(), "/home/agent");
        assert_eq!(env.get("USER").unwrap(), "_mino_agent");
        assert_eq!(env.get("MINO_SANDBOX").unwrap(), "native");
        let path = env.get("PATH").unwrap();
        assert!(path.contains("/opt/homebrew/bin"));
        assert!(path.contains("/usr/bin"));
        assert!(path.contains("/bin"));
    }

    #[test]
    fn build_sandbox_env_includes_credentials() {
        let config = Config::default();
        let mut credentials = HashMap::new();
        credentials.insert("AWS_ACCESS_KEY_ID".to_string(), "AKIA123".to_string());
        credentials.insert("AWS_SECRET_ACCESS_KEY".to_string(), "secret123".to_string());

        let env = build_sandbox_env(&config, &credentials);
        assert_eq!(env.get("AWS_ACCESS_KEY_ID").unwrap(), "AKIA123");
        assert_eq!(env.get("AWS_SECRET_ACCESS_KEY").unwrap(), "secret123");
    }

    #[test]
    #[serial]
    fn build_sandbox_env_inherits_term_when_set() {
        unsafe { std::env::set_var("TERM", "xterm-256color") };
        let env = build_sandbox_env(&Config::default(), &HashMap::new());
        assert_eq!(env.get("TERM").unwrap(), "xterm-256color");
        unsafe { std::env::remove_var("TERM") };
    }

    #[test]
    #[serial]
    fn build_sandbox_env_omits_term_when_unset() {
        unsafe { std::env::remove_var("TERM") };
        let env = build_sandbox_env(&Config::default(), &HashMap::new());
        assert!(!env.contains_key("TERM"));
        // Restore a reasonable default
        unsafe { std::env::set_var("TERM", "xterm-256color") };
    }

    #[test]
    fn build_sandbox_env_includes_config_env() {
        let mut config = Config::default();
        config
            .container
            .env
            .insert("CUSTOM_VAR".to_string(), "custom_value".to_string());
        let credentials = HashMap::new();
        let env = build_sandbox_env(&config, &credentials);
        assert_eq!(env.get("CUSTOM_VAR").unwrap(), "custom_value");
    }

    // ---- resolve_project_dir tests ----

    #[test]
    fn resolve_project_dir_uses_cwd_when_none() {
        let args = test_run_args();
        let dir = resolve_project_dir(&args).unwrap();
        assert!(dir.is_dir());
    }

    #[test]
    fn resolve_project_dir_uses_explicit_path() {
        let mut args = test_run_args();
        args.project = Some(PathBuf::from("/tmp"));
        let dir = resolve_project_dir(&args).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn resolve_project_dir_rejects_nonexistent() {
        let mut args = test_run_args();
        args.project = Some(PathBuf::from("/nonexistent/path/abc123"));
        let err = resolve_project_dir(&args).unwrap_err();
        assert!(err.to_string().contains("Path not found"));
    }

    #[test]
    fn validate_native_flags_volume_is_ok() {
        // --volume is allowed in native mode (maps to passthrough/writable paths)
        let mut args = test_run_args();
        args.volume = vec!["/host/path:/container/path".to_string()];
        assert!(validate_native_flags(&args).is_ok());
    }

    #[test]
    fn validate_native_flags_detach_is_ok() {
        let mut args = test_run_args();
        args.detach = true;
        assert!(validate_native_flags(&args).is_ok());
    }

    // ---- collect_dotfile_names tests ----

    #[test]
    fn collect_dotfile_names_includes_defaults() {
        let names = collect_dotfile_names(&[]);
        assert!(names.contains(&".gitconfig".to_string()));
        assert!(names.contains(&".config/git/ignore".to_string()));
        assert!(names.contains(&".zshrc".to_string()));
        assert!(names.contains(&".zshenv".to_string()));
        assert!(names.contains(&".zprofile".to_string()));
        assert!(names.contains(&".tmux.conf".to_string()));
    }

    #[test]
    fn collect_dotfile_names_includes_user_dotfiles() {
        let names = collect_dotfile_names(&[".vimrc".to_string()]);
        assert!(names.contains(&".vimrc".to_string()));
    }

    #[test]
    fn collect_dotfile_names_deduplicates() {
        let names = collect_dotfile_names(&[".gitconfig".to_string()]);
        let gitconfig_count = names.iter().filter(|n| *n == ".gitconfig").count();
        assert_eq!(
            gitconfig_count, 1,
            "expected .gitconfig to appear only once"
        );
    }

    #[test]
    fn build_sandbox_env_prefers_sandbox_env() {
        let mut config = Config::default();
        // Set container env
        config
            .container
            .env
            .insert("SHARED".to_string(), "from_container".to_string());
        // Set sandbox-specific env that should take precedence
        let mut sandbox_env = HashMap::new();
        sandbox_env.insert("SHARED".to_string(), "from_sandbox".to_string());
        sandbox_env.insert("SANDBOX_ONLY".to_string(), "yes".to_string());
        config.sandbox.env = Some(sandbox_env);
        let env = build_sandbox_env(&config, &HashMap::new());
        assert_eq!(env.get("SHARED").unwrap(), "from_sandbox");
        assert_eq!(env.get("SANDBOX_ONLY").unwrap(), "yes");
    }

    #[test]
    fn build_sandbox_env_falls_back_to_container_env() {
        let mut config = Config::default();
        config
            .container
            .env
            .insert("FROM_CONTAINER".to_string(), "hello".to_string());
        // sandbox.env is None (default)
        let env = build_sandbox_env(&config, &HashMap::new());
        assert_eq!(env.get("FROM_CONTAINER").unwrap(), "hello");
    }

    #[test]
    #[serial]
    fn build_sandbox_env_default_passthrough_includes_anthropic_and_locale() {
        // Ensure the defaults are in place when env_passthrough is not configured.
        // EnvGuard restores the variable on drop, even if an assertion panics.
        let _guard = EnvGuard::set(&[("ANTHROPIC_API_KEY", "test-key-123")]);
        let config = Config::default();
        assert!(config.sandbox.env_passthrough.is_none());
        let env = build_sandbox_env(&config, &HashMap::new());
        assert_eq!(env.get("ANTHROPIC_API_KEY").unwrap(), "test-key-123");
    }

    #[test]
    #[serial]
    fn build_sandbox_env_custom_passthrough_replaces_defaults() {
        // Custom list should NOT inherit ANTHROPIC_API_KEY unless explicitly listed.
        // EnvGuard restores both variables on drop, even if an assertion panics.
        let _guard = EnvGuard::set(&[
            ("ANTHROPIC_API_KEY", "should-not-appear"),
            ("MY_CUSTOM_KEY", "custom-value"),
        ]);

        let mut config = Config::default();
        config.sandbox.env_passthrough = Some(vec!["MY_CUSTOM_KEY".to_string()]);

        let env = build_sandbox_env(&config, &HashMap::new());
        assert_eq!(env.get("MY_CUSTOM_KEY").unwrap(), "custom-value");
        assert!(
            !env.contains_key("ANTHROPIC_API_KEY"),
            "ANTHROPIC_API_KEY should not appear when not in custom passthrough list"
        );
    }

    #[test]
    #[serial]
    fn build_sandbox_env_empty_passthrough_disables_all_inheritance() {
        // EnvGuard restores both variables on drop, even if an assertion panics.
        let _guard = EnvGuard::set(&[("ANTHROPIC_API_KEY", "key"), ("LANG", "en_US.UTF-8")]);

        let mut config = Config::default();
        config.sandbox.env_passthrough = Some(vec![]);

        let env = build_sandbox_env(&config, &HashMap::new());
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert!(!env.contains_key("LANG"));
    }

    // ---- create_passthrough_symlinks multi-segment tests ----

    #[cfg(unix)]
    #[tokio::test]
    async fn create_passthrough_symlinks_handles_multi_segment_entry() {
        let home = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();

        // Create .config/gh on the host
        let config_gh = home.path().join(".config").join("gh");
        tokio::fs::create_dir_all(&config_gh).await.unwrap();

        create_passthrough_symlinks(
            staging.path().to_path_buf(),
            home.path().to_path_buf(),
            vec![".config/gh".to_string()],
        )
        .await
        .unwrap();

        // staging/.config/ must be a real directory
        let staging_config = staging.path().join(".config");
        assert!(staging_config.is_dir(), "staging/.config should be a dir");

        // staging/.config/gh must be a symlink
        let staging_gh = staging_config.join("gh");
        let meta = tokio::fs::symlink_metadata(&staging_gh).await.unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "staging/.config/gh should be a symlink"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn create_passthrough_symlinks_idempotent_parent_creation() {
        let home = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();

        // Create two dirs under .config on the host
        tokio::fs::create_dir_all(home.path().join(".config").join("gh"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(home.path().join(".config").join("other"))
            .await
            .unwrap();

        create_passthrough_symlinks(
            staging.path().to_path_buf(),
            home.path().to_path_buf(),
            vec![".config/gh".to_string(), ".config/other".to_string()],
        )
        .await
        .unwrap();

        let staging_config = staging.path().join(".config");
        assert!(staging_config.is_dir(), "staging/.config should be a dir");

        let meta_gh = tokio::fs::symlink_metadata(staging_config.join("gh"))
            .await
            .unwrap();
        assert!(meta_gh.file_type().is_symlink());

        let meta_other = tokio::fs::symlink_metadata(staging_config.join("other"))
            .await
            .unwrap();
        assert!(meta_other.file_type().is_symlink());
    }
}
