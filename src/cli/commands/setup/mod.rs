//! Setup command - interactive prerequisite installation
//!
//! Decomposed into domain-specific submodules:
//! - `container_macos` — OrbStack + Podman-in-VM checks
//! - `container_linux` — native Podman + rootless mode checks
//! - `native_macos` — macOS sandbox user, helper, sudoers, pf
//! - `native_linux` — Linux user namespace + unshare checks

mod container_linux;
mod container_macos;
mod native_linux;
mod native_macos;

mod helpers;

use crate::cli::args::SetupArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::Platform;
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

/// Setup step result for tracking what was done
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepResult {
    /// Already installed/configured
    AlreadyOk,
    /// Successfully installed/configured
    Installed,
    /// Skipped by user
    Skipped,
    /// Failed
    Failed,
    /// Depends on something else that failed/skipped
    Blocked,
}

impl StepResult {
    /// Whether the step completed successfully (already ok or just installed).
    pub(super) fn is_ok(self) -> bool {
        matches!(self, Self::AlreadyOk | Self::Installed)
    }

    /// Whether this step represents a user-actionable issue.
    ///
    /// Returns true for `Failed` and `Skipped` but NOT for `Blocked`,
    /// since blocked steps are cascading consequences of an upstream failure
    /// and should not be counted separately in the issue total.
    pub(super) fn is_issue(self) -> bool {
        matches!(self, Self::Failed | Self::Skipped)
    }
}

/// Execute the setup command
pub async fn execute(args: SetupArgs, config: &Config) -> MinoResult<()> {
    let ctx = UiContext::detect().with_auto_yes(args.yes);

    // Handle --uninstall: remove all native sandbox artifacts
    if args.uninstall {
        ui::intro(&ctx, "Native Sandbox Uninstall");
        return match Platform::detect() {
            Platform::MacOS => native_macos::uninstall_native_macos(&ctx).await,
            Platform::Linux => {
                ui::remark(
                    &ctx,
                    "Native sandbox on Linux uses user namespaces (no persistent artifacts). Nothing to uninstall.",
                );
                ui::outro_success(&ctx, "Nothing to clean up.");
                Ok(())
            }
            Platform::Unsupported => Err(MinoError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            )),
        };
    }

    // Native sandbox setup is a separate flow
    if args.native {
        if args.check {
            ui::intro(&ctx, "Native Sandbox Check");
        } else {
            ui::intro(&ctx, "Native Sandbox Setup");
        }

        return match Platform::detect() {
            Platform::MacOS => native_macos::setup_native_macos(&ctx, &args).await,
            Platform::Linux => native_linux::setup_native_linux(&ctx, &args).await,
            Platform::Unsupported => Err(MinoError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            )),
        };
    }

    if args.check {
        ui::intro(&ctx, "Mino Setup (check only)");
    } else {
        ui::intro(&ctx, "Mino Setup");
    }

    match Platform::detect() {
        Platform::MacOS => container_macos::setup_macos(&ctx, &args, config).await,
        Platform::Linux => container_linux::setup_linux(&ctx, &args).await,
        Platform::Unsupported => Err(MinoError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        )),
    }
}

// =============================================================================
// Shared helpers
// =============================================================================

/// Check if an OrbStack VM exists by name
pub(super) async fn vm_exists(vm_name: &str) -> bool {
    let output = Command::new("orb")
        .args(["list", "-q"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.lines().any(|line| line.trim() == vm_name)
        }
        _ => false,
    }
}

/// Check if user namespaces are enabled (shared between container_linux and native_linux)
pub(super) async fn check_user_namespaces(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let output = Command::new("cat")
        .arg("/proc/sys/user/max_user_namespaces")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let value = String::from_utf8_lossy(&out.stdout);
            let max_ns: u32 = value.trim().parse().unwrap_or(0);
            if max_ns > 0 {
                ui::step_ok_detail(ctx, "User namespaces enabled", &format!("max: {}", max_ns));
                StepResult::AlreadyOk
            } else {
                if args.check {
                    ui::step_error(ctx, "User namespaces disabled (max_user_namespaces = 0)");
                } else {
                    ui::step_warn(ctx, "User namespaces disabled (max_user_namespaces = 0)");
                }
                ui::remark(
                    ctx,
                    "User namespaces must be enabled for rootless containers.",
                );
                ui::remark(ctx, "Run: sudo sysctl -w user.max_user_namespaces=15000");
                ui::remark(ctx, "To make permanent, add to /etc/sysctl.conf:");
                ui::remark(ctx, "  user.max_user_namespaces=15000");

                StepResult::Failed
            }
        }
        _ => {
            // If we can't read the file, assume it's fine (some distros don't have this)
            ui::step_ok_detail(ctx, "User namespaces", "could not check (assuming enabled)");
            StepResult::AlreadyOk
        }
    }
}

/// Find an available system UID in the 400-499 range (macOS)
pub(super) async fn find_available_system_uid() -> Option<u32> {
    for uid in (400..500).rev() {
        let output = Command::new("dscl")
            .args([".", "-search", "/Users", "UniqueID", &uid.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                // If no user found with this UID, the output is empty
                if stdout.trim().is_empty() {
                    return Some(uid);
                }
            }
            _ => {
                // If dscl fails, assume UID is available
                return Some(uid);
            }
        }
    }

    None
}

/// Check the version of the installed helper binary
pub(super) async fn check_installed_helper_version() -> Option<String> {
    let output = Command::new("mino-sandbox-helper")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Map a distro name to its install command for a package.
///
/// For apt-based distros the caller must run `apt-get update` separately
/// before invoking the returned command.
pub(super) fn distro_install_cmd(distro: &str, package: &str) -> Vec<String> {
    match distro {
        "ubuntu" | "debian" => {
            vec![
                "apt-get".into(),
                "install".into(),
                "-y".into(),
                package.into(),
            ]
        }
        "fedora" | "rhel" | "centos" | "rocky" | "alma" => {
            vec!["dnf".into(), "install".into(), "-y".into(), package.into()]
        }
        "arch" => vec![
            "pacman".into(),
            "-S".into(),
            "--noconfirm".into(),
            package.into(),
        ],
        "opensuse" | "suse" => {
            vec![
                "zypper".into(),
                "install".into(),
                "-y".into(),
                package.into(),
            ]
        }
        _ => vec!["dnf".into(), "install".into(), "-y".into(), package.into()],
    }
}

/// Map a distro name to its upgrade command for a package.
///
/// For apt-based distros the caller must run `apt-get update` separately
/// before invoking the returned command.
pub(super) fn distro_upgrade_cmd(distro: &str, package: &str) -> Vec<String> {
    match distro {
        "ubuntu" | "debian" => {
            vec![
                "apt-get".into(),
                "upgrade".into(),
                "-y".into(),
                package.into(),
            ]
        }
        "fedora" | "rhel" | "centos" | "rocky" | "alma" => {
            vec!["dnf".into(), "upgrade".into(), "-y".into(), package.into()]
        }
        "arch" => vec![
            "pacman".into(),
            "-Syu".into(),
            "--noconfirm".into(),
            package.into(),
        ],
        "opensuse" | "suse" => {
            vec![
                "zypper".into(),
                "update".into(),
                "-y".into(),
                package.into(),
            ]
        }
        _ => vec!["dnf".into(), "upgrade".into(), "-y".into(), package.into()],
    }
}

/// Run a command, showing output to user
pub(super) async fn run_visible(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command in an OrbStack VM, showing output to user
pub(super) async fn run_visible_orb(vm_name: &str, args: &[&str]) -> bool {
    let mut cmd = Command::new("orb");
    cmd.arg("-m").arg(vm_name);
    cmd.args(args);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command with sudo, showing output to user
pub(super) async fn run_visible_sudo(cmd: &str, args: &[&str]) -> bool {
    Command::new("sudo")
        .arg(cmd)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Detect Linux package manager
pub(super) async fn detect_package_manager() -> Option<(&'static str, Vec<&'static str>)> {
    let managers = [
        ("dnf", vec!["install", "-y"]),
        ("apt-get", vec!["install", "-y"]),
        ("pacman", vec!["-S", "--noconfirm"]),
        ("zypper", vec!["install", "-y"]),
    ];

    for (cmd, args) in managers {
        let result = Command::new("which")
            .arg(cmd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        if result.map(|s| s.success()).unwrap_or(false) {
            return Some((cmd, args));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_result_is_ok() {
        assert!(StepResult::AlreadyOk.is_ok());
        assert!(StepResult::Installed.is_ok());
        assert!(!StepResult::Failed.is_ok());
        assert!(!StepResult::Skipped.is_ok());
        assert!(!StepResult::Blocked.is_ok());
    }

    #[test]
    fn step_result_is_issue() {
        // Only Failed and Skipped are user-actionable issues.
        // Blocked is a cascading consequence and should not inflate the count.
        assert!(!StepResult::AlreadyOk.is_issue());
        assert!(!StepResult::Installed.is_issue());
        assert!(StepResult::Failed.is_issue());
        assert!(StepResult::Skipped.is_issue());
        assert!(!StepResult::Blocked.is_issue());
    }

    #[test]
    fn distro_install_cmd_ubuntu() {
        let cmd = distro_install_cmd("ubuntu", "podman");
        assert_eq!(cmd, vec!["apt-get", "install", "-y", "podman"]);
    }

    #[test]
    fn distro_install_cmd_fedora() {
        let cmd = distro_install_cmd("fedora", "podman");
        assert_eq!(cmd, vec!["dnf", "install", "-y", "podman"]);
    }

    #[test]
    fn distro_install_cmd_arch() {
        let cmd = distro_install_cmd("arch", "podman");
        assert_eq!(cmd, vec!["pacman", "-S", "--noconfirm", "podman"]);
    }

    #[test]
    fn distro_install_cmd_opensuse() {
        let cmd = distro_install_cmd("opensuse", "podman");
        assert_eq!(cmd, vec!["zypper", "install", "-y", "podman"]);
    }

    #[test]
    fn distro_install_cmd_unknown_defaults_to_dnf() {
        let cmd = distro_install_cmd("gentoo", "podman");
        assert_eq!(cmd, vec!["dnf", "install", "-y", "podman"]);
    }

    #[test]
    fn distro_upgrade_cmd_debian() {
        let cmd = distro_upgrade_cmd("debian", "podman");
        assert_eq!(cmd, vec!["apt-get", "upgrade", "-y", "podman"]);
    }

    #[test]
    fn distro_upgrade_cmd_arch() {
        let cmd = distro_upgrade_cmd("arch", "podman");
        assert_eq!(cmd, vec!["pacman", "-Syu", "--noconfirm", "podman"]);
    }

    #[test]
    fn distro_upgrade_cmd_suse() {
        let cmd = distro_upgrade_cmd("suse", "podman");
        assert_eq!(cmd, vec!["zypper", "update", "-y", "podman"]);
    }

    #[test]
    fn distro_upgrade_cmd_unknown_defaults_to_dnf() {
        let cmd = distro_upgrade_cmd("nixos", "podman");
        assert_eq!(cmd, vec!["dnf", "upgrade", "-y", "podman"]);
    }
}
