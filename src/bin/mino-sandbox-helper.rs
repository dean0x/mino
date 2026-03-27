//! Mino sandbox helper - macOS privileged helper binary
//!
//! Installed to /usr/local/bin/ during `mino setup --native`.
//! Called via sudoers.d: `<user> ALL=(root) NOPASSWD: /usr/local/bin/mino-sandbox-helper`
//!
//! Operations:
//! - spawn: Set ACLs, create pf sub-anchor, fork+setuid to _mino_agent, exec command
//! - exec: Drop privileges to _mino_agent and exec a command (no ACL setup, no fork)
//! - cleanup: Remove ACLs, remove pf sub-anchor
//! - health-check: Return version

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicI32, Ordering};

use mino::sandbox::helper_protocol::*;

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

    // Find --request-file arg
    let request_file = args
        .iter()
        .position(|a| a == "--request-file")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    match action.as_str() {
        "spawn" | "cleanup" => {
            let request_file = match request_file {
                Some(f) => f,
                None => {
                    print_error("Missing --request-file argument");
                    process::exit(1);
                }
            };

            let request_json = match std::fs::read_to_string(&request_file) {
                Ok(s) => s,
                Err(e) => {
                    print_error(&format!("Failed to read request file: {}", e));
                    process::exit(1);
                }
            };

            let request: HelperRequest = match serde_json::from_str(&request_json) {
                Ok(r) => r,
                Err(e) => {
                    print_error(&format!("Failed to parse request: {}", e));
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
                } => {
                    handle_spawn(SpawnParams {
                        session_id,
                        project_dir,
                        env,
                        command,
                        resource_limits,
                        acl_paths,
                        dotfile_dir,
                        home_dir,
                    });
                }
                HelperRequest::Cleanup {
                    session_id,
                    project_dir,
                } => {
                    handle_cleanup(&session_id, &project_dir);
                }
                HelperRequest::HealthCheck => {
                    print_response(&HelperResponse::Healthy {
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    });
                }
            }
        }
        "exec" => {
            handle_exec(&args[2..]);
        }
        "health-check" => {
            print_response(&HelperResponse::Healthy {
                version: env!("CARGO_PKG_VERSION").to_string(),
            });
        }
        _ => {
            print_error(&format!("Unknown action: {}", action));
            process::exit(1);
        }
    }
}

fn handle_spawn(params: SpawnParams) {
    let SpawnParams {
        session_id,
        project_dir,
        env,
        command,
        resource_limits,
        acl_paths,
        dotfile_dir,
        home_dir,
    } = params;

    // 1. Create home directory
    if let Err(e) = std::fs::create_dir_all(&home_dir) {
        print_error(&format!("Failed to create home dir: {}", e));
        process::exit(1);
    }

    // 2. Copy dotfiles to home
    if let Some(dotfile_src) = &dotfile_dir {
        copy_dotfiles(dotfile_src, &home_dir);
    }

    // 3. Set ACLs for _mino_agent on all paths
    for acl in &acl_paths {
        if let Err(e) = set_acl(&acl.path, acl.writable) {
            print_error(&format!(
                "Failed to set ACL on {}: {}",
                acl.path.display(),
                e
            ));
            process::exit(1);
        }
    }

    // Set ACL on home dir
    if let Err(e) = set_acl(&home_dir, true) {
        print_error(&format!(
            "Failed to set ACL on home dir: {}",
            home_dir.display()
        ));
        // Clean up ACLs already set
        for acl in &acl_paths {
            let _ = remove_acl(&acl.path);
        }
        print_error(&format!("ACL setup failed: {}", e));
        process::exit(1);
    }

    // 4. Look up _mino_agent UID and GID
    let sandbox_user = "_mino_agent";
    let uid = match get_user_uid(sandbox_user) {
        Some(uid) => uid,
        None => {
            print_error(&format!("User '{}' not found", sandbox_user));
            // Clean up
            for acl in &acl_paths {
                let _ = remove_acl(&acl.path);
            }
            let _ = remove_acl(&home_dir);
            process::exit(1);
        }
    };
    let gid = match get_user_gid(sandbox_user) {
        Some(gid) => gid,
        None => {
            print_error(&format!("GID for user '{}' not found", sandbox_user));
            // Clean up
            for acl in &acl_paths {
                let _ = remove_acl(&acl.path);
            }
            let _ = remove_acl(&home_dir);
            process::exit(1);
        }
    };

    // 5. Fork + setgid + setuid + exec
    // The parent stays alive to relay signals and report exit code
    #[cfg(unix)]
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            print_error("fork() failed");
            process::exit(1);
        }

        if pid == 0 {
            // Child process
            child_process(
                uid,
                gid,
                &resource_limits,
                &env,
                &home_dir,
                &project_dir,
                &command,
            );
        } else {
            // Parent process — wait for child, relay signals
            parent_process(pid, &acl_paths, &home_dir, &session_id);
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (session_id, env, command, resource_limits, uid, gid);
        print_error("Spawn is only supported on Unix");
        process::exit(1);
    }
}

/// Execute a command as `_mino_agent` inside an existing session.
///
/// Unlike `spawn`, this does not set up ACLs or fork — it simply drops
/// privileges to the sandbox user and execs the command directly.
/// ACLs from the original `spawn` are still active on the session's paths.
///
/// Usage: mino-sandbox-helper exec --session-id <id> [--pid <pid>] -- <command...>
fn handle_exec(args: &[String]) {
    let mut session_id: Option<&str> = None;
    let mut command_start: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session-id" => {
                session_id = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--pid" => {
                // Accepted for logging but not used for exec
                i += 2;
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
        _ => {
            print_error("Missing --session-id argument");
            process::exit(1);
        }
    };

    // Validate session_id: must be alphanumeric plus hyphen/underscore.
    // Same validation as handle_cleanup to prevent path injection.
    if !session_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        print_error(&format!("Invalid session_id: {:?}", session_id));
        process::exit(1);
    }

    let command: &[String] = match command_start {
        Some(idx) if idx < args.len() => &args[idx..],
        _ => {
            print_error("Missing command after '--'");
            process::exit(1);
        }
    };

    if command.is_empty() {
        print_error("Empty command");
        process::exit(1);
    }

    eprintln!(
        "[mino-helper] exec session={} command={:?}",
        session_id, &command[0]
    );

    let sandbox_user = "_mino_agent";
    let uid = match get_user_uid(sandbox_user) {
        Some(uid) => uid,
        None => {
            print_error(&format!("User '{}' not found", sandbox_user));
            process::exit(1);
        }
    };
    let gid = match get_user_gid(sandbox_user) {
        Some(gid) => gid,
        None => {
            print_error(&format!("GID for user '{}' not found", sandbox_user));
            process::exit(1);
        }
    };

    #[cfg(unix)]
    unsafe {
        // Drop supplementary groups
        if libc::setgroups(0, std::ptr::null()) != 0 {
            eprintln!("setgroups failed");
            process::exit(1);
        }

        // setgid before setuid (after setuid we can't change GID)
        if libc::setgid(gid) != 0 {
            eprintln!("setgid failed");
            process::exit(1);
        }

        // setuid to _mino_agent (drops root)
        if libc::setuid(uid) != 0 {
            eprintln!("setuid failed");
            process::exit(1);
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (uid, gid, session_id);
        print_error("Exec is only supported on Unix");
        process::exit(1);
    }

    // exec the command — this replaces the current process
    let err = exec_command(command);
    eprintln!("exec failed: {}", err);
    process::exit(1);
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
unsafe fn child_process(
    uid: u32,
    gid: u32,
    resource_limits: &ResourceLimitsDto,
    env: &HashMap<String, String>,
    home_dir: &Path,
    project_dir: &Path,
    command: &[String],
) -> ! {
    // Set resource limits (must happen before dropping root)
    apply_resource_limits(resource_limits);

    // Drop supplementary groups first
    if libc::setgroups(0, std::ptr::null()) != 0 {
        eprintln!("setgroups failed");
        process::exit(1);
    }

    // setgid MUST come before setuid — after setuid we can't change GID
    if libc::setgid(gid) != 0 {
        eprintln!("setgid failed");
        process::exit(1);
    }

    // setuid to _mino_agent (drops root)
    if libc::setuid(uid) != 0 {
        eprintln!("setuid failed");
        process::exit(1);
    }

    // Clear environment
    for (key, _) in std::env::vars() {
        std::env::remove_var(&key);
    }

    // Set sandbox environment
    for (key, value) in env {
        std::env::set_var(key, value);
    }
    std::env::set_var("HOME", home_dir.to_str().unwrap_or("/tmp"));
    std::env::set_var("USER", "_mino_agent");

    // Change to project dir
    if std::env::set_current_dir(project_dir).is_err() {
        eprintln!("Failed to chdir to {}", project_dir.display());
        process::exit(1);
    }

    // exec the command
    if command.is_empty() {
        eprintln!("Empty command");
        process::exit(1);
    }

    let err = exec_command(command);
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
    for acl in acl_paths {
        let _ = remove_acl(&acl.path);
    }
    let _ = remove_acl(home_dir);
    let _ = std::fs::remove_dir_all(home_dir);

    print_response(&HelperResponse::Spawned { pid: pid as u32 });
    process::exit(exit_code);
}

fn handle_cleanup(session_id: &str, project_dir: &Path) {
    // Validate session_id to prevent anchor path injection.
    // Must be alphanumeric plus hyphen/underscore — no slashes, spaces, or
    // special characters that could reference arbitrary pf anchors.
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        print_error(&format!("Invalid session_id: {:?}", session_id));
        process::exit(1);
    }

    // Remove ACLs on project dir
    let _ = remove_acl(project_dir);

    // Remove pf sub-anchor
    let _ = std::process::Command::new("pfctl")
        .args(["-a", &format!("mino/session-{}", session_id), "-F", "rules"])
        .output();

    print_response(&HelperResponse::Cleaned);
}

fn set_acl(path: &Path, writable: bool) -> Result<(), String> {
    let perms = if writable {
        "allow read,write,execute,file_inherit,directory_inherit"
    } else {
        "allow read,execute,file_inherit,directory_inherit"
    };

    let path_str = path
        .to_str()
        .ok_or_else(|| "Invalid UTF-8 in path".to_string())?;

    let output = std::process::Command::new("chmod")
        .args(["+a", &format!("_mino_agent {}", perms), path_str])
        .output()
        .map_err(|e| format!("chmod +a failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("chmod +a failed: {}", stderr));
    }

    Ok(())
}

fn remove_acl(path: &Path) -> Result<(), String> {
    let path_str = path.to_str().unwrap_or("");

    // Remove read-write ACL
    let _ = std::process::Command::new("chmod")
        .args([
            "-a",
            "_mino_agent allow read,write,execute,file_inherit,directory_inherit",
            path_str,
        ])
        .output();

    // Remove read-only ACL
    let _ = std::process::Command::new("chmod")
        .args([
            "-a",
            "_mino_agent allow read,execute,file_inherit,directory_inherit",
            path_str,
        ])
        .output();

    Ok(())
}

fn get_user_uid(username: &str) -> Option<u32> {
    let output = std::process::Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", username), "UniqueID"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Output format: "UniqueID: 502"
    stdout.split_whitespace().last()?.parse().ok()
}

fn get_user_gid(username: &str) -> Option<u32> {
    let output = std::process::Command::new("dscl")
        .args([
            ".",
            "-read",
            &format!("/Users/{}", username),
            "PrimaryGroupID",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Output format: "PrimaryGroupID: 20"
    stdout.split_whitespace().last()?.parse().ok()
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
    if limits.max_memory_bytes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: limits.max_memory_bytes,
            rlim_max: limits.max_memory_bytes,
        };
        if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
            eprintln!(
                "[mino-helper] setrlimit RLIMIT_AS failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    if limits.max_processes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: u64::from(limits.max_processes),
            rlim_max: u64::from(limits.max_processes),
        };
        if libc::setrlimit(libc::RLIMIT_NPROC, &rlim) != 0 {
            eprintln!(
                "[mino-helper] setrlimit RLIMIT_NPROC failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    if limits.max_cpu_seconds > 0 {
        let rlim = libc::rlimit {
            rlim_cur: limits.max_cpu_seconds,
            rlim_max: limits.max_cpu_seconds,
        };
        if libc::setrlimit(libc::RLIMIT_CPU, &rlim) != 0 {
            eprintln!(
                "[mino-helper] setrlimit RLIMIT_CPU failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    if limits.max_file_size_bytes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: limits.max_file_size_bytes,
            rlim_max: limits.max_file_size_bytes,
        };
        if libc::setrlimit(libc::RLIMIT_FSIZE, &rlim) != 0 {
            eprintln!(
                "[mino-helper] setrlimit RLIMIT_FSIZE failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(unix)]
fn exec_command(command: &[String]) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(&command[0])
        .args(&command[1..])
        .exec()
}

/// Set up signal forwarding to child process
///
/// # Safety
/// Installs C-style signal handlers via libc::signal.
/// Must be called only once, from the parent process after fork().
#[cfg(unix)]
unsafe fn setup_signal_forwarding(child_pid: i32) {
    CHILD_PID.store(child_pid, Ordering::SeqCst);
    libc::signal(libc::SIGINT, forward_signal as *const () as usize);
    libc::signal(libc::SIGTERM, forward_signal as *const () as usize);
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
                let _ = std::fs::create_dir_all(&dest_path);
                copy_dotfiles(&src_path, &dest_path);
            } else {
                let _ = std::fs::copy(&src_path, &dest_path);
            }
        }
    }
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
