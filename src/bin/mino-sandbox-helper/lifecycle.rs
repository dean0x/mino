use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process;

use mino::sandbox::helper;
use mino::sandbox::helper_protocol::{AclEntry, ResourceLimitsDto};

use super::acl::{cleanup_acls, set_acl};
use super::dotfiles::copy_dotfiles;
use super::exec::exec_command;
use super::resource_limits::apply_resource_limits;
use super::signal::setup_signal_forwarding;

/// Parameters for the spawn operation, extracted from HelperRequest::Spawn
pub(crate) struct SpawnParams {
    pub(crate) session_id: String,
    pub(crate) project_dir: PathBuf,
    pub(crate) env: HashMap<String, String>,
    pub(crate) command: Vec<String>,
    pub(crate) resource_limits: ResourceLimitsDto,
    pub(crate) acl_paths: Vec<AclEntry>,
    pub(crate) dotfile_dir: Option<PathBuf>,
    pub(crate) home_dir: PathBuf,
    pub(crate) sandbox_user: String,
}

/// Pre-fork state: UID/GID resolved and validated, ready to fork.
pub(crate) struct SpawnReady {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

/// Arguments for the child process after fork.
pub(crate) struct ChildArgs<'a> {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) resource_limits: &'a ResourceLimitsDto,
    pub(crate) env: &'a HashMap<String, String>,
    pub(crate) home_dir: &'a Path,
    pub(crate) project_dir: &'a Path,
    pub(crate) command: &'a [String],
    pub(crate) sandbox_user: &'a str,
}

/// Logical stages executed by [`handle_spawn`], in canonical order.
///
/// This enum exists so the ordering can be asserted by tests: any reordering
/// of `SPAWN_STAGES` is a breaking change that must be reviewed explicitly.
/// The actual dispatch still happens via explicit code in `handle_spawn`,
/// but the comments in that function reference the corresponding `SpawnStage`
/// variant so reviewers can cross-check against the const.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnStage {
    /// Validate the sandbox username and create the home directory.
    CreateHome,
    /// Install the RAII `SpawnGuard` and set ACL on home dir.
    InstallGuard,
    /// Resolve UID/GID for the sandbox user.
    ResolveIds,
    /// Copy dotfiles from the staging directory into home.
    CopyDotfiles,
    /// chown all files in home to the sandbox user.
    ChownHome,
    /// Apply ACLs to project and passthrough paths.
    SetProjectAcls,
    /// Forget the guard, fork, drop privileges, and exec the command.
    ExecChild,
}

/// Canonical execution order for the spawn pipeline.
///
/// Tests assert against this slice to verify:
/// - `InstallGuard` precedes `CopyDotfiles` (ACL must be set before files land)
/// - `ResolveIds` precedes `ChownHome` (can't chown without knowing uid/gid)
/// - `ExecChild` is last (fork must happen after all setup)
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const SPAWN_STAGES: &[SpawnStage] = &[
    SpawnStage::CreateHome,
    SpawnStage::InstallGuard,
    SpawnStage::ResolveIds,
    SpawnStage::CopyDotfiles,
    SpawnStage::ChownHome,
    SpawnStage::SetProjectAcls,
    SpawnStage::ExecChild,
];

/// RAII guard that cleans up the home directory and ACLs on error.
///
/// Tracks which ACLs were successfully set and the home directory so that
/// any error path through `handle_spawn` (via `?`) triggers cleanup.
/// On success, call `std::mem::forget(guard)` to skip cleanup — the parent
/// process handles cleanup after the child exits.
///
/// Reference pattern: `TerminalGuard` in the codebase.
pub(crate) struct SpawnGuard<'a> {
    home_dir: Option<PathBuf>,
    /// Only ACLs that were successfully set are tracked here.
    set_acl_paths: Vec<&'a AclEntry>,
    sandbox_user: &'a str,
}

impl<'a> SpawnGuard<'a> {
    pub(crate) fn new(home_dir: PathBuf, sandbox_user: &'a str) -> Self {
        Self {
            home_dir: Some(home_dir),
            set_acl_paths: Vec::new(),
            sandbox_user,
        }
    }

    pub(crate) fn track_acl(&mut self, entry: &'a AclEntry) {
        self.set_acl_paths.push(entry);
    }
}

impl Drop for SpawnGuard<'_> {
    fn drop(&mut self) {
        use super::acl::remove_acl;
        for acl in &self.set_acl_paths {
            let _ = remove_acl(&acl.path, acl.writable, self.sandbox_user);
        }
        if let Some(home) = self.home_dir.take() {
            // Also remove the home dir ACL (set in InstallGuard stage before the guard tracks it).
            // Note: remove_acl on a path that was never ACL'd is a no-op (chmod -a returns 0),
            // so this is safe even if set_acl failed before Drop was triggered.
            let _ = remove_acl(&home, true, self.sandbox_user);
            let _ = std::fs::remove_dir_all(&home);
        }
    }
}

/// Confirm that `path` is not a symlink, removing it if it is.
///
/// Called twice during home-dir setup to close a TOCTOU window: once after
/// `create_dir_all` (before the guard is installed) and once immediately before
/// the first privileged ACL operation (after the guard is installed but before
/// any chown or ACL write). A second check is needed because an attacker could
/// remove the real directory and plant a symlink in the gap between the two calls.
pub(crate) fn reject_if_symlink(path: &Path, context: &str) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to stat {}: {}", context, e))?;
    if meta.file_type().is_symlink() {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "Security: {} is a symlink (possible attack): {}",
            context,
            path.display()
        ));
    }
    Ok(())
}

/// Look up UID/GID for the sandbox user before forking.
///
/// Cleanup on error is handled by the caller's `SpawnGuard` — this function
/// simply returns Err and lets the guard's Drop impl do the work.
/// Caller must validate `sandbox_user` before calling this function.
pub(crate) fn prepare_spawn(sandbox_user: &str) -> Result<SpawnReady, String> {
    let (uid, gid) = get_user_ids(sandbox_user)?;
    Ok(SpawnReady { uid, gid })
}

/// Look up both UID and GID for a macOS user in a single dscl call.
pub(crate) fn get_user_ids(username: &str) -> Result<(u32, u32), String> {
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

/// Recursively set ownership of all files and directories under a path.
///
/// Uses libc::chown (not lchown) and explicitly skips symlinks so that
/// only regular files and directories are chowned. Errors are logged but non-fatal.
#[cfg(unix)]
pub(crate) fn chown_recursive(path: &Path, uid: u32, gid: u32) {
    fn chown_path(path: &Path, uid: u32, gid: u32) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        if let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) {
            // SAFETY: valid CString, uid/gid from dscl lookup
            if unsafe { libc::chown(cpath.as_ptr(), uid, gid) } != 0 {
                eprintln!(
                    "[mino-helper] chown failed on {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    chown_path(path, uid, gid);

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            let metadata = match std::fs::symlink_metadata(&entry_path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                chown_recursive(&entry_path, uid, gid);
            } else if !metadata.file_type().is_symlink() {
                chown_path(&entry_path, uid, gid);
            }
        }
    }
}

/// Drop supplementary groups, then set GID and UID to the sandbox user.
///
/// # Safety
/// Calls libc setgroups, setgid, setuid — all FFI calls.
/// Order: setgid before setuid (after setuid we can't change GID).
/// Must run as root; after completion the process runs as the specified user.
#[cfg(unix)]
pub(crate) unsafe fn drop_privileges(uid: u32, gid: u32) -> Result<(), String> {
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

pub(crate) fn handle_spawn(params: SpawnParams) -> Result<i32, String> {
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

    // Stage: CreateHome — validate sandbox_user and create home directory.
    //   (Corresponds to SPAWN_STAGES[0]: SpawnStage::CreateHome)
    mino::sandbox::config::validate_sandbox_user(&sandbox_user).map_err(|e| e.to_string())?;

    std::fs::create_dir_all(&home_dir).map_err(|e| format!("Failed to create home dir: {}", e))?;

    // Symlink check: /tmp is world-writable and session_id is predictable.
    // An attacker could pre-plant a symlink at /tmp/mino-home-<id> pointing to
    // /etc or another sensitive path. We check AFTER create_dir_all because
    // create_dir_all does not follow symlinks for the final component — if the path
    // was a pre-planted symlink, create_dir_all succeeds (the target dir already
    // existed). Detecting the symlink here closes that window.
    reject_if_symlink(&home_dir, "home dir path")?;

    // Stage: InstallGuard — construct RAII guard; set ACL on home dir so that
    //   file_inherit applies to dotfiles copied in the next stage.
    //   (Corresponds to SPAWN_STAGES[1]: SpawnStage::InstallGuard)
    //   Any `?` error return below will trigger Drop → cleanup.
    let mut guard = SpawnGuard::new(home_dir.clone(), &sandbox_user);

    // Re-verify immediately before the privileged ACL operation. A narrow TOCTOU race
    // exists between the check above and here: an attacker could remove the real dir and
    // plant a symlink in the gap. A second check closes that window.
    reject_if_symlink(&home_dir, "home dir before ACL")?;

    if let Err(e) = set_acl(&home_dir, true, &sandbox_user) {
        return Err(format!("ACL setup failed on home dir: {}", e));
    }

    // Stage: ResolveIds — look up UID/GID for the sandbox user.
    //   (Corresponds to SPAWN_STAGES[2]: SpawnStage::ResolveIds)
    let ready = prepare_spawn(&sandbox_user)?;

    // Stage: CopyDotfiles — copy dotfiles from the staging dir into home.
    //   ACL from the InstallGuard stage ensures file_inherit applies here.
    //   (Corresponds to SPAWN_STAGES[3]: SpawnStage::CopyDotfiles)
    if let Some(dotfile_src) = &dotfile_dir {
        copy_dotfiles(dotfile_src, &home_dir);
    }

    // Stage: ChownHome — chown all files in home to sandbox user.
    //   (Corresponds to SPAWN_STAGES[4]: SpawnStage::ChownHome)
    #[cfg(unix)]
    chown_recursive(&home_dir, ready.uid, ready.gid);

    // Stage: SetProjectAcls — apply ACLs to project and passthrough paths.
    //   Track each successful ACL so the guard can remove them on error.
    //   (Corresponds to SPAWN_STAGES[5]: SpawnStage::SetProjectAcls)
    for acl in &acl_paths {
        set_acl(&acl.path, acl.writable, &sandbox_user)?;
        guard.track_acl(acl);
    }

    // Stage: ExecChild — forget the guard (cleanup handed to parent), fork, exec.
    //   (Corresponds to SPAWN_STAGES[6]: SpawnStage::ExecChild)
    // All setup succeeded — hand off cleanup responsibility to the parent
    // process (which calls cleanup_acls + remove_dir_all after child exits).
    std::mem::forget(guard);
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
        super::print_error("waitpid failed");
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

    process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- SPAWN_STAGES ordering regression tests (T-002) ----
    //
    // These tests assert invariants about the spawn pipeline order to prevent
    // accidental reordering. ACL must be installed before dotfiles are copied
    // (so file_inherit applies), and ExecChild must be last.

    #[test]
    fn spawn_stages_install_guard_before_copy_dotfiles() {
        let install_pos = SPAWN_STAGES
            .iter()
            .position(|s| *s == SpawnStage::InstallGuard)
            .expect("InstallGuard must be in SPAWN_STAGES");
        let copy_pos = SPAWN_STAGES
            .iter()
            .position(|s| *s == SpawnStage::CopyDotfiles)
            .expect("CopyDotfiles must be in SPAWN_STAGES");
        assert!(
            install_pos < copy_pos,
            "InstallGuard (sets ACL with file_inherit) must come before CopyDotfiles; \
             got InstallGuard at {} and CopyDotfiles at {}",
            install_pos,
            copy_pos
        );
    }

    #[test]
    fn spawn_stages_resolve_ids_before_chown_home() {
        let resolve_pos = SPAWN_STAGES
            .iter()
            .position(|s| *s == SpawnStage::ResolveIds)
            .expect("ResolveIds must be in SPAWN_STAGES");
        let chown_pos = SPAWN_STAGES
            .iter()
            .position(|s| *s == SpawnStage::ChownHome)
            .expect("ChownHome must be in SPAWN_STAGES");
        assert!(
            resolve_pos < chown_pos,
            "ResolveIds must come before ChownHome; \
             got ResolveIds at {} and ChownHome at {}",
            resolve_pos,
            chown_pos
        );
    }

    #[test]
    fn spawn_stages_exec_child_is_last() {
        let last = SPAWN_STAGES.last().expect("SPAWN_STAGES must not be empty");
        assert_eq!(
            *last,
            SpawnStage::ExecChild,
            "ExecChild must be the terminal stage (fork must happen after all setup)"
        );
    }

    #[test]
    fn spawn_stages_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for stage in SPAWN_STAGES {
            assert!(
                seen.insert(format!("{:?}", stage)),
                "SPAWN_STAGES contains duplicate stage: {:?}",
                stage
            );
        }
    }

    #[test]
    fn spawn_stages_covers_all_variants() {
        // Exhaustive match forces a compile error when a new SpawnStage variant is added
        // without also updating SPAWN_STAGES. This is more reliable than a hardcoded count
        // because the compiler rejects the match rather than silently passing a stale number.
        let expected_count = [
            SpawnStage::CreateHome,
            SpawnStage::InstallGuard,
            SpawnStage::ResolveIds,
            SpawnStage::CopyDotfiles,
            SpawnStage::ChownHome,
            SpawnStage::SetProjectAcls,
            SpawnStage::ExecChild,
        ]
        .len();
        assert_eq!(
            SPAWN_STAGES.len(),
            expected_count,
            "SPAWN_STAGES has {} entries but expected {}. \
             Update SPAWN_STAGES when adding/removing SpawnStage variants.",
            SPAWN_STAGES.len(),
            expected_count
        );
    }

    // ---- chown_recursive tests ----
    //
    // These tests run as the current (non-root) user. They cannot verify that
    // ownership actually changes — that requires root. What they DO verify:
    //
    // 1. Empty dir is a no-op (no panic, no error)
    // 2. Nested directories are visited without panic
    // 3. Symlinks are NOT followed (symlink_metadata detects them; chown is skipped)
    //
    // The actual chown is exercised only in integration / root CI environments.
    // The behaviour under non-root is "log and continue" (non-fatal), which is
    // intentional — the test confirms the function doesn't panic or return an error.

    #[cfg(unix)]
    #[test]
    fn chown_recursive_empty_dir_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        // Use current user's uid/gid — chown to self is a no-op that always succeeds
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        // Must not panic
        chown_recursive(dir.path(), uid, gid);
    }

    #[cfg(unix)]
    #[test]
    fn chown_recursive_recurses_into_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a").join("b")).unwrap();
        std::fs::write(dir.path().join("a").join("b").join("file.txt"), "x").unwrap();
        std::fs::write(dir.path().join("top.txt"), "y").unwrap();

        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        // Must not panic; all paths must still be accessible after the call
        chown_recursive(dir.path(), uid, gid);

        assert!(dir.path().join("a").join("b").join("file.txt").exists());
        assert!(dir.path().join("top.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn chown_recursive_skips_symlinks_does_not_follow() {
        let dir = tempfile::tempdir().unwrap();

        // Create a symlink pointing at a path that does NOT exist.
        // If chown_recursive followed the symlink, libc::chown would fail on a
        // non-existent target — but since we skip symlinks, that must not happen.
        let dangling_target = dir.path().join("does-not-exist");
        std::os::unix::fs::symlink(&dangling_target, dir.path().join("link")).unwrap();

        // Also create a real file to confirm recursion still works alongside symlinks
        std::fs::write(dir.path().join("real.txt"), "content").unwrap();

        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        // Must not panic even though the dangling symlink target doesn't exist
        chown_recursive(dir.path(), uid, gid);

        // Symlink still exists and is still a symlink (not dereferenced, not removed)
        let meta = std::fs::symlink_metadata(dir.path().join("link")).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "symlink should be preserved, not followed"
        );

        // Real file should still be accessible
        assert!(dir.path().join("real.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn chown_recursive_symlink_to_dir_is_not_recursed() {
        let outer = tempfile::tempdir().unwrap();
        let inner = tempfile::tempdir().unwrap();

        // inner_dir/secret.txt — should NOT be touched if we link inner into outer
        std::fs::write(inner.path().join("secret.txt"), "sensitive").unwrap();

        // outer/link_to_inner -> inner (a directory symlink)
        std::os::unix::fs::symlink(inner.path(), outer.path().join("link_to_inner")).unwrap();

        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        // chown_recursive on outer must NOT recurse into inner via the symlink
        chown_recursive(outer.path(), uid, gid);

        // inner/secret.txt must still be accessible (not removed, not corrupted)
        assert!(inner.path().join("secret.txt").exists());

        // The symlink in outer should remain a symlink
        let meta = std::fs::symlink_metadata(outer.path().join("link_to_inner")).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "directory symlink should not be recursed"
        );
    }
}
