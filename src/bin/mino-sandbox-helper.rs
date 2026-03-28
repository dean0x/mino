//! Mino sandbox helper - macOS privileged helper binary
//!
//! Installed to /usr/local/bin/ during `mino setup --native`.
//! Called via sudoers.d: `<user> ALL=(root) NOPASSWD: /usr/local/bin/mino-sandbox-helper`
//!
//! Operations:
//! - spawn: Set ACLs, create pf sub-anchor, fork+setuid to sandbox user, exec command
//! - exec: Drop privileges to sandbox user and exec a command (no ACL setup, no fork)
//! - cleanup: Remove ACLs, remove pf sub-anchor
//! - health-check: Return version

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicI32, Ordering};

use mino::sandbox::helper;
use mino::sandbox::helper_protocol::*;
use mino::session::validate_session_name;

/// Parameters for the spawn operation, extracted from HelperRequest::Spawn
struct SpawnParams {
    session_id: String,
    project_dir: PathBuf,
    env: HashMap<String, String>,
    command: Vec<String>,
    resource_limits: ResourceLimitsDto,
    acl_paths: Vec<AclEntry>,
    dotfile_dir: Option<PathBuf>,
    home_dir: PathBuf,
    sandbox_user: String,
}

/// Pre-fork state: UID/GID resolved and validated, ready to fork.
struct SpawnReady {
    uid: u32,
    gid: u32,
}

/// Arguments for the child process after fork.
struct ChildArgs<'a> {
    uid: u32,
    gid: u32,
    resource_limits: &'a ResourceLimitsDto,
    env: &'a HashMap<String, String>,
    home_dir: &'a Path,
    project_dir: &'a Path,
    command: &'a [String],
    sandbox_user: &'a str,
}

/// Load a HelperRequest from the --request-file argument.
///
/// Reads and parses the JSON file, then deletes it immediately to minimize
/// the time credentials sit on disk.
fn load_request(args: &[String]) -> Result<HelperRequest, String> {
    let request_file = args
        .iter()
        .position(|a| a == "--request-file")
        .and_then(|i| args.get(i + 1))
        .ok_or_else(|| "Missing --request-file argument".to_string())?;
    let content = std::fs::read_to_string(request_file)
        .map_err(|e| format!("Failed to read request file '{}': {}", request_file, e))?;
    // Delete credential file immediately after reading — shortest time on disk
    std::fs::remove_file(request_file).ok();
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse request: {}", e))
}

/// Parsed arguments for the exec subcommand.
#[derive(Debug)]
struct ExecArgs<'a> {
    session_id: &'a str,
    sandbox_user: &'a str,
    command: &'a [String],
}

/// Parse exec subcommand arguments into an ExecArgs struct.
fn parse_exec_args(args: &[String]) -> Result<ExecArgs<'_>, String> {
    let mut session_id: Option<&str> = None;
    let mut sandbox_user: Option<&str> = None;
    let mut command_start: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session-id" => {
                session_id = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--sandbox-user" => {
                sandbox_user = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--pid" => {
                i += 2; // Accepted for compat, not used for exec
            }
            "--" => {
                command_start = Some(i + 1);
                break;
            }
            _ => {
                i += 1;
            }
        }
    }

    let session_id = match session_id {
        Some(id) if !id.is_empty() => id,
        _ => return Err("Missing --session-id argument".to_string()),
    };

    let sandbox_user = match sandbox_user {
        Some(u) if !u.is_empty() => u,
        _ => return Err("Missing --sandbox-user argument".to_string()),
    };

    let command: &[String] = match command_start {
        Some(idx) if idx < args.len() => &args[idx..],
        _ => return Err("Missing command after '--'".to_string()),
    };

    if command.is_empty() {
        return Err("Empty command".to_string());
    }

    Ok(ExecArgs {
        session_id,
        sandbox_user,
        command,
    })
}

/// Parsed arguments for the cleanup subcommand.
#[derive(Debug)]
struct CleanupArgs<'a> {
    session_id: &'a str,
    project_dir: PathBuf,
    sandbox_user: &'a str,
}

/// Parse cleanup subcommand arguments into a CleanupArgs struct.
fn parse_cleanup_args(args: &[String]) -> Result<CleanupArgs<'_>, String> {
    let mut session_id: Option<&str> = None;
    let mut project_dir: Option<&str> = None;
    let mut sandbox_user: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session-id" => {
                session_id = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--project-dir" => {
                project_dir = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--sandbox-user" => {
                sandbox_user = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    Ok(CleanupArgs {
        session_id: session_id.ok_or("Missing --session-id")?,
        project_dir: PathBuf::from(project_dir.ok_or("Missing --project-dir")?),
        sandbox_user: sandbox_user.ok_or("Missing --sandbox-user")?,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 2 && args[1] == "--version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if args.len() < 2 {
        eprintln!("Usage: mino-sandbox-helper <action> --request-file <path>");
        eprintln!("       mino-sandbox-helper --version");
        process::exit(1);
    }

    let action = &args[1];

    let result: Result<i32, String> = match action.as_str() {
        "spawn" => {
            let request = match load_request(&args) {
                Ok(r) => r,
                Err(msg) => {
                    print_error(&msg);
                    process::exit(1);
                }
            };

            match request {
                HelperRequest::Spawn {
                    session_id,
                    project_dir,
                    env,
                    command,
                    resource_limits,
                    acl_paths,
                    dotfile_dir,
                    home_dir,
                    sandbox_user,
                } => handle_spawn(SpawnParams {
                    session_id,
                    project_dir,
                    env,
                    command,
                    resource_limits,
                    acl_paths,
                    dotfile_dir,
                    home_dir,
                    sandbox_user,
                }),
                _ => Err("Expected Spawn request".into()),
            }
        }
        "cleanup" => {
            // Try CLI args first, fall back to request file for backward compat
            match parse_cleanup_args(&args[2..]) {
                Ok(parsed) => {
                    handle_cleanup(parsed.session_id, &parsed.project_dir, parsed.sandbox_user)
                        .map(|()| 0)
                }
                Err(_) => {
                    // Fallback: try loading from request file
                    let request = match load_request(&args) {
                        Ok(r) => r,
                        Err(msg) => {
                            print_error(&msg);
                            process::exit(1);
                        }
                    };
                    match request {
                        HelperRequest::Cleanup {
                            session_id,
                            project_dir,
                            sandbox_user,
                        } => handle_cleanup(&session_id, &project_dir, &sandbox_user).map(|()| 0),
                        _ => Err("Expected Cleanup request".into()),
                    }
                }
            }
        }
        "exec" => handle_exec(&args[2..]),
        "health-check" => respond_healthy(),
        _ => {
            print_error(&format!("Unknown action: {}", action));
            process::exit(1);
        }
    };

    match result {
        Ok(code) => process::exit(code),
        Err(msg) => {
            print_error(&msg);
            process::exit(1);
        }
    }
}

/// Validate sandbox_user and look up UID/GID before forking.
///
/// On error, cleans up any ACLs that were already set and returns Err.
fn prepare_spawn(
    sandbox_user: &str,
    acl_paths: &[AclEntry],
    home_dir: &Path,
) -> Result<SpawnReady, String> {
    mino::sandbox::config::validate_sandbox_user(sandbox_user).map_err(|e| e.to_string())?;

    let (uid, gid) = match get_user_ids(sandbox_user) {
        Ok(ids) => ids,
        Err(e) => {
            cleanup_acls(acl_paths, Some(home_dir), sandbox_user);
            return Err(e);
        }
    };

    Ok(SpawnReady { uid, gid })
}

fn handle_spawn(params: SpawnParams) -> Result<i32, String> {
    let SpawnParams {
        session_id,
        project_dir,
        env,
        command,
        resource_limits,
        acl_paths,
        dotfile_dir,
        home_dir,
        sandbox_user,
    } = params;

    // 0. Validate sandbox_user before any ACL or filesystem operations.
    //    This binary runs as root — reject malformed usernames immediately.
    mino::sandbox::config::validate_sandbox_user(&sandbox_user).map_err(|e| e.to_string())?;

    // 1. Create home directory
    std::fs::create_dir_all(&home_dir).map_err(|e| format!("Failed to create home dir: {}", e))?;

    // 2. Copy dotfiles to home
    if let Some(dotfile_src) = &dotfile_dir {
        copy_dotfiles(dotfile_src, &home_dir);
    }

    // 3. Set ACLs for sandbox user on all paths
    for acl in &acl_paths {
        set_acl(&acl.path, acl.writable, &sandbox_user)?;
    }

    // Set ACL on home dir
    if let Err(e) = set_acl(&home_dir, true, &sandbox_user) {
        cleanup_acls(&acl_paths, None, &sandbox_user);
        return Err(format!("ACL setup failed on home dir: {}", e));
    }

    // 4. Look up sandbox user UID and GID
    let ready = prepare_spawn(&sandbox_user, &acl_paths, &home_dir)?;

    // 5. Fork + setgid + setuid + exec
    #[cfg(unix)]
    {
        // SAFETY: fork() duplicates the process. The helper binary is single-threaded
        // (no tokio runtime, no background threads), so there is no risk of duplicating
        // locked mutexes. The child calls only async-signal-safe libc functions and
        // process::exit/exec before returning control.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err("fork() failed".to_string());
        }

        if pid == 0 {
            // SAFETY: child process after fork — single-threaded, owned resources
            unsafe {
                child_process(ChildArgs {
                    uid: ready.uid,
                    gid: ready.gid,
                    resource_limits: &resource_limits,
                    env: &env,
                    home_dir: &home_dir,
                    project_dir: &project_dir,
                    command: &command,
                    sandbox_user: &sandbox_user,
                });
            }
        } else {
            // SAFETY: parent process — monitors child, handles signals
            unsafe {
                parent_process(pid, &acl_paths, &home_dir, &session_id, &sandbox_user);
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (session_id, env, command, resource_limits, ready);
        Err("Spawn is only supported on Unix".to_string())
    }
}

/// Drop supplementary groups, then set GID and UID to the sandbox user.
///
/// # Safety
/// Calls libc setgroups, setgid, setuid — all FFI calls.
/// Order: setgid before setuid (after setuid we can't change GID).
/// Must run as root; after completion the process runs as the specified user.
#[cfg(unix)]
unsafe fn drop_privileges(uid: u32, gid: u32) -> Result<(), String> {
    if libc::setgroups(0, std::ptr::null()) != 0 {
        return Err("setgroups failed".into());
    }
    if libc::setgid(gid) != 0 {
        return Err("setgid failed".into());
    }
    if libc::setuid(uid) != 0 {
        return Err("setuid failed".into());
    }
    Ok(())
}

/// Execute a command as the sandbox user inside an existing session.
///
/// Unlike `spawn`, this does not set up ACLs or fork — it simply drops
/// privileges to the sandbox user and execs the command directly.
/// ACLs from the original `spawn` are still active on the session's paths.
///
/// Usage: mino-sandbox-helper exec --session-id <id> --sandbox-user <user> [--pid <pid>] -- <command...>
fn handle_exec(args: &[String]) -> Result<i32, String> {
    let parsed = parse_exec_args(args)?;

    // Validate session_id using the library function
    validate_session_name(parsed.session_id).map_err(|e| format!("Invalid session_id: {}", e))?;

    mino::sandbox::config::validate_sandbox_user(parsed.sandbox_user)
        .map_err(|e| format!("Invalid sandbox_user: {}", e))?;

    eprintln!(
        "[mino-helper] exec session={} command={:?}",
        parsed.session_id, &parsed.command[0]
    );

    let (uid, gid) = get_user_ids(parsed.sandbox_user)?;

    // SAFETY: drop_privileges calls setgroups/setgid/setuid — must run as root.
    #[cfg(unix)]
    unsafe {
        drop_privileges(uid, gid)?;
    }

    #[cfg(not(unix))]
    {
        let _ = (uid, gid);
        return Err("Exec is only supported on Unix".to_string());
    }

    // Build minimal env for exec (don't inherit root's environment)
    let home_dir = PathBuf::from(format!("/tmp/mino-home-{}", parsed.session_id));
    let exec_env = helper::build_exec_env(&home_dir, parsed.sandbox_user)
        .map_err(|e| format!("failed to build exec env: {}", e))?;

    // exec the command — this replaces the current process
    let err = exec_command(parsed.command, Some(&exec_env));
    Err(format!("exec failed: {}", err))
}

/// Child process: drop privileges and exec command
///
/// # Safety
/// Called after fork() in the child process. Uses libc functions for
/// setgid, setuid, and resource limits. The child process never returns
/// from this function — it either execs into the command or exits.
///
/// Privilege drop order: setgid MUST come before setuid, because once
/// we drop root UID we lose the ability to change our GID.
#[cfg(unix)]
unsafe fn child_process(args: ChildArgs<'_>) -> ! {
    // Set resource limits (must happen before dropping root)
    apply_resource_limits(args.resource_limits);

    // Drop privileges: setgid before setuid (can't change GID after dropping root)
    if let Err(e) = drop_privileges(args.uid, args.gid) {
        eprintln!("{}", e);
        process::exit(1);
    }

    // Build final environment using the library helper
    let final_env = match helper::build_child_env(args.env, args.home_dir, args.sandbox_user) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[mino-helper] failed to build child env: {}", e);
            process::exit(1);
        }
    };

    // Change to project dir
    if std::env::set_current_dir(args.project_dir).is_err() {
        eprintln!("Failed to chdir to {}", args.project_dir.display());
        process::exit(1);
    }

    // exec the command
    if args.command.is_empty() {
        eprintln!("Empty command");
        process::exit(1);
    }

    let err = exec_command(args.command, Some(&final_env));
    eprintln!("exec failed: {}", err);
    process::exit(1);
}

/// Parent process: forward signals, wait for child, clean up
///
/// # Safety
/// Called after fork() in the parent process. Uses libc signal handlers
/// and waitpid. The static CHILD_PID is only written once here (before
/// signal handlers can fire) and only read in the signal handler.
#[cfg(unix)]
unsafe fn parent_process(
    pid: i32,
    acl_paths: &[AclEntry],
    home_dir: &Path,
    _session_id: &str,
    sandbox_user: &str,
) -> ! {
    // Forward SIGINT and SIGTERM to child
    setup_signal_forwarding(pid);

    // Wait for child
    let mut status: libc::c_int = 0;
    let wait_result = libc::waitpid(pid, &mut status, 0);

    if wait_result < 0 {
        print_error("waitpid failed");
        process::exit(1);
    }

    let exit_code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    };

    // Clean up ACLs
    cleanup_acls(acl_paths, Some(home_dir), sandbox_user);
    let _ = std::fs::remove_dir_all(home_dir);

    print_response(&HelperResponse::Spawned { pid: pid as u32 });
    process::exit(exit_code);
}

fn handle_cleanup(session_id: &str, project_dir: &Path, sandbox_user: &str) -> Result<(), String> {
    // Validate inputs — this binary runs as root so all inputs must be checked
    validate_session_name(session_id).map_err(|e| format!("Invalid session_id: {}", e))?;
    mino::sandbox::config::validate_sandbox_user(sandbox_user)
        .map_err(|e| format!("Invalid sandbox_user: {}", e))?;

    // Remove ACLs on project dir
    let _ = remove_acl(project_dir, sandbox_user);

    // Remove pf sub-anchor using validated args from library
    if let Ok(pf_args) = helper::build_pf_cleanup_args(session_id) {
        let _ = std::process::Command::new("pfctl").args(&pf_args).output();
    }

    print_response(&HelperResponse::Cleaned);
    Ok(())
}

fn set_acl(path: &Path, writable: bool, sandbox_user: &str) -> Result<(), String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| "Invalid UTF-8 in path".to_string())?;

    let args = helper::build_acl_args(path_str, sandbox_user, writable);

    let output = std::process::Command::new("chmod")
        .args(&args)
        .output()
        .map_err(|e| format!("chmod +a failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("chmod +a failed: {}", stderr));
    }

    Ok(())
}

/// Remove ACLs from all tracked paths and optionally the home directory.
fn cleanup_acls(acl_paths: &[AclEntry], home_dir: Option<&Path>, sandbox_user: &str) {
    for acl in acl_paths {
        let _ = remove_acl(&acl.path, sandbox_user);
    }
    if let Some(home) = home_dir {
        let _ = remove_acl(home, sandbox_user);
    }
}

fn remove_acl(path: &Path, sandbox_user: &str) -> Result<(), String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("Path contains invalid UTF-8: {:?}", path))?;

    // Remove read-write ACL
    let rw_args = helper::build_remove_acl_args(path_str, sandbox_user, true);
    let _ = std::process::Command::new("chmod").args(&rw_args).output();

    // Remove read-only ACL
    let ro_args = helper::build_remove_acl_args(path_str, sandbox_user, false);
    let _ = std::process::Command::new("chmod").args(&ro_args).output();

    Ok(())
}

/// Look up both UID and GID for a macOS user in a single dscl call.
fn get_user_ids(username: &str) -> Result<(u32, u32), String> {
    let output = std::process::Command::new("dscl")
        .args([
            ".",
            "-read",
            &format!("/Users/{}", username),
            "UniqueID",
            "PrimaryGroupID",
        ])
        .output()
        .map_err(|e| format!("dscl failed: {}", e))?;

    if !output.status.success() {
        return Err(format!("User '{}' not found", username));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    helper::parse_dscl_ids(&stdout).map_err(|e| e.to_string())
}

/// Apply POSIX resource limits via setrlimit
///
/// # Safety
/// Calls libc::setrlimit which is an FFI call. Safe when called with
/// valid rlimit values. Zero values are treated as "no limit" and skipped.
/// Failures are logged to stderr but are non-fatal — the sandbox still
/// runs with default OS limits for the failed resource.
#[cfg(unix)]
unsafe fn apply_resource_limits(limits: &ResourceLimitsDto) {
    set_rlimit(libc::RLIMIT_AS, limits.max_memory_bytes, "RLIMIT_AS");
    set_rlimit(
        libc::RLIMIT_NPROC,
        u64::from(limits.max_processes),
        "RLIMIT_NPROC",
    );
    set_rlimit(libc::RLIMIT_CPU, limits.max_cpu_seconds, "RLIMIT_CPU");
    set_rlimit(
        libc::RLIMIT_FSIZE,
        limits.max_file_size_bytes,
        "RLIMIT_FSIZE",
    );
}

/// Set a single resource limit. Zero values are skipped (no limit).
///
/// # Safety
/// Calls libc::setrlimit. Must be called before dropping root privileges.
#[cfg(unix)]
unsafe fn set_rlimit(resource: libc::c_int, value: u64, name: &str) {
    if value == 0 {
        return;
    }
    let rlim = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if libc::setrlimit(resource, &rlim) != 0 {
        eprintln!(
            "[mino-helper] setrlimit {} failed: {}",
            name,
            std::io::Error::last_os_error()
        );
    }
}

/// Execute a command, optionally with a custom environment.
///
/// With `Some(env)`: clears the process environment and sets only the provided vars.
/// With `None`: inherits the current environment.
///
/// On success, this function never returns (the process image is replaced).
/// On failure, returns the IO error from the exec attempt.
#[cfg(unix)]
fn exec_command(command: &[String], env: Option<&HashMap<String, String>>) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    if let Some(env_map) = env {
        cmd.env_clear();
        cmd.envs(env_map);
    }
    cmd.exec()
}

/// Set up signal forwarding to child process
///
/// # Safety
/// Installs signal handlers via `sigaction(2)` with `SA_RESTART`.
/// Must be called only once, from the parent process after fork().
#[cfg(unix)]
unsafe fn setup_signal_forwarding(child_pid: i32) {
    CHILD_PID.store(child_pid, Ordering::SeqCst);

    let mut action: libc::sigaction = std::mem::zeroed();
    action.sa_sigaction = forward_signal as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    libc::sigemptyset(&mut action.sa_mask);

    libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
}

/// Global child PID for signal forwarding
///
/// Stored as an atomic to avoid `static mut` unsoundness.
/// Written once in parent_process() before signal handlers fire,
/// read only in the signal handler. Single-threaded binary.
#[cfg(unix)]
static CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// C-compatible signal handler that forwards signals to the child
///
/// # Safety
/// This is a signal handler. It only calls async-signal-safe functions
/// (libc::kill). Reads CHILD_PID atomically; the value was stored before
/// handler installation.
#[cfg(unix)]
extern "C" fn forward_signal(sig: libc::c_int) {
    let pid = CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

fn copy_dotfiles(src: &Path, dest: &Path) {
    if let Ok(entries) = std::fs::read_dir(src) {
        for entry in entries.flatten() {
            let src_path = entry.path();
            let file_name = entry.file_name();
            let dest_path = dest.join(&file_name);

            // Skip symlinks to prevent data exfiltration when running as root.
            // symlink_metadata() does NOT follow symlinks, unlike metadata().
            let metadata = match std::fs::symlink_metadata(&src_path) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!(
                        "[mino-helper] skipping dotfile (metadata error): {}: {}",
                        src_path.display(),
                        e
                    );
                    continue;
                }
            };

            if metadata.file_type().is_symlink() {
                eprintln!(
                    "[mino-helper] skipping symlink in dotfiles: {}",
                    src_path.display()
                );
                continue;
            }

            if metadata.is_dir() {
                if let Err(e) = std::fs::create_dir_all(&dest_path) {
                    eprintln!(
                        "[mino-helper] failed to create dir {}: {}",
                        dest_path.display(),
                        e
                    );
                    continue;
                }
                copy_dotfiles(&src_path, &dest_path);
            } else if let Err(e) = std::fs::copy(&src_path, &dest_path) {
                eprintln!(
                    "[mino-helper] failed to copy dotfile {} -> {}: {}",
                    src_path.display(),
                    dest_path.display(),
                    e
                );
            }
        }
    }
}

fn respond_healthy() -> Result<i32, String> {
    print_response(&HelperResponse::Healthy {
        version: env!("CARGO_PKG_VERSION").to_string(),
    });
    Ok(0)
}

fn print_response(response: &HelperResponse) {
    if let Ok(json) = serde_json::to_string(response) {
        println!("{}", json);
    }
}

fn print_error(message: &str) {
    print_response(&HelperResponse::Error {
        message: message.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert a slice of &str into Vec<String> for test convenience.
    fn args(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_exec_args_valid() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "--",
            "bash",
            "-c",
            "echo hello",
        ]);
        let parsed = parse_exec_args(&input).unwrap();
        assert_eq!(parsed.session_id, "my-session");
        assert_eq!(parsed.sandbox_user, "_mino_agent");
        assert_eq!(parsed.command.len(), 3);
        assert_eq!(parsed.command[0], "bash");
        assert_eq!(parsed.command[1], "-c");
        assert_eq!(parsed.command[2], "echo hello");
    }

    #[test]
    fn parse_exec_args_missing_session_id() {
        let input = args(&["--sandbox-user", "_mino_agent", "--", "bash"]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("--session-id"),
            "expected error about --session-id, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_missing_sandbox_user() {
        let input = args(&["--session-id", "my-session", "--", "bash"]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("--sandbox-user"),
            "expected error about --sandbox-user, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_missing_separator() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "bash",
        ]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("command") || err.contains("--"),
            "expected error about missing command/separator, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_empty_command_after_separator() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "--",
        ]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("command"),
            "expected error about missing/empty command, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_pid_flag_accepted_and_ignored() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "--pid",
            "12345",
            "--",
            "ls",
        ]);
        let parsed = parse_exec_args(&input).unwrap();
        assert_eq!(parsed.session_id, "my-session");
        assert_eq!(parsed.sandbox_user, "_mino_agent");
        assert_eq!(parsed.command.len(), 1);
        assert_eq!(parsed.command[0], "ls");
    }

    // ---- parse_cleanup_args tests ----

    #[test]
    fn parse_cleanup_args_valid() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--project-dir",
            "/home/user/project",
            "--sandbox-user",
            "_mino_agent",
        ]);
        let parsed = parse_cleanup_args(&input).unwrap();
        assert_eq!(parsed.session_id, "my-session");
        assert_eq!(parsed.project_dir, PathBuf::from("/home/user/project"));
        assert_eq!(parsed.sandbox_user, "_mino_agent");
    }

    #[test]
    fn parse_cleanup_args_missing_session_id() {
        let input = args(&[
            "--project-dir",
            "/tmp/proj",
            "--sandbox-user",
            "_mino_agent",
        ]);
        let err = parse_cleanup_args(&input).unwrap_err();
        assert!(
            err.contains("--session-id"),
            "expected error about --session-id, got: {}",
            err
        );
    }

    #[test]
    fn parse_cleanup_args_missing_project_dir() {
        let input = args(&["--session-id", "s1", "--sandbox-user", "_mino_agent"]);
        let err = parse_cleanup_args(&input).unwrap_err();
        assert!(
            err.contains("--project-dir"),
            "expected error about --project-dir, got: {}",
            err
        );
    }

    #[test]
    fn parse_cleanup_args_missing_sandbox_user() {
        let input = args(&["--session-id", "s1", "--project-dir", "/tmp/proj"]);
        let err = parse_cleanup_args(&input).unwrap_err();
        assert!(
            err.contains("--sandbox-user"),
            "expected error about --sandbox-user, got: {}",
            err
        );
    }
}
