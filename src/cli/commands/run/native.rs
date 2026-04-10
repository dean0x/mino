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
use crate::sandbox::config::validate_sandbox_paths;
use crate::sandbox::dotfiles;
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
    let (project_dir, network_mode) =
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
        #[cfg(target_os = "macos")]
        {
            vec!["/bin/zsh".to_string()]
        }
        #[cfg(not(target_os = "macos"))]
        {
            vec!["/bin/bash".to_string()]
        }
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

    // Auto-mount common host directories (read-only) when present
    let mut sandbox_config = config.sandbox.clone();
    if let Some(ref host_home) = dirs::home_dir() {
        for dir_name in dotfiles::AUTO_PASSTHROUGH_DIRS {
            let dir = host_home.join(dir_name);
            if dir.is_dir() {
                sandbox_config
                    .passthrough_paths
                    .push(dir.to_string_lossy().to_string());
            }
        }
    }

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

/// Validate native sandbox prerequisites and resolve project dir + network mode.
async fn validate_and_resolve(
    args: &RunArgs,
    config: &Config,
    platform: &dyn SandboxPlatform,
    _spinner: &mut TaskSpinner,
) -> MinoResult<(PathBuf, NetworkMode)> {
    platform.validate_setup().await?;
    validate_native_flags(args)?;

    let project_dir = resolve_project_dir(args)?;
    debug!("Project directory: {}", project_dir.display());

    let (cfg_network, cfg_allow, cfg_preset) =
        crate::sandbox::config::resolve_sandbox_network(&config.sandbox, &config.container);
    let network_mode = resolve_network_mode(&NetworkResolutionInput {
        cli_network: args.network.as_deref(),
        cli_allow_rules: &args.network_allow,
        cli_preset: args.network_preset.as_deref(),
        config_network: cfg_network,
        config_network_allow: cfg_allow,
        config_preset: cfg_preset,
    })?;
    debug!("Network mode: {:?}", network_mode);

    let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    validate_sandbox_paths(&config.sandbox, &home_dir)?;

    Ok((project_dir, network_mode))
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

/// Build sandbox environment variables
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

    // Inherit locale, terminal, and ANTHROPIC_API_KEY from the host environment.
    // Other provider keys (OPENAI_API_KEY, etc.) are not automatically inherited;
    // add them explicitly via `sandbox.env` in config if needed.
    for key in &["LANG", "LC_ALL", "TZ", "TERM", "ANTHROPIC_API_KEY"] {
        if let Ok(val) = std::env::var(key) {
            env.insert(key.to_string(), val);
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
async fn wait_with_signal_forwarding(
    process: &mut crate::sandbox::process::SandboxProcess,
) -> MinoResult<i32> {
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
async fn wait_with_signal_forwarding(
    process: &mut crate::sandbox::process::SandboxProcess,
) -> MinoResult<i32> {
    process.wait().await
}

/// Copy essential files from `.claude` into the sandbox.
///
/// Uses an allowlist to copy only configuration files the agent needs
/// (CLAUDE.md, settings, agents, commands, skills, and the current project's
/// memory). This avoids copying multi-GB state directories (sessions, debug,
/// telemetry, file-history, etc.) that are not needed inside the sandbox.
async fn copy_claude_dir(src: &Path, dest: &Path, project_dir: &Path) -> MinoResult<()> {
    /// Top-level entries to copy from ~/.claude (allowlist)
    const ALLOW_ENTRIES: &[&str] = &[
        "CLAUDE.md",
        "settings.json",
        "agents",
        "commands",
        "skills",
    ];

    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| MinoError::io("creating .claude copy dir", e))?;

    // Copy allowlisted top-level entries
    for name in ALLOW_ENTRIES {
        let src_path = src.join(name);
        let dest_path = dest.join(name);
        let metadata = match tokio::fs::symlink_metadata(&src_path).await {
            Ok(m) => m,
            Err(_) => continue, // Entry doesn't exist, skip
        };
        if metadata.is_file() {
            tokio::fs::copy(&src_path, &dest_path)
                .await
                .map_err(|e| MinoError::io(format!("copying .claude/{}", name), e))?;
        } else if metadata.is_dir() {
            copy_dir_recursive(&src_path, &dest_path).await?;
        }
    }

    // Copy only the current project's memory directory from projects/
    // Project dirs are named by path with slashes replaced by dashes,
    // e.g. /Users/dean/Sandbox/minotaur -> -Users-dean-Sandbox-minotaur
    let project_key = project_dir
        .to_string_lossy()
        .replace('/', "-");
    let projects_src = src.join("projects").join(&project_key);
    if projects_src.is_dir() {
        let projects_dest = dest.join("projects").join(&project_key);
        copy_dir_recursive(&projects_src, &projects_dest).await?;
    }

    Ok(())
}

/// Recursively copy a directory (all files and subdirectories).
///
/// Symlinks within the source are skipped (only regular files and dirs are copied).
async fn copy_dir_recursive(src: &Path, dest: &Path) -> MinoResult<()> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| MinoError::io("creating copy dir", e))?;

    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| MinoError::io("reading dir", e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| MinoError::io("reading dir entry", e))?
    {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let src_path = entry.path();
        let dest_path = dest.join(&name);
        let metadata = tokio::fs::symlink_metadata(&src_path)
            .await
            .map_err(|e| MinoError::io("stat", e))?;

        if metadata.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dest_path)).await?;
        } else if metadata.is_file() {
            tokio::fs::copy(&src_path, &dest_path)
                .await
                .map_err(|e| MinoError::io(format!("copying {}", name_str), e))?;
        }
        // Skip symlinks — don't follow them into the copy
    }
    Ok(())
}

/// Collect and deduplicate dotfile names from defaults and user config.
fn collect_dotfile_names(config_dotfiles: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for name in crate::sandbox::dotfiles::DEFAULT_DOTFILES
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

/// Prepare dotfiles for the native sandbox by building a staging directory containing:
///
/// 1. **Sanitized dotfiles** — files listed in `config.sandbox.dotfiles` are read from the
///    host home directory, stripped of sensitive content (tokens, credentials) via
///    [`dotfiles::prepare_dotfile_content`], and written into a process-scoped temp dir.
///    The temp dir is created with mode `0o700` to prevent TOCTOU symlink attacks.
///
/// 2. **Passthrough symlinks** — for each directory in [`dotfiles::AUTO_PASSTHROUGH_DIRS`]
///    (e.g. `.oh-my-zsh`, `.nvm`) that exists on the host, a symlink pointing to the host
///    path is created inside the staging dir. The helper binary re-creates these links inside
///    the sandbox home so shell configs resolve correctly.
///
/// 3. **Copied directories** — directories in [`dotfiles::AUTO_COPY_DIRS`] (e.g. `.claude`)
///    are copied rather than symlinked because the agent needs a writable, sandbox-local
///    version. `.claude` uses an allowlist-based copy ([`copy_claude_dir`]) to avoid
///    pulling in large state directories such as `sessions/` or `file-history/`.
///
/// # Parameters
/// - `config`: full Mino configuration; `sandbox.dotfiles` determines which dotfiles are staged.
/// - `project_dir`: absolute path to the current project directory, used by [`copy_claude_dir`]
///   to select the matching per-project memory directory.
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

    for dotfile in collect_dotfile_names(&config.sandbox.dotfiles) {
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

        let dest = tmp_dir.join(&dotfile);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MinoError::io("creating dotfile subdir", e))?;
        }
        tokio::fs::write(&dest, cleaned)
            .await
            .map_err(|e| MinoError::io(format!("writing {}", dotfile), e))?;
    }

    // Create symlinks to host directories so shell configs that reference
    // ~/.oh-my-zsh, ~/.nvm etc. resolve correctly via the sandbox home.
    // These are recreated in the sandbox home by copy_dotfiles in the helper.
    for dir_name in dotfiles::AUTO_PASSTHROUGH_DIRS {
        let host_dir = home_dir.join(dir_name);
        if host_dir.is_dir() {
            let link_path = tmp_dir.join(dir_name);
            #[cfg(unix)]
            {
                if let Err(e) = tokio::fs::symlink(&host_dir, &link_path).await {
                    tracing::debug!("Failed to create symlink for {}: {}", dir_name, e);
                }
            }
        }
    }

    // Copy directories that need sandbox-local mutability (not symlinked to host).
    // For .claude, use allowlist-based copy to avoid multi-GB state directories.
    for dir_name in dotfiles::AUTO_COPY_DIRS {
        let host_dir = home_dir.join(dir_name);
        if host_dir.is_dir() {
            let dest_dir = tmp_dir.join(dir_name);
            if dir_name == &".claude" {
                copy_claude_dir(&host_dir, &dest_dir, project_dir).await?;
            } else {
                copy_dir_recursive(&host_dir, &dest_dir).await?;
            }
        }
    }

    Ok(Some(tmp_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::RunArgs;
    use serial_test::serial;

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

    // ---- copy_dir_recursive tests ----

    #[tokio::test]
    async fn copy_dir_recursive_copies_files_and_subdirs() {
        let src_guard = tempfile::tempdir().unwrap();
        let dest_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path();
        let dest = dest_guard.path().join("dest");

        tokio::fs::write(src.join("file.txt"), b"hello").await.unwrap();
        tokio::fs::create_dir_all(src.join("subdir")).await.unwrap();
        tokio::fs::write(src.join("subdir").join("nested.txt"), b"world")
            .await
            .unwrap();

        copy_dir_recursive(src, &dest).await.unwrap();

        assert!(dest.join("file.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(dest.join("file.txt")).await.unwrap(),
            "hello"
        );
        assert!(dest.join("subdir").join("nested.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(dest.join("subdir").join("nested.txt"))
                .await
                .unwrap(),
            "world"
        );
        // cleanup is automatic when guards drop
    }

    // ---- copy_claude_dir tests ----

    #[tokio::test]
    async fn copy_claude_dir_copies_allowlisted_entries() {
        let src_guard = tempfile::tempdir().unwrap();
        let dest_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path();
        let dest = dest_guard.path().join("dest");

        // Create allowlisted files and dirs
        tokio::fs::write(src.join("CLAUDE.md"), b"# Config").await.unwrap();
        tokio::fs::write(src.join("settings.json"), b"{}").await.unwrap();
        tokio::fs::create_dir_all(src.join("skills").join("review"))
            .await
            .unwrap();
        tokio::fs::write(src.join("skills").join("review").join("skill.md"), b"skill")
            .await
            .unwrap();

        // Create non-allowlisted dirs that should NOT be copied
        for skip_dir in &["debug", "sessions", "telemetry", "file-history", "downloads"] {
            tokio::fs::create_dir_all(src.join(skip_dir)).await.unwrap();
            tokio::fs::write(src.join(skip_dir).join("data.bin"), b"big data")
                .await
                .unwrap();
        }

        let project_dir = PathBuf::from("/Users/test/my-project");
        copy_claude_dir(src, &dest, &project_dir).await.unwrap();

        // Allowlisted entries should be copied
        assert!(dest.join("CLAUDE.md").exists());
        assert!(dest.join("settings.json").exists());
        assert!(dest.join("skills").join("review").join("skill.md").exists());

        // Non-allowlisted dirs should NOT be copied
        assert!(!dest.join("debug").exists());
        assert!(!dest.join("sessions").exists());
        assert!(!dest.join("telemetry").exists());
        assert!(!dest.join("file-history").exists());
        assert!(!dest.join("downloads").exists());
        // cleanup is automatic when guards drop
    }

    #[tokio::test]
    async fn copy_claude_dir_copies_current_project_memory() {
        let src_guard = tempfile::tempdir().unwrap();
        let dest_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path();
        let dest = dest_guard.path().join("dest");

        // Create project dirs: one matching, one not
        let matching = src.join("projects").join("-Users-test-my-project");
        let other = src.join("projects").join("-Users-test-other-project");
        tokio::fs::create_dir_all(&matching).await.unwrap();
        tokio::fs::create_dir_all(matching.join("memory")).await.unwrap();
        tokio::fs::write(matching.join("memory").join("MEMORY.md"), b"memory")
            .await
            .unwrap();
        tokio::fs::create_dir_all(&other).await.unwrap();
        tokio::fs::write(other.join("data.json"), b"other project")
            .await
            .unwrap();

        let project_dir = PathBuf::from("/Users/test/my-project");
        copy_claude_dir(src, &dest, &project_dir).await.unwrap();

        // Current project's memory should be copied
        assert!(dest
            .join("projects")
            .join("-Users-test-my-project")
            .join("memory")
            .join("MEMORY.md")
            .exists());

        // Other project should NOT be copied
        assert!(!dest
            .join("projects")
            .join("-Users-test-other-project")
            .exists());
        // cleanup is automatic when guards drop
    }
}
