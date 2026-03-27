//! Linux native sandbox using user namespaces
//!
//! Uses `unshare` with user, mount, PID, and network namespaces to create
//! a sandbox with filesystem isolation via `pivot_root`.
//!
//! The core of this module is `generate_setup_script()`, which produces a
//! shell script that runs inside the namespace to set up the isolated
//! filesystem and apply resource limits before exec'ing the user command.

use crate::error::{MinoError, MinoResult};
use crate::network::NetworkMode;
use crate::sandbox::native::SandboxSpawnConfig;
use crate::sandbox::process::SandboxProcess;
use crate::sandbox::resource_limits::ResourceLimits;
use tokio::process::Command;

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
pub async fn validate_linux_setup() -> MinoResult<()> {
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
pub async fn spawn_linux_sandbox(config: SandboxSpawnConfig) -> MinoResult<SandboxProcess> {
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

/// Single-quote a string for safe inclusion in shell scripts.
///
/// Wraps the value in single quotes after escaping embedded single quotes
/// using the POSIX `'\''` idiom (end quote, escaped quote, restart quote).
fn shell_quote(s: &str) -> String {
    format!("'{}'", crate::network::shell_escape(s))
}

/// Single-quote a filesystem path for safe shell interpolation.
fn shell_quote_path(p: &std::path::Path) -> String {
    shell_quote(&p.display().to_string())
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

    // 6. Bind-mount project dir read-write
    let project_dir = shell_quote_path(&config.project_dir);
    script.push_str(&format!("mount --bind {project_dir} {root}/workspace\n"));

    // 7. Bind-mount passthrough paths read-only
    for path_str in &config.sandbox_config.passthrough_paths {
        let mount_point = path_str.trim_start_matches('/');
        let quoted = shell_quote(path_str);
        let quoted_mount = shell_quote(mount_point);
        script.push_str(&format!(
            "[ -d {quoted} ] && mkdir -p {root}/{quoted_mount} && mount --bind {quoted} {root}/{quoted_mount} && mount -o remount,ro,bind {root}/{quoted_mount}\n"
        ));
    }

    // 8. Bind-mount writable paths
    for path_str in &config.sandbox_config.writable_paths {
        let mount_point = path_str.trim_start_matches('/');
        let quoted = shell_quote(path_str);
        let quoted_mount = shell_quote(mount_point);
        script.push_str(&format!(
            "[ -d {quoted} ] && mkdir -p {root}/{quoted_mount} && mount --bind {quoted} {root}/{quoted_mount}\n"
        ));
    }

    // 9. Copy dotfiles to sandbox HOME
    if let Some(dotfile_dir) = &config.dotfile_dir {
        let quoted = shell_quote_path(dotfile_dir);
        script.push_str(&format!(
            "cp -a {quoted}/* {root}/home/agent/ 2>/dev/null || true\n"
        ));
    }

    // 10. Mount tmpfs at /tmp and proc
    script.push_str(&format!("mount -t tmpfs tmpfs {root}/tmp\n"));
    script.push_str(&format!("mount -t proc proc {root}/proc\n"));

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

    // 14. exec the user command with proper quoting
    let escaped_cmd = config
        .command
        .iter()
        .map(|arg| format!("'{}'", crate::network::shell_escape(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    script.push_str(&format!("exec {escaped_cmd}\n"));

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
        assert!(script.contains("mount -t tmpfs tmpfs /tmp/mino-root-$$/tmp"));
        assert!(script.contains("mount -t proc proc /tmp/mino-root-$$/proc"));
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
        assert!(script.contains("exec 'echo' 'hello'"));
    }

    #[test]
    fn script_exec_escapes_single_quotes() {
        let mut config = test_spawn_config();
        config.command = vec!["echo".to_string(), "it's alive".to_string()];
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("exec 'echo' 'it'\\''s alive'"));
    }

    #[test]
    fn script_exec_single_command() {
        let mut config = test_spawn_config();
        config.command = vec!["/bin/bash".to_string()];
        let limits = no_limits();
        let script = generate_setup_script(&config, &limits).unwrap();
        assert!(script.contains("exec '/bin/bash'\n"));
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
        assert!(script.contains("mount --bind '/opt/my tools'"));
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

        assert!(script.contains("mount --bind '/opt/toolchain' /tmp/mino-root-$$/'opt/toolchain'"));
        assert!(script.contains("mount -o remount,ro,bind /tmp/mino-root-$$/'opt/toolchain'"));
        assert!(script.contains("mount --bind '/usr/local/go' /tmp/mino-root-$$/'usr/local/go'"));
        assert!(script.contains("mount -o remount,ro,bind /tmp/mino-root-$$/'usr/local/go'"));
    }

    #[test]
    fn script_writable_paths_mounted_readwrite() {
        let mut config = test_spawn_config();
        config.sandbox_config.writable_paths = vec!["/tmp/shared".to_string()];
        let script = generate_setup_script(&config, &no_limits()).unwrap();

        assert!(script.contains("mount --bind '/tmp/shared' /tmp/mino-root-$$/'tmp/shared'"));
        assert!(!script.contains("remount,ro,bind /tmp/mino-root-$$/'tmp/shared'"));
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
            "'/opt/tools'",
            "'/var/cache'",
            "cp -a '/tmp/dotfiles'/*",
            "mount -t tmpfs tmpfs /tmp/mino-root-$$/tmp",
            "mount -t proc proc",
            "pivot_root",
            "umount -l /old_root",
            "rmdir /old_root",
            "export HOME=/home/agent",
            "cd /workspace",
            "ulimit -v",
            "ulimit -u 128",
            "ulimit -t 1800",
            "ulimit -f",
            "exec '/bin/bash' '-c' 'ls -la'",
        ];

        let mut last_pos = 0;
        for section in &sections {
            let pos = script[last_pos..]
                .find(section)
                .unwrap_or_else(|| panic!("Missing section: {}", section));
            last_pos += pos;
        }
    }
}
