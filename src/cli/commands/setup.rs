//! Setup command - interactive prerequisite installation

use crate::config::Config;
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::{OrbStack, Platform};
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

use super::super::args::SetupArgs;

/// Setup step result for tracking what was done
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepResult {
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

/// Execute the setup command
pub async fn execute(args: SetupArgs, config: &Config) -> MinotaurResult<()> {
    let ctx = UiContext::detect().with_auto_yes(args.yes);

    if args.check {
        ui::intro(&ctx, "Minotaur Setup (check only)");
    } else {
        ui::intro(&ctx, "Minotaur Setup");
    }

    match Platform::detect() {
        Platform::MacOS => setup_macos(&ctx, &args, config).await,
        Platform::Linux => setup_linux(&ctx, &args).await,
        Platform::Unsupported => Err(MinotaurError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        )),
    }
}

async fn setup_macos(ctx: &UiContext, args: &SetupArgs, config: &Config) -> MinotaurResult<()> {
    ui::section(ctx, "Checking prerequisites...");

    let mut issues = 0;

    // Step 1: Check Homebrew
    let homebrew_result = check_homebrew(ctx, args).await;
    if homebrew_result == StepResult::Failed || homebrew_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 2: Check OrbStack
    let orbstack_result =
        if homebrew_result == StepResult::AlreadyOk || homebrew_result == StepResult::Installed {
            check_orbstack(ctx, args).await
        } else {
            ui::step_blocked(ctx, "OrbStack", "Homebrew");
            StepResult::Blocked
        };
    if orbstack_result == StepResult::Failed || orbstack_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 3: Check OrbStack running
    let orbstack_running_result =
        if orbstack_result == StepResult::AlreadyOk || orbstack_result == StepResult::Installed {
            check_orbstack_running(ctx, args).await
        } else {
            ui::step_blocked(ctx, "OrbStack Service", "OrbStack");
            StepResult::Blocked
        };
    if orbstack_running_result == StepResult::Failed
        || orbstack_running_result == StepResult::Skipped
    {
        issues += 1;
    }

    // Step 4: Check VM exists
    let vm_name = &config.vm.name;
    let vm_distro = &config.vm.distro;
    let vm_result = if orbstack_running_result == StepResult::AlreadyOk
        || orbstack_running_result == StepResult::Installed
    {
        check_vm(ctx, args, vm_name, vm_distro).await
    } else {
        ui::step_blocked(ctx, &format!("Minotaur VM ({})", vm_name), "OrbStack");
        StepResult::Blocked
    };
    if vm_result == StepResult::Failed || vm_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 5: Check Podman in VM
    let podman_result = if vm_result == StepResult::AlreadyOk || vm_result == StepResult::Installed
    {
        check_podman_in_vm(ctx, args, vm_name, vm_distro).await
    } else {
        ui::step_blocked(ctx, "Podman (in VM)", "VM");
        StepResult::Blocked
    };
    if podman_result == StepResult::Failed || podman_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 6: Check rootless Podman in VM
    let rootless_result =
        if podman_result == StepResult::AlreadyOk || podman_result == StepResult::Installed {
            check_rootless_mode_in_vm(ctx, args, vm_name).await
        } else {
            ui::step_blocked(ctx, "Rootless Mode (in VM)", "Podman");
            StepResult::Blocked
        };
    if rootless_result == StepResult::Failed || rootless_result == StepResult::Skipped {
        issues += 1;
    }

    // Summary
    if issues > 0 {
        if args.check {
            ui::outro_warn(
                ctx,
                &format!(
                    "{} issue(s) found. Run 'minotaur setup' to install.",
                    issues
                ),
            );
        } else {
            ui::outro_warn(ctx, "Setup incomplete - see above for details.");
        }
    } else {
        ui::outro_success(
            ctx,
            "Setup complete! Run 'minotaur run -- <command>' to start.",
        );
    }

    Ok(())
}

async fn setup_linux(ctx: &UiContext, args: &SetupArgs) -> MinotaurResult<()> {
    ui::section(ctx, "Checking prerequisites...");

    let mut issues = 0;

    // Step 1: Check/install Podman
    let podman_result = check_native_podman(ctx, args).await;
    if podman_result == StepResult::Failed || podman_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 2: Check rootless mode
    let rootless_result =
        if podman_result == StepResult::AlreadyOk || podman_result == StepResult::Installed {
            check_rootless_mode(ctx, args).await
        } else {
            ui::step_blocked(ctx, "Rootless Mode", "Podman");
            StepResult::Blocked
        };
    if rootless_result == StepResult::Failed || rootless_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 3: Check user namespace support
    let userns_result =
        if rootless_result == StepResult::AlreadyOk || rootless_result == StepResult::Installed {
            check_user_namespaces(ctx, args).await
        } else {
            ui::step_blocked(ctx, "User Namespaces", "Rootless Mode");
            StepResult::Blocked
        };
    if userns_result == StepResult::Failed || userns_result == StepResult::Skipped {
        issues += 1;
    }

    // Summary
    if issues > 0 {
        if args.check {
            ui::outro_warn(
                ctx,
                &format!(
                    "{} issue(s) found. Run 'minotaur setup' to install.",
                    issues
                ),
            );
        } else {
            ui::outro_warn(ctx, "Setup incomplete - see above for details.");
        }
    } else {
        ui::outro_success(
            ctx,
            "Setup complete! Run 'minotaur run -- <command>' to start.",
        );
    }

    Ok(())
}

// =============================================================================
// macOS Steps
// =============================================================================

/// Check if an OrbStack VM exists by name
async fn vm_exists(vm_name: &str) -> bool {
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

async fn check_homebrew(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let output = Command::new("brew")
        .arg("--prefix")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let prefix = String::from_utf8_lossy(&out.stdout);
            ui::step_ok_detail(ctx, "Homebrew installed", prefix.trim());
            StepResult::AlreadyOk
        }
        _ => {
            if args.check {
                ui::step_error(ctx, "Homebrew not installed");
                return StepResult::Failed;
            }

            ui::step_warn_hint(ctx, "Homebrew not installed", "https://brew.sh");

            if ui::confirm_inline("Install Homebrew now?", args.yes) {
                ui::remark(ctx, "Running Homebrew installer...");

                let install_result = run_visible(
                    "/bin/bash",
                    &[
                        "-c",
                        "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)",
                    ],
                )
                .await;

                if install_result {
                    ui::step_ok(ctx, "Homebrew installed");
                    StepResult::Installed
                } else {
                    ui::step_error_detail(
                        ctx,
                        "Homebrew installation failed",
                        "Visit https://brew.sh",
                    );
                    StepResult::Failed
                }
            } else {
                ui::remark(ctx, "Skipped Homebrew installation");
                StepResult::Skipped
            }
        }
    }
}

async fn check_orbstack(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    if OrbStack::is_installed().await {
        if let Ok(version) = OrbStack::version().await {
            ui::step_ok_detail(ctx, "OrbStack installed", &version);
        } else {
            ui::step_ok(ctx, "OrbStack installed");
        }

        // Upgrade if requested
        if args.upgrade {
            ui::remark(ctx, "Running: brew upgrade --cask orbstack");
            if run_visible("brew", &["upgrade", "--cask", "orbstack"]).await {
                if let Ok(new_version) = OrbStack::version().await {
                    ui::step_ok_detail(ctx, "OrbStack upgraded", &new_version);
                }
            }
            // Don't fail if upgrade fails - package might already be latest
        }

        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error(ctx, "OrbStack not installed");
        return StepResult::Failed;
    }

    ui::step_warn(ctx, "OrbStack not installed");

    if ui::confirm_inline("Install OrbStack via Homebrew?", args.yes) {
        ui::remark(ctx, "Running: brew install --cask orbstack");

        if run_visible("brew", &["install", "--cask", "orbstack"]).await {
            ui::step_ok(ctx, "OrbStack installed");
            StepResult::Installed
        } else {
            ui::step_error_detail(ctx, "OrbStack installation failed", "https://orbstack.dev");
            StepResult::Failed
        }
    } else {
        ui::remark(ctx, "Skipped OrbStack installation");
        StepResult::Skipped
    }
}

async fn check_orbstack_running(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    match OrbStack::is_running().await {
        Ok(true) => {
            ui::step_ok(ctx, "OrbStack running");
            StepResult::AlreadyOk
        }
        Ok(false) => {
            if args.check {
                ui::step_warn_hint(ctx, "OrbStack not running", "Run: orb start");
                return StepResult::Failed;
            }

            ui::remark(ctx, "Starting OrbStack...");
            match OrbStack::start().await {
                Ok(()) => {
                    ui::step_ok(ctx, "OrbStack started");
                    StepResult::Installed
                }
                Err(e) => {
                    ui::step_error_detail(ctx, "Failed to start OrbStack", &e.to_string());
                    ui::remark(ctx, "Try starting OrbStack manually from Applications");
                    StepResult::Failed
                }
            }
        }
        Err(e) => {
            ui::step_error_detail(ctx, "Error checking OrbStack status", &e.to_string());
            StepResult::Failed
        }
    }
}

async fn check_vm(ctx: &UiContext, args: &SetupArgs, vm_name: &str, vm_distro: &str) -> StepResult {
    if vm_exists(vm_name).await {
        ui::step_ok_detail(ctx, "Minotaur VM exists", vm_name);
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error_detail(ctx, "Minotaur VM not found", vm_name);
        return StepResult::Failed;
    }

    ui::step_warn_hint(ctx, "Minotaur VM not found", vm_name);

    if ui::confirm_inline(&format!("Create {} VM '{}'?", vm_distro, vm_name), args.yes) {
        ui::remark(ctx, "Creating VM...");

        if run_visible("orb", &["create", vm_distro, vm_name]).await {
            ui::step_ok_detail(ctx, "VM created", vm_name);
            StepResult::Installed
        } else if vm_exists(vm_name).await {
            // VM was created externally (e.g., via OrbStack UI) while we were waiting
            ui::step_ok_detail(ctx, "VM already exists", vm_name);
            StepResult::AlreadyOk
        } else {
            ui::step_error(ctx, "VM creation failed");
            ui::remark(
                ctx,
                &format!("Try: orb delete {} && minotaur setup", vm_name),
            );
            StepResult::Failed
        }
    } else {
        ui::remark(ctx, "Skipped VM creation");
        StepResult::Skipped
    }
}

async fn check_podman_in_vm(
    ctx: &UiContext,
    args: &SetupArgs,
    vm_name: &str,
    vm_distro: &str,
) -> StepResult {
    let output = Command::new("orb")
        .args(["-m", vm_name, "podman", "--version"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            let first_line = version.lines().next().unwrap_or("unknown");
            ui::step_ok_detail(ctx, "Podman installed in VM", first_line.trim());

            // Upgrade if requested
            if args.upgrade {
                ui::remark(ctx, "Upgrading Podman in VM...");
                let upgrade_success = upgrade_podman_in_vm(ctx, vm_name, vm_distro).await;
                if upgrade_success {
                    // Show new version
                    let new_output = Command::new("orb")
                        .args(["-m", vm_name, "podman", "--version"])
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .output()
                        .await;
                    if let Ok(out) = new_output {
                        if out.status.success() {
                            let new_version = String::from_utf8_lossy(&out.stdout);
                            let new_first_line = new_version.lines().next().unwrap_or("unknown");
                            ui::step_ok_detail(ctx, "Podman upgraded", new_first_line.trim());
                        }
                    }
                }
                // Don't fail if upgrade fails - package might already be latest
            }

            StepResult::AlreadyOk
        }
        _ => {
            if args.check {
                ui::step_error(ctx, "Podman not installed in VM");
                return StepResult::Failed;
            }

            ui::step_warn(ctx, "Podman not installed in VM");

            if ui::confirm_inline("Install Podman in VM?", args.yes) {
                ui::remark(ctx, "Installing Podman...");

                // For apt-based systems, we need to run update first
                if vm_distro == "ubuntu" || vm_distro == "debian" {
                    let update_success =
                        run_visible_orb(vm_name, &["sudo", "apt-get", "update"]).await;
                    if !update_success {
                        ui::step_error(ctx, "Package update failed");
                        return StepResult::Failed;
                    }

                    if run_visible_orb(vm_name, &["sudo", "apt-get", "install", "-y", "podman"])
                        .await
                    {
                        ui::step_ok(ctx, "Podman installed");
                        return StepResult::Installed;
                    }
                } else {
                    // Determine package manager based on distro
                    let install_cmd = match vm_distro {
                        "fedora" | "rhel" | "centos" | "rocky" | "alma" => {
                            vec!["sudo", "dnf", "install", "-y", "podman"]
                        }
                        "arch" => {
                            vec!["sudo", "pacman", "-S", "--noconfirm", "podman"]
                        }
                        "opensuse" | "suse" => {
                            vec!["sudo", "zypper", "install", "-y", "podman"]
                        }
                        _ => {
                            // Default to dnf for unknown distros
                            vec!["sudo", "dnf", "install", "-y", "podman"]
                        }
                    };

                    if run_visible_orb(vm_name, &install_cmd).await {
                        ui::step_ok(ctx, "Podman installed");
                        return StepResult::Installed;
                    }
                }

                ui::step_error(ctx, "Podman installation failed");
                StepResult::Failed
            } else {
                ui::remark(ctx, "Skipped Podman installation");
                StepResult::Skipped
            }
        }
    }
}

/// Upgrade Podman in VM using the appropriate package manager
async fn upgrade_podman_in_vm(ctx: &UiContext, vm_name: &str, vm_distro: &str) -> bool {
    match vm_distro {
        "ubuntu" | "debian" => {
            let update_success = run_visible_orb(vm_name, &["sudo", "apt-get", "update"]).await;
            if !update_success {
                ui::remark(ctx, "Package update failed, skipping upgrade");
                return false;
            }
            run_visible_orb(vm_name, &["sudo", "apt-get", "upgrade", "-y", "podman"]).await
        }
        "fedora" | "rhel" | "centos" | "rocky" | "alma" => {
            run_visible_orb(vm_name, &["sudo", "dnf", "upgrade", "-y", "podman"]).await
        }
        "arch" => {
            run_visible_orb(
                vm_name,
                &["sudo", "pacman", "-Syu", "--noconfirm", "podman"],
            )
            .await
        }
        "opensuse" | "suse" => {
            run_visible_orb(vm_name, &["sudo", "zypper", "update", "-y", "podman"]).await
        }
        _ => {
            // Default to dnf for unknown distros
            run_visible_orb(vm_name, &["sudo", "dnf", "upgrade", "-y", "podman"]).await
        }
    }
}

/// Check and configure rootless Podman mode in VM
///
/// Note: `podman info --format {{.Host.Security.Rootless}}` returns true even when
/// subuid/subgid aren't configured, so we must explicitly check those files.
async fn check_rootless_mode_in_vm(
    ctx: &UiContext,
    args: &SetupArgs,
    vm_name: &str,
) -> StepResult {
    // Get the username in the VM
    let whoami_output = Command::new("orb")
        .args(["-m", vm_name, "whoami"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    let username = match whoami_output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => {
            ui::step_error(ctx, "Could not determine VM username");
            return StepResult::Failed;
        }
    };

    // Check if subuid entry exists for the user
    // We grep for "^username:" to ensure exact match at start of line
    let subuid_check = Command::new("orb")
        .args(["-m", vm_name, "grep", "-q", &format!("^{}:", username), "/etc/subuid"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    let has_subuid = subuid_check.map(|s| s.success()).unwrap_or(false);

    // Check if subgid entry exists for the user
    let subgid_check = Command::new("orb")
        .args(["-m", vm_name, "grep", "-q", &format!("^{}:", username), "/etc/subgid"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    let has_subgid = subgid_check.map(|s| s.success()).unwrap_or(false);

    if has_subuid && has_subgid {
        ui::step_ok_detail(ctx, "Rootless mode configured in VM", &username);
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error_detail(
            ctx,
            "Rootless mode not configured in VM",
            "subuid/subgid not set up",
        );
        return StepResult::Failed;
    }

    ui::step_warn(ctx, "Configuring rootless Podman in VM...");
    ui::remark(ctx, &format!("Adding subuid/subgid entries for '{}'", username));

    if !has_subuid {
        // Add subuid entry
        let subuid_cmd = format!("echo '{}:100000:65536' | sudo tee -a /etc/subuid", username);
        let subuid_result = run_visible_orb(vm_name, &["sh", "-c", &subuid_cmd]).await;
        if !subuid_result {
            ui::step_error(ctx, "Failed to configure /etc/subuid");
            return StepResult::Failed;
        }
    }

    if !has_subgid {
        // Add subgid entry
        let subgid_cmd = format!("echo '{}:100000:65536' | sudo tee -a /etc/subgid", username);
        let subgid_result = run_visible_orb(vm_name, &["sh", "-c", &subgid_cmd]).await;
        if !subgid_result {
            ui::step_error(ctx, "Failed to configure /etc/subgid");
            return StepResult::Failed;
        }
    }

    // Run podman system migrate to apply the configuration
    ui::remark(ctx, "Running: podman system migrate");
    if run_visible_orb(vm_name, &["podman", "system", "migrate"]).await {
        ui::step_ok(ctx, "Rootless mode configured in VM");
        StepResult::Installed
    } else {
        ui::step_error(ctx, "Failed to run podman system migrate");
        StepResult::Failed
    }
}

// =============================================================================
// Linux Steps
// =============================================================================

async fn check_native_podman(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let output = Command::new("podman")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            let first_line = version.lines().next().unwrap_or("unknown");
            ui::step_ok_detail(ctx, "Podman", first_line.trim());

            // Upgrade if requested
            if args.upgrade {
                if let Some((name, _)) = detect_package_manager().await {
                    ui::remark(ctx, &format!("Upgrading Podman via {}...", name));

                    let upgrade_args = match name {
                        "dnf" => vec!["upgrade", "-y", "podman"],
                        "apt-get" => {
                            // Run apt-get update first
                            let _ = run_visible_sudo("apt-get", &["update"]).await;
                            vec!["upgrade", "-y", "podman"]
                        }
                        "pacman" => vec!["-Syu", "--noconfirm", "podman"],
                        "zypper" => vec!["update", "-y", "podman"],
                        _ => vec!["upgrade", "-y", "podman"],
                    };

                    if run_visible_sudo(name, &upgrade_args).await {
                        // Show new version
                        let new_output = Command::new("podman")
                            .arg("--version")
                            .stdout(Stdio::piped())
                            .stderr(Stdio::null())
                            .output()
                            .await;
                        if let Ok(out) = new_output {
                            if out.status.success() {
                                let new_version = String::from_utf8_lossy(&out.stdout);
                                let new_first_line =
                                    new_version.lines().next().unwrap_or("unknown");
                                ui::step_ok_detail(ctx, "Podman upgraded", new_first_line.trim());
                            }
                        }
                    }
                    // Don't fail if upgrade fails - package might already be latest
                }
            }

            StepResult::AlreadyOk
        }
        _ => {
            if args.check {
                ui::step_error(ctx, "Podman not installed");
                return StepResult::Failed;
            }

            ui::step_warn(ctx, "Podman not installed");

            let pkg_manager = detect_package_manager().await;
            match pkg_manager {
                Some((name, _)) => {
                    if ui::confirm_inline(&format!("Install Podman via {}?", name), args.yes) {
                        let mut cmd_args = vec!["install", "-y", "podman"];
                        if name == "pacman" {
                            cmd_args = vec!["-S", "--noconfirm", "podman"];
                        }

                        ui::remark(
                            ctx,
                            &format!("Running: sudo {} {}", name, cmd_args.join(" ")),
                        );

                        if run_visible_sudo(name, &cmd_args).await {
                            ui::step_ok(ctx, "Podman installed");
                            StepResult::Installed
                        } else {
                            ui::step_error(ctx, "Podman installation failed");
                            StepResult::Failed
                        }
                    } else {
                        ui::remark(ctx, "Skipped Podman installation");
                        StepResult::Skipped
                    }
                }
                None => {
                    ui::step_error(ctx, "Could not detect package manager");
                    ui::remark(ctx, "Supported: dnf, apt-get, pacman, zypper");
                    StepResult::Failed
                }
            }
        }
    }
}

async fn check_rootless_mode(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let output = Command::new("podman")
        .args(["info", "--format", "{{.Host.Security.Rootless}}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.trim() == "true" {
                ui::step_ok(ctx, "Rootless mode enabled");
                StepResult::AlreadyOk
            } else {
                if args.check {
                    ui::step_warn_hint(
                        ctx,
                        "Rootless mode not enabled",
                        "Run: podman system migrate",
                    );
                    return StepResult::Failed;
                }

                ui::remark(ctx, "Running: podman system migrate");
                if run_visible("podman", &["system", "migrate"]).await {
                    ui::step_ok(ctx, "Rootless mode configured");
                    StepResult::Installed
                } else {
                    ui::step_error(ctx, "Failed to configure rootless mode");
                    StepResult::Failed
                }
            }
        }
        _ => {
            ui::step_error(ctx, "Could not check rootless status");
            StepResult::Failed
        }
    }
}

async fn check_user_namespaces(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    // Check if user namespaces are enabled
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

// =============================================================================
// Helpers
// =============================================================================

/// Run a command, showing output to user
async fn run_visible(cmd: &str, args: &[&str]) -> bool {
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
async fn run_visible_orb(vm_name: &str, args: &[&str]) -> bool {
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
async fn run_visible_sudo(cmd: &str, args: &[&str]) -> bool {
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
async fn detect_package_manager() -> Option<(&'static str, Vec<&'static str>)> {
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
    fn step_result_eq() {
        assert_eq!(StepResult::AlreadyOk, StepResult::AlreadyOk);
        assert_ne!(StepResult::AlreadyOk, StepResult::Failed);
    }
}
