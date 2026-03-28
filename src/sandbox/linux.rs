//! Linux native sandbox using user namespaces
//!
//! Uses `unshare` with user, mount, PID, and network namespaces to create
//! a sandbox with filesystem isolation via `pivot_root`.
//!
//! The core of this module is `generate_setup_script()`, which produces a
//! shell script that runs inside the namespace to set up the isolated
//! filesystem and apply resource limits before exec'ing the user command.

use async_trait::async_trait;
use std::path::Path;

use crate::error::{MinoError, MinoResult};
use crate::network::NetworkMode;
use crate::sandbox::native::{SandboxPlatform, SandboxSpawnConfig};
use crate::sandbox::process::SandboxProcess;
use crate::sandbox::resource_limits::ResourceLimits;
use tokio::process::Command;

/// Linux sandbox implementation using user namespaces.
pub struct LinuxSandbox;

#[async_trait]
impl SandboxPlatform for LinuxSandbox {
    async fn validate_setup(&self) -> MinoResult<()> {
        validate_linux_setup().await
    }

    async fn spawn(&self, config: SandboxSpawnConfig) -> MinoResult<SandboxProcess> {
        spawn_linux_sandbox(config).await
    }

    async fn exec(
        &self,
        pid: u32,
        _session_name: &str,
        _sandbox_user: &str,
        command: &[String],
    ) -> MinoResult<i32> {
        exec_linux(pid, command).await
    }

    async fn cleanup(
        &self,
        _session_id: &str,
        _project_dir: &Path,
        _sandbox_user: &str,
    ) -> MinoResult<()> {
        // Linux namespaces auto-clean on process exit — nothing to do
        Ok(())
    }
}

/// Execute a command inside a Linux sandbox using nsenter.
///
/// Enters the user, mount, PID, and network namespaces of the target process.
/// Verifies that the target PID is owned by the current user before entering
/// its namespaces, preventing namespace entry into other users' processes if
/// the session file is tampered with.
async fn exec_linux(pid: u32, command: &[String]) -> MinoResult<i32> {
    verify_pid_ownership(pid).await?;

    let pid_str = pid.to_string();
    let status = Command::new("nsenter")
        .args([
            "--target", &pid_str, "--user", "--mount", "--pid", "--net", "--",
        ])
        .args(command)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| MinoError::command_failed("nsenter", e))?;

    Ok(status.code().unwrap_or(128))
}

/// Verify that a PID belongs to the current user by reading /proc/{pid}/status.
///
/// Checks the Uid line in the process status file and compares the real UID
/// against the current user's UID. This prevents entering namespaces of
/// processes owned by other users.
async fn verify_pid_ownership(pid: u32) -> MinoResult<()> {
    let status_path = format!("/proc/{}/status", pid);
    let content = tokio::fs::read_to_string(&status_path)
        .await
        .map_err(|e| MinoError::io(format!("reading /proc/{}/status", pid), e))?;

    // SAFETY: getuid() is always safe — it has no preconditions and
    // simply returns the real UID of the calling process.
    let expected_uid = unsafe { libc::getuid() };

    for line in content.lines() {
        if let Some(uid_str) = line.strip_prefix("Uid:") {
            // Format: "Uid:\treal\teffective\tsaved\tfs"
            let real_uid: u32 = uid_str
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| {
                    MinoError::Internal(format!("Failed to parse UID from /proc/{}/status", pid))
                })?;

            if real_uid != expected_uid {
                return Err(MinoError::User(format!(
                    "PID {} is owned by UID {} but current user is UID {}. \
                     Refusing to enter namespaces of another user's process.",
                    pid, real_uid, expected_uid
                )));
            }
            return Ok(());
        }
    }

    Err(MinoError::Internal(format!(
        "No Uid line found in /proc/{}/status",
        pid
    )))
}

/// System paths to bind-mount read-only into the sandbox
const SYSTEM_BIND_MOUNTS: &[&str] = &["/usr", "/lib", "/bin", "/sbin"];

/// Optional system paths that may not exist on all distros
const OPTIONAL_BIND_MOUNTS: &[&str] = &["/lib64"];

/// /etc entries to bind-mount read-only (DNS, TLS, hostname, timezone)
const ETC_ENTRIES: &[&str] = &["resolv.conf", "ssl", "hosts", "localtime"];

/// Device nodes to mount from the host
const DEVICE_NODES: &[&str] = &["null", "zero", "urandom", "tty"];

/// Required directories in the new root
const ROOT_DIRS: &[&str] = &[
    "workspace",
    "usr",
    "lib",
    "lib64",
    "bin",
    "sbin",
    "etc",
    "dev",
    "dev/shm",
    "proc",
    "tmp",
    "home/agent",
];

/// Validate Linux prerequisites for native sandbox
pub(crate) async fn validate_linux_setup() -> MinoResult<()> {
    check_user_namespaces().await?;
    check_unshare_available().await?;
    Ok(())
}

/// Check that unprivileged user namespaces are enabled
async fn check_user_namespaces() -> MinoResult<()> {
    // /proc/sys/kernel/unprivileged_userns_clone exists on some distros (Debian/Ubuntu).
    // If it exists and is "0", user namespaces are disabled.
    // If it doesn't exist, namespaces are enabled by default (Fedora, Arch, etc.).
    let path = std::path::Path::new("/proc/sys/kernel/unprivileged_userns_clone");
    if path.exists() {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| MinoError::io("reading unprivileged_userns_clone", e))?;

        if content.trim() == "0" {
            return Err(MinoError::NamespaceSetup(
                "Unprivileged user namespaces are disabled. \
                 Enable with: sudo sysctl -w kernel.unprivileged_userns_clone=1"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Check that the `unshare` binary is available
async fn check_unshare_available() -> MinoResult<()> {
    let output = Command::new("which")
        .arg("unshare")
        .output()
        .await
        .map_err(|e| MinoError::io("checking for unshare", e))?;

    if !output.status.success() {
        return Err(MinoError::CliNotFound {
            name: "unshare".to_string(),
            hint: "Install util-linux: sudo apt install util-linux (Debian/Ubuntu) \
                   or sudo dnf install util-linux (Fedora)"
                .to_string(),
        });
    }
    Ok(())
}

/// Spawn a Linux sandbox using user namespaces + pivot_root
pub(crate) async fn spawn_linux_sandbox(config: SandboxSpawnConfig) -> MinoResult<SandboxProcess> {
    let resource_limits = ResourceLimits::from_config(&config.sandbox_config);
    let setup_script = generate_setup_script(&config, &resource_limits)?;

    let mut cmd = Command::new("unshare");
    cmd.args(["--user", "--map-root-user", "--mount", "--pid", "--fork"]);

    // Add network namespace isolation when NetworkMode::None
    if matches!(config.network_mode, NetworkMode::None) {
        cmd.arg("--net");
    }

    cmd.arg("--");
    cmd.args(["/bin/sh", "-c", &setup_script]);

    // Clear environment, set only allowed vars
    cmd.env_clear();
    cmd.envs(&config.env);

    if config.interactive {
        cmd.stdin(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());
    }

    let child = cmd
        .spawn()
        .map_err(|e| MinoError::NamespaceSetup(format!("Failed to spawn unshare: {}", e)))?;

    Ok(SandboxProcess::new(child, config.session_id))
}

/// Shell-escape a path for use in the setup script.
///
/// Rejects paths containing null bytes or newlines, which could enable
/// injection attacks in the generated shell script. Returns the path
/// wrapped in POSIX single quotes.
fn escape_path(path: &std::path::Path) -> MinoResult<String> {
    let s = path.to_str().ok_or_else(|| MinoError::PathInvalid {
        path: path.to_path_buf(),
        reason: "non-UTF8 path".to_string(),
    })?;
    if s.contains('\0') || s.contains('\n') {
        return Err(MinoError::PathInvalid {
            path: path.to_path_buf(),
            reason: "contains null byte or newline".to_string(),
        });
    }
    Ok(shell_quote(s))
}

/// Shell-escape a string path for use in the setup script.
///
/// Like `escape_path` but accepts a `&str` directly (for passthrough/writable paths).
fn escape_path_str(s: &str) -> MinoResult<String> {
    escape_path(std::path::Path::new(s))
}

/// Append a bind-mount command for a user path to the setup script.
///
/// Resolves symlinks at mount time via `readlink -f` to prevent mounting
/// unexpected content. `|| :` prevents `set -e` from aborting if the path
/// doesn't exist — readlink returns empty, `[ -d "" ]` is false, and the
/// `&&` chain skips the mount.
fn append_bind_mount(
    script: &mut String,
    root: &str,
    path_str: &str,
    readonly: bool,
) -> MinoResult<()> {
    let mount_point = path_str.trim_start_matches('/');
    let quoted = escape_path_str(path_str)?;
    let quoted_mount = escape_path_str(mount_point)?;
    let ro_flag = if readonly { "ro," } else { "" };
    use std::fmt::Write;
    writeln!(
        script,
        "REAL=$(readlink -f {quoted} 2>/dev/null || :) && [ -d \"$REAL\" ] && \
         mkdir -p {root}/{quoted_mount} && mount --bind \"$REAL\" {root}/{quoted_mount} && \
         mount -o remount,{ro_flag}nosuid,nodev,bind {root}/{quoted_mount}"
    )
    .unwrap();
    Ok(())
}

/// Single-quote a string for safe inclusion in shell scripts.
///
/// Wraps the value in single quotes after escaping embedded single quotes
/// using the POSIX `'\''` idiom (end quote, escaped quote, restart quote).
fn shell_quote(s: &str) -> String {
    format!("'{}'", crate::network::shell_escape(s))
}

/// Generate the setup script that runs inside the namespace.
///
/// This script:
/// 1. Creates a tmpfs root
/// 2. Bind-mounts system paths read-only
/// 3. Bind-mounts project dir read-write
/// 4. Mounts /proc, /dev nodes, /tmp
/// 5. Copies dotfiles into sandbox HOME
/// 6. Applies resource limits via ulimit
/// 7. Runs pivot_root to isolate filesystem
/// 8. exec's the user command
///
/// This function is deliberately `pub(crate)` to allow thorough unit testing
/// of the generated script without needing to run `unshare`.
pub(crate) fn generate_setup_script(
    config: &SandboxSpawnConfig,
    resource_limits: &ResourceLimits,
) -> MinoResult<String> {
    let mut script = String::with_capacity(4096);
    script.push_str("set -e\n");

    let root = "/tmp/mino-root-$$";

    // 1. Create new root on tmpfs
    script.push_str(&format!("mkdir -p {root}\n"));
    script.push_str(&format!("mount -t tmpfs tmpfs {root}\n"));

    // 2. Create directory structure
    for dir in ROOT_DIRS {
        script.push_str(&format!("mkdir -p {root}/{dir}\n"));
    }

    // 3. Bind-mount system paths read-only
    for sys_path in SYSTEM_BIND_MOUNTS {
        script.push_str(&format!(
            "[ -d {path} ] && mount --bind {path} {root}{path} && mount -o remount,ro,bind {root}{path}\n",
            path = sys_path
        ));
    }

    // lib64 separately since it's optional on some distros
    for opt_path in OPTIONAL_BIND_MOUNTS {
        script.push_str(&format!(
            "[ -d {path} ] && mkdir -p {root}{path} && mount --bind {path} {root}{path} && mount -o remount,ro,bind {root}{path}\n",
            path = opt_path
        ));
    }

    // 4. Bind-mount specific /etc files read-only
    for etc_entry in ETC_ENTRIES {
        script.push_str(&format!(
            "[ -e /etc/{entry} ] && {{ [ -d /etc/{entry} ] && mkdir -p {root}/etc/{entry} || touch {root}/etc/{entry}; }} && mount --bind /etc/{entry} {root}/etc/{entry} && mount -o remount,ro,bind {root}/etc/{entry}\n",
            entry = etc_entry
        ));
    }

    // 5. Mount device nodes
    for dev in DEVICE_NODES {
        script.push_str(&format!("touch {root}/dev/{dev}\n"));
        script.push_str(&format!("mount --bind /dev/{dev} {root}/dev/{dev}\n"));
    }
    // /dev/shm as tmpfs
    script.push_str(&format!("mount -t tmpfs tmpfs {root}/dev/shm\n"));

    // 6. Bind-mount project dir read-write (with nosuid,nodev)
    let project_dir = escape_path(&config.project_dir)?;
    script.push_str(&format!(
        "mount --bind {project_dir} {root}/workspace && mount -o remount,nosuid,nodev,bind {root}/workspace\n"
    ));

    // 7. Bind-mount passthrough paths read-only (with nosuid,nodev)
    for path_str in &config.sandbox_config.passthrough_paths {
        append_bind_mount(&mut script, root, path_str, true)?;
    }

    // 8. Bind-mount writable paths (with nosuid,nodev)
    for path_str in &config.sandbox_config.writable_paths {
        append_bind_mount(&mut script, root, path_str, false)?;
    }

    // 9. Copy dotfiles to sandbox HOME
    if let Some(dotfile_dir) = &config.dotfile_dir {
        let quoted = escape_path(dotfile_dir)?;
        script.push_str(&format!(
            "cp -a {quoted}/* {root}/home/agent/ 2>/dev/null || true\n"
        ));
    }

    // 10. Mount tmpfs at /tmp and proc (hidepid=2 hides other users' processes)
    script.push_str(&format!(
        "mount -t tmpfs -o nosuid,nodev tmpfs {root}/tmp\n"
    ));
    script.push_str(&format!("mount -t proc -o hidepid=2 proc {root}/proc\n"));

    // 11. pivot_root — this is the critical security step
    script.push_str(&format!("mkdir -p {root}/old_root\n"));
    script.push_str(&format!("pivot_root {root} {root}/old_root\n"));
    script.push_str("umount -l /old_root\n");
    script.push_str("rmdir /old_root\n");

    // 12. Set HOME and cd to workspace
    script.push_str("export HOME=/home/agent\n");
    script.push_str("cd /workspace\n");

    // 13. Apply resource limits via ulimit
    if resource_limits.max_memory_bytes > 0 {
        // ulimit -v takes KB
        let kb = resource_limits.max_memory_bytes / 1024;
        script.push_str(&format!("ulimit -v {kb}\n"));
    }
    if resource_limits.max_processes > 0 {
        script.push_str(&format!("ulimit -u {}\n", resource_limits.max_processes));
    }
    if resource_limits.max_cpu_seconds > 0 {
        script.push_str(&format!("ulimit -t {}\n", resource_limits.max_cpu_seconds));
    }
    if resource_limits.max_file_size_bytes > 0 {
        // ulimit -f takes 512-byte blocks
        let blocks = resource_limits.max_file_size_bytes / 512;
        script.push_str(&format!("ulimit -f {blocks}\n"));
    }

    // 14. Prevent privilege escalation (defense-in-depth)
    //     setpriv --no-new-privs sets PR_SET_NO_NEW_PRIVS so that the sandboxed
    //     process cannot gain privileges via setuid binaries or capabilities.
    let escaped_cmd = config
        .command
        .iter()
        .map(|arg| format!("'{}'", crate::network::shell_escape(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    script.push_str("if command -v setpriv >/dev/null 2>&1; then\n");
    script.push_str(&format!("  exec setpriv --no-new-privs {escaped_cmd}\n"));
    script.push_str("else\n");
    script.push_str(&format!("  exec {escaped_cmd}\n"));
    script.push_str("fi\n");

    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::NetworkMode;
    use crate::sandbox::config::SandboxConfig;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn test_spawn_config() -> SandboxSpawnConfig {
        SandboxSpawnConfig {
            session_id: "test-session".to_string(),
            project_dir: PathBuf::from("/home/user/project"),
            command: vec!["echo".to_string(), "hello".to_string()],
            env: HashMap::new(),
            network_mode: NetworkMode::Bridge,
            sandbox_config: SandboxConfig::default(),
            dotfile_dir: None,
            interactive: true,
        }
    }

    fn default_limits() -> ResourceLimits {
        ResourceLimits::from_config(&SandboxConfig::default())
    }

    fn no_limits() -> ResourceLimits {
        ResourceLimits {
            max_memory_bytes: 0,
            max_processes: 0,
            max_cpu_seconds: 0,
            max_file_size_bytes: 0,
        }
    }

    /// Generate script with default config and no resource limits
    fn script_default() -> String {
        generate_setup_script(&test_spawn_config(), &no_limits()).unwrap()
    }

    // ---- Script structure tests ----

    #[test]
    fn script_creates_tmpfs_root_and_required_dirs() {
        let script = generate_setup_script(&test_spawn_config(), &default_limits()).unwrap();
        assert!(script.starts_with("set -e\n"));
        assert!(script.contains("mount -t tmpfs tmpfs /tmp/mino-root-$$"));
        for dir in ROOT_DIRS {
            assert!(
                script.contains(&format!("mkdir -p /tmp/mino-root-$$/{}", dir)),
                "Missing mkdir for {}",
                dir
            );
        }
    }

    // ---- System path bind-mount tests ----

    #[test]
    fn script_bind_mounts_system_paths_readonly() {
        let script = script_default();
        for path in SYSTEM_BIND_MOUNTS {
            assert!(
                script.contains(&format!("mount --bind {path} /tmp/mino-root-$${path}")),
                "Missing bind-mount for {}",
                path
            );
            assert!(
                script.contains(&format!("mount -o remount,ro,bind /tmp/mino-root-$${path}")),
                "Missing ro remount for {}",
                path
            );
        }
    }

    #[test]
    fn script_bind_mounts_lib64_optionally() {
        let script = script_default();
        assert!(script.contains("[ -d /lib64 ] && mkdir -p /tmp/mino-root-$$/lib64"));
        assert!(script.contains("mount --bind /lib64 /tmp/mino-root-$$/lib64"));
        assert!(script.contains("mount -o remount,ro,bind /tmp/mino-root-$$/lib64"));
    }

    #[test]
    fn script_mounts_etc_entries_readonly() {
        let script = script_default();
        for entry in ETC_ENTRIES {
            assert!(
                script.contains(&format!("mount --bind /etc/{entry}")),
                "Missing /etc/{} bind-mount",
                entry
            );
            assert!(
                script.contains(&format!(
                    "mount -o remount,ro,bind /tmp/mino-root-$$/etc/{entry}"
                )),
                "Missing /etc/{} ro remount",
                entry
            );
        }
    }

    #[test]
    fn script_mounts_device_nodes_and_shm() {
        let script = script_default();
        for dev in DEVICE_NODES {
            assert!(
                script.contains(&format!("touch /tmp/mino-root-$$/dev/{dev}")),
                "Missing touch for /dev/{}",
                dev
            );
            assert!(
                script.contains(&format!(
                    "mount --bind /dev/{dev} /tmp/mino-root-$$/dev/{dev}"
                )),
                "Missing bind-mount for /dev/{}",
                dev
            );
        }
        assert!(script.contains("mount -t tmpfs tmpfs /tmp/mino-root-$$/dev/shm"));
    }

    #[test]
    fn script_mounts_project_dir_and_filesystems() {
        let script = script_default();
        assert!(script.contains("mount --bind '/home/user/project' /tmp/mino-root-$$/workspace"));
        assert!(script.contains("remount,nosuid,nodev,bind /tmp/mino-root-$$/workspace"));
        assert!(script.contains("mount -t tmpfs -o nosuid,nodev tmpfs /tmp/mino-root-$$/tmp"));
        assert!(script.contains("mount -t proc -o hidepid=2 proc /tmp/mino-root-$$/proc"));
    }

    // ---- pivot_root, cleanup, and environment ----

    #[test]
    fn script_pivot_root_then_cleanup_then_env() {
        let script = script_default();
        let pivot_pos = script.find("pivot_root").expect("missing pivot_root");
        let umount_pos = script.find("umount -l /old_root").expect("missing umount");
        let rmdir_pos = script.find("rmdir /old_root").expect("missing rmdir");
        let home_pos = script
            .find("export HOME=/home/agent")
            .expect("missing HOME export");
        let cd_pos = script.find("cd /workspace").expect("missing cd");
        let exec_pos = script.find("exec ").expect("missing exec");

        assert!(pivot_pos < umount_pos);
        assert!(umount_pos < rmdir_pos);
        assert!(rmdir_pos < home_pos);
        assert!(home_pos < cd_pos);
        assert!(cd_pos < exec_pos);
    }

    // ---- Resource limit tests ----

    #[test]
    fn script_applies_each_limit_individually() {
        let config = test_spawn_config();
        let cases: &[(ResourceLimits, &str)] = &[
            (
                ResourceLimits {
                    max_memory_bytes: 4096 * 1024 * 1024,
                    ..no_limits()
                },
                &format!("ulimit -v {}", 4096 * 1024 * 1024u64 / 1024),
            ),
            (
                ResourceLimits {
                    max_processes: 256,
                    ..no_limits()
                },
                "ulimit -u 256",
            ),
            (
                ResourceLimits {
                    max_cpu_seconds: 3600,
                    ..no_limits()
                },
                "ulimit -t 3600",
            ),
            (
                ResourceLimits {
                    max_file_size_bytes: 100 * 1024 * 1024,
                    ..no_limits()
                },
                &format!("ulimit -f {}", 100 * 1024 * 1024u64 / 512),
            ),
        ];
        for (limits, expected) in cases {
            let script = generate_setup_script(&config, limits).unwrap();
            assert!(
                script.contains(expected),
                "Expected '{}' in script for limits {:?}",
                expected,
                limits
            );
        }
    }

    #[test]
    fn script_skips_all_limits_when_zero() {
        let script = script_default();
        assert!(!script.contains("ulimit"));
    }

    #[test]
    fn script_applies_default_limits() {
        let script = generate_setup_script(&test_spawn_config(), &default_limits()).unwrap();
        assert!(script.contains("ulimit -v "));
        assert!(script.contains("ulimit -u 256"));
        assert!(!script.contains("ulimit -t "));
        assert!(!script.contains("ulimit -f "));
    }

    // ---- Command exec tests ----

    #[test]
    fn script_exec_with_properly_escaped_command() {
        let script = script_default();
        // When setpriv is available, wraps with --no-new-privs
        assert!(script.contains("exec setpriv --no-new-privs 'echo' 'hello'"));
        // Falls back to plain exec without setpriv
        assert!(script.contains("exec 'echo' 'hello'\n"));
    }

    #[test]
    fn script_exec_escapes_single_quotes() {
        let mut config = test_spawn_config();
        config.command = vec!["echo".to_string(), "it's alive".to_string()];
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("exec setpriv --no-new-privs 'echo' 'it'\\''s alive'"));
    }

    #[test]
    fn script_exec_single_command() {
        let mut config = test_spawn_config();
        config.command = vec!["/bin/bash".to_string()];
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("exec setpriv --no-new-privs '/bin/bash'\n"));
    }

    #[test]
    fn script_setpriv_conditional() {
        let script = script_default();
        // Verify the setpriv conditional structure
        assert!(script.contains("if command -v setpriv >/dev/null 2>&1; then"));
        assert!(script.contains("else\n"));
        assert!(script.contains("fi\n"));
    }

    // ---- Shell injection safety tests ----

    #[test]
    fn script_quotes_project_dir_with_spaces() {
        let mut config = test_spawn_config();
        config.project_dir = PathBuf::from("/home/user/my project");
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("mount --bind '/home/user/my project' /tmp/mino-root-$$/workspace"));
    }

    #[test]
    fn script_quotes_project_dir_with_single_quotes() {
        let mut config = test_spawn_config();
        config.project_dir = PathBuf::from("/home/user/it's");
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("mount --bind '/home/user/it'\\''s' /tmp/mino-root-$$/workspace"));
    }

    #[test]
    fn script_quotes_passthrough_path_with_spaces() {
        let mut config = test_spawn_config();
        config.sandbox_config.passthrough_paths = vec!["/opt/my tools".to_string()];
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("readlink -f '/opt/my tools'"));
    }

    #[test]
    fn script_quotes_dotfile_dir_with_special_chars() {
        let mut config = test_spawn_config();
        config.dotfile_dir = Some(PathBuf::from("/tmp/mino dots $(id)"));
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("cp -a '/tmp/mino dots $(id)'/*"));
    }

    // ---- Optional config paths ----

    #[test]
    fn script_passthrough_paths_mounted_readonly() {
        let mut config = test_spawn_config();
        config.sandbox_config.passthrough_paths =
            vec!["/opt/toolchain".to_string(), "/usr/local/go".to_string()];
        let script = generate_setup_script(&config, &no_limits()).unwrap();

        assert!(script.contains("readlink -f '/opt/toolchain'"));
        assert!(script.contains("mount --bind \"$REAL\" /tmp/mino-root-$$/'opt/toolchain'"));
        assert!(script.contains("remount,ro,nosuid,nodev,bind /tmp/mino-root-$$/'opt/toolchain'"));
        assert!(script.contains("readlink -f '/usr/local/go'"));
        assert!(script.contains("mount --bind \"$REAL\" /tmp/mino-root-$$/'usr/local/go'"));
        assert!(script.contains("remount,ro,nosuid,nodev,bind /tmp/mino-root-$$/'usr/local/go'"));
    }

    #[test]
    fn script_writable_paths_mounted_readwrite() {
        let mut config = test_spawn_config();
        config.sandbox_config.writable_paths = vec!["/tmp/shared".to_string()];
        let script = generate_setup_script(&config, &no_limits()).unwrap();

        assert!(script.contains("readlink -f '/tmp/shared'"));
        assert!(script.contains("mount --bind \"$REAL\" /tmp/mino-root-$$/'tmp/shared'"));
        // Writable paths get nosuid,nodev but NOT read-only
        assert!(script.contains("remount,nosuid,nodev,bind /tmp/mino-root-$$/'tmp/shared'"));
        assert!(!script.contains("remount,ro,nosuid,nodev,bind /tmp/mino-root-$$/'tmp/shared'"));
    }

    #[test]
    fn script_dotfile_copy_present_when_configured() {
        let mut config = test_spawn_config();
        config.dotfile_dir = Some(PathBuf::from("/tmp/mino-dotfiles-12345"));
        let script = generate_setup_script(&config, &no_limits()).unwrap();
        assert!(script.contains("cp -a '/tmp/mino-dotfiles-12345'/* /tmp/mino-root-$$/home/agent/"));
    }

    #[test]
    fn script_dotfile_copy_absent_when_none() {
        let script = script_default();
        assert!(!script.contains("cp -a"));
    }

    // ---- Full script structure snapshot ----

    #[test]
    fn script_full_structure() {
        let mut config = test_spawn_config();
        config.dotfile_dir = Some(PathBuf::from("/tmp/dotfiles"));
        config.sandbox_config.passthrough_paths = vec!["/opt/tools".to_string()];
        config.sandbox_config.writable_paths = vec!["/var/cache".to_string()];
        config.command = vec![
            "/bin/bash".to_string(),
            "-c".to_string(),
            "ls -la".to_string(),
        ];

        let limits = ResourceLimits {
            max_memory_bytes: 2048 * 1024 * 1024,
            max_processes: 128,
            max_cpu_seconds: 1800,
            max_file_size_bytes: 50 * 1024 * 1024,
        };

        let script = generate_setup_script(&config, &limits).unwrap();

        // Verify all major sections are present and in order
        let sections = [
            "set -e",
            "mount -t tmpfs",
            "mkdir -p",
            "mount --bind /usr",
            "mount --bind /lib",
            "/etc/resolv.conf",
            "/dev/null",
            "/dev/shm",
            "mount --bind '/home/user/project'",
            "nosuid,nodev,bind /tmp/mino-root-$$/workspace",
            "readlink -f '/opt/tools'",
            "ro,nosuid,nodev,bind",
            "readlink -f '/var/cache'",
            "nosuid,nodev,bind",
            "cp -a '/tmp/dotfiles'/*",
            "mount -t tmpfs -o nosuid,nodev tmpfs /tmp/mino-root-$$/tmp",
            "mount -t proc -o hidepid=2 proc",
            "pivot_root",
            "umount -l /old_root",
            "rmdir /old_root",
            "export HOME=/home/agent",
            "cd /workspace",
            "ulimit -v",
            "ulimit -u 128",
            "ulimit -t 1800",
            "ulimit -f",
            "setpriv --no-new-privs '/bin/bash' '-c' 'ls -la'",
        ];

        let mut last_pos = 0;
        for section in &sections {
            let pos = script[last_pos..]
                .find(section)
                .unwrap_or_else(|| panic!("Missing section: {}", section));
            last_pos += pos;
        }
    }

    // ---- escape_path validation tests ----

    #[test]
    fn escape_path_normal_path() {
        let result = escape_path(std::path::Path::new("/home/user/project"));
        assert_eq!(result.unwrap(), "'/home/user/project'");
    }

    #[test]
    fn escape_path_with_spaces() {
        let result = escape_path(std::path::Path::new("/home/user/my project"));
        assert_eq!(result.unwrap(), "'/home/user/my project'");
    }

    #[test]
    fn escape_path_with_single_quotes() {
        let result = escape_path(std::path::Path::new("/home/user/it's"));
        assert_eq!(result.unwrap(), "'/home/user/it'\\''s'");
    }

    #[test]
    fn escape_path_with_dollar_sign() {
        let result = escape_path(std::path::Path::new("/tmp/$(whoami)"));
        assert_eq!(result.unwrap(), "'/tmp/$(whoami)'");
    }

    #[test]
    fn escape_path_with_unicode() {
        let result = escape_path(std::path::Path::new("/home/user/cafe\u{0301}"));
        assert!(result.is_ok());
        assert!(result.unwrap().contains("cafe\u{0301}"));
    }

    #[test]
    fn escape_path_rejects_null_bytes() {
        let path = std::path::Path::new("/tmp/bad\0path");
        let result = escape_path(path);
        // On most systems, Path::to_str() will fail on null bytes, but our
        // explicit check handles the case too. Either way, it must be Err.
        assert!(result.is_err());
    }

    #[test]
    fn escape_path_rejects_newlines() {
        let result = escape_path(std::path::Path::new("/tmp/bad\npath"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("null byte or newline"));
    }

    #[test]
    fn escape_path_str_rejects_newlines() {
        let result = escape_path_str("/opt/bad\npath");
        assert!(result.is_err());
    }

    #[test]
    fn escape_path_str_normal() {
        let result = escape_path_str("/opt/tools");
        assert_eq!(result.unwrap(), "'/opt/tools'");
    }

    // ---- Script rejects paths with injection characters ----

    #[test]
    fn script_rejects_project_dir_with_newline() {
        let mut config = test_spawn_config();
        config.project_dir = PathBuf::from("/home/user/bad\npath");
        let result = generate_setup_script(&config, &no_limits());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("null byte or newline"));
    }

    #[test]
    fn script_rejects_passthrough_path_with_newline() {
        let mut config = test_spawn_config();
        config.sandbox_config.passthrough_paths = vec!["/opt/bad\npath".to_string()];
        let result = generate_setup_script(&config, &no_limits());
        assert!(result.is_err());
    }

    #[test]
    fn script_rejects_writable_path_with_newline() {
        let mut config = test_spawn_config();
        config.sandbox_config.writable_paths = vec!["/tmp/bad\npath".to_string()];
        let result = generate_setup_script(&config, &no_limits());
        assert!(result.is_err());
    }

    #[test]
    fn script_rejects_dotfile_dir_with_newline() {
        let mut config = test_spawn_config();
        config.dotfile_dir = Some(PathBuf::from("/tmp/dots\nbad"));
        let result = generate_setup_script(&config, &no_limits());
        assert!(result.is_err());
    }

    // ---- Hardening feature tests ----

    #[test]
    fn script_mounts_proc_with_hidepid() {
        let script = script_default();
        assert!(script.contains("mount -t proc -o hidepid=2 proc"));
    }

    #[test]
    fn script_mounts_tmp_with_nosuid_nodev() {
        let script = script_default();
        assert!(script.contains("mount -t tmpfs -o nosuid,nodev tmpfs"));
    }

    #[test]
    fn script_project_dir_has_nosuid_nodev() {
        let script = script_default();
        assert!(script.contains("remount,nosuid,nodev,bind /tmp/mino-root-$$/workspace"));
    }

    // ---- append_bind_mount tests ----

    #[test]
    fn append_bind_mount_readonly() {
        let mut script = String::new();
        append_bind_mount(&mut script, "/tmp/root", "/opt/tools", true).unwrap();
        assert!(script.contains("readlink -f '/opt/tools'"));
        assert!(script.contains("remount,ro,nosuid,nodev,bind"));
    }

    #[test]
    fn append_bind_mount_readwrite() {
        let mut script = String::new();
        append_bind_mount(&mut script, "/tmp/root", "/var/cache", false).unwrap();
        assert!(script.contains("readlink -f '/var/cache'"));
        assert!(script.contains("remount,nosuid,nodev,bind"));
        assert!(!script.contains("remount,ro,"));
    }

    #[test]
    fn append_bind_mount_escapes_path() {
        let mut script = String::new();
        append_bind_mount(&mut script, "/tmp/root", "/opt/my tools", true).unwrap();
        assert!(script.contains("readlink -f '/opt/my tools'"));
    }
}
