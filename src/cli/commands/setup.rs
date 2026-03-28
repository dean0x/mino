//! Setup command - interactive prerequisite installation

use crate::cli::args::SetupArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::{OrbStack, Platform};
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

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

impl StepResult {
    /// Whether the step completed successfully (already ok or just installed).
    fn is_ok(self) -> bool {
        matches!(self, Self::AlreadyOk | Self::Installed)
    }

    /// Whether this step represents a user-actionable issue.
    ///
    /// Returns true for `Failed` and `Skipped` but NOT for `Blocked`,
    /// since blocked steps are cascading consequences of an upstream failure
    /// and should not be counted separately in the issue total.
    fn is_issue(self) -> bool {
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
            Platform::MacOS => uninstall_native_macos(&ctx).await,
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
            Platform::MacOS => setup_native_macos(&ctx, &args).await,
            Platform::Linux => setup_native_linux(&ctx, &args).await,
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
        Platform::MacOS => setup_macos(&ctx, &args, config).await,
        Platform::Linux => setup_linux(&ctx, &args).await,
        Platform::Unsupported => Err(MinoError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        )),
    }
}

async fn setup_macos(ctx: &UiContext, args: &SetupArgs, config: &Config) -> MinoResult<()> {
    ui::section(ctx, "Checking prerequisites...");

    // Step 1: Check Homebrew
    let homebrew_result = check_homebrew(ctx, args).await;

    // Step 2: Check OrbStack
    let orbstack_result = if homebrew_result.is_ok() {
        check_orbstack(ctx, args).await
    } else {
        ui::step_blocked(ctx, "OrbStack", "Homebrew");
        StepResult::Blocked
    };

    // Step 3: Check OrbStack running
    let orbstack_running_result = if orbstack_result.is_ok() {
        check_orbstack_running(ctx, args).await
    } else {
        ui::step_blocked(ctx, "OrbStack Service", "OrbStack");
        StepResult::Blocked
    };

    // Step 4: Check VM exists
    let vm_name = &config.vm.name;
    let vm_distro = &config.vm.distro;
    let vm_result = if orbstack_running_result.is_ok() {
        check_vm(ctx, args, vm_name, vm_distro).await
    } else {
        ui::step_blocked(ctx, &format!("Mino VM ({})", vm_name), "OrbStack");
        StepResult::Blocked
    };

    // Step 5: Check Podman in VM
    let podman_result = if vm_result.is_ok() {
        check_podman_in_vm(ctx, args, vm_name, vm_distro).await
    } else {
        ui::step_blocked(ctx, "Podman (in VM)", "VM");
        StepResult::Blocked
    };

    // Step 6: Check rootless Podman in VM
    let rootless_result = if podman_result.is_ok() {
        check_rootless_mode_in_vm(ctx, args, vm_name).await
    } else {
        ui::step_blocked(ctx, "Rootless Mode (in VM)", "Podman");
        StepResult::Blocked
    };

    // Summary
    let results = [
        homebrew_result,
        orbstack_result,
        orbstack_running_result,
        vm_result,
        podman_result,
        rootless_result,
    ];
    let issues = results.iter().filter(|r| r.is_issue()).count();

    if issues > 0 {
        if args.check {
            ui::outro_warn(
                ctx,
                &format!("{} issue(s) found. Run 'mino setup' to install.", issues),
            );
        } else {
            ui::outro_warn(ctx, "Setup incomplete - see above for details.");
        }
    } else {
        ui::outro_success(ctx, "Setup complete! Run 'mino run -- <command>' to start.");
    }

    Ok(())
}

async fn setup_linux(ctx: &UiContext, args: &SetupArgs) -> MinoResult<()> {
    ui::section(ctx, "Checking prerequisites...");

    // Step 1: Check/install Podman
    let podman_result = check_native_podman(ctx, args).await;

    // Step 2: Check rootless mode
    let rootless_result = if podman_result.is_ok() {
        check_rootless_mode(ctx, args).await
    } else {
        ui::step_blocked(ctx, "Rootless Mode", "Podman");
        StepResult::Blocked
    };

    // Step 3: Check user namespace support
    let userns_result = if rootless_result.is_ok() {
        check_user_namespaces(ctx, args).await
    } else {
        ui::step_blocked(ctx, "User Namespaces", "Rootless Mode");
        StepResult::Blocked
    };

    // Summary
    let results = [podman_result, rootless_result, userns_result];
    let issues = results.iter().filter(|r| r.is_issue()).count();

    if issues > 0 {
        if args.check {
            ui::outro_warn(
                ctx,
                &format!("{} issue(s) found. Run 'mino setup' to install.", issues),
            );
        } else {
            ui::outro_warn(ctx, "Setup incomplete - see above for details.");
        }
    } else {
        ui::outro_success(ctx, "Setup complete! Run 'mino run -- <command>' to start.");
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
        ui::step_ok_detail(ctx, "Mino VM exists", vm_name);
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error_detail(ctx, "Mino VM not found", vm_name);
        return StepResult::Failed;
    }

    ui::step_warn_hint(ctx, "Mino VM not found", vm_name);

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
            ui::remark(ctx, &format!("Try: orb delete {} && mino setup", vm_name));
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
async fn check_rootless_mode_in_vm(ctx: &UiContext, args: &SetupArgs, vm_name: &str) -> StepResult {
    // Get the username in the VM
    let whoami_output = Command::new("orb")
        .args(["-m", vm_name, "whoami"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    let username = match whoami_output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => {
            ui::step_error(ctx, "Could not determine VM username");
            return StepResult::Failed;
        }
    };

    // Check if subuid entry exists for the user
    // We grep for "^username:" to ensure exact match at start of line
    let subuid_check = Command::new("orb")
        .args([
            "-m",
            vm_name,
            "grep",
            "-q",
            &format!("^{}:", username),
            "/etc/subuid",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    let has_subuid = subuid_check.map(|s| s.success()).unwrap_or(false);

    // Check if subgid entry exists for the user
    let subgid_check = Command::new("orb")
        .args([
            "-m",
            vm_name,
            "grep",
            "-q",
            &format!("^{}:", username),
            "/etc/subgid",
        ])
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
    ui::remark(
        ctx,
        &format!("Adding subuid/subgid entries for '{}'", username),
    );

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
// Native Sandbox Setup (macOS)
// =============================================================================

async fn setup_native_macos(ctx: &UiContext, args: &SetupArgs) -> MinoResult<()> {
    ui::section(ctx, "Native Sandbox Setup (macOS)");

    let sandbox_user = crate::sandbox::config::DEFAULT_SANDBOX_USER;

    // Step 1: Create system user
    let user_result = setup_sandbox_user(ctx, args, sandbox_user).await;

    // Step 2: Install helper binary
    let helper_result = if user_result.is_ok() {
        install_helper_binary(ctx, args).await
    } else {
        ui::step_blocked(ctx, "Helper Binary", "System User");
        StepResult::Blocked
    };

    // Step 3: Configure sudoers
    let sudoers_result = if helper_result.is_ok() {
        configure_sudoers(ctx, args).await
    } else {
        ui::step_blocked(ctx, "Sudoers", "Helper Binary");
        StepResult::Blocked
    };

    // Step 4: Configure pf anchor
    let pf_result = if sudoers_result.is_ok() {
        configure_pf_anchor(ctx, args, sandbox_user).await
    } else {
        ui::step_blocked(ctx, "pf Anchor", "Sudoers");
        StepResult::Blocked
    };

    // Summary
    let issues = [user_result, helper_result, sudoers_result, pf_result]
        .iter()
        .filter(|r| r.is_issue())
        .count();

    if issues > 0 {
        ui::outro_warn(ctx, "Native sandbox setup incomplete. See issues above.");
    } else {
        ui::outro_success(
            ctx,
            "Native sandbox ready! Use: mino run --runtime native -- <command>",
        );
    }

    Ok(())
}

async fn setup_sandbox_user(ctx: &UiContext, args: &SetupArgs, username: &str) -> StepResult {
    if let Err(e) = crate::sandbox::config::validate_sandbox_user(username) {
        ui::step_error(ctx, &e.to_string());
        return StepResult::Failed;
    }

    // Check if user exists via dscl
    let exists = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", username)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if exists {
        ui::step_ok_detail(ctx, "Sandbox user exists", username);
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error_detail(ctx, "Sandbox user not found", username);
        return StepResult::Failed;
    }

    ui::remark(ctx, &format!("Creating system user '{}'...", username));

    // Find an available UID in the system range (400-499)
    let uid = match find_available_system_uid().await {
        Some(uid) => uid,
        None => {
            ui::step_error(ctx, "Failed to find available UID in range 400-499");
            return StepResult::Failed;
        }
    };

    // Create the user record
    let user_path = format!("/Users/{}", username);
    let uid_str = uid.to_string();

    let steps: &[(&[&str], &str)] = &[
        (&[".", "-create", &user_path], "create user record"),
        (
            &[".", "-create", &user_path, "UserShell", "/usr/bin/false"],
            "set shell",
        ),
        (
            &[".", "-create", &user_path, "RealName", "Mino Sandbox Agent"],
            "set real name",
        ),
        (
            &[".", "-create", &user_path, "UniqueID", &uid_str],
            "set UID",
        ),
        (
            &[".", "-create", &user_path, "PrimaryGroupID", "20"],
            "set group",
        ),
        (
            &[".", "-create", &user_path, "NFSHomeDirectory", "/var/empty"],
            "set home",
        ),
        (
            &[".", "-create", &user_path, "IsHidden", "1"],
            "hide from login screen",
        ),
    ];

    for (dscl_args, description) in steps {
        if !run_visible_sudo("dscl", dscl_args).await {
            ui::step_error_detail(ctx, "Failed to create sandbox user", description);
            return StepResult::Failed;
        }
    }

    ui::step_ok_detail(
        ctx,
        "Sandbox user created",
        &format!("{} (UID {})", username, uid),
    );
    StepResult::Installed
}

/// Find an available system UID in the 400-499 range
async fn find_available_system_uid() -> Option<u32> {
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

async fn install_helper_binary(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let mino_version = env!("CARGO_PKG_VERSION");

    // Check if helper exists and version matches
    let current_version = check_installed_helper_version().await;

    if let Some(version) = current_version {
        if version == mino_version {
            ui::step_ok_detail(ctx, "Helper binary", &format!("v{}", version));
            return StepResult::AlreadyOk;
        }
        ui::remark(
            ctx,
            &format!(
                "Helper version mismatch (v{} vs v{}), upgrading...",
                version, mino_version
            ),
        );
    }

    if args.check {
        ui::step_error(ctx, "Helper binary not installed or outdated");
        return StepResult::Failed;
    }

    // Get path to current mino binary, the helper is built alongside it
    let helper_src = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("mino-sandbox-helper")));

    match helper_src {
        Some(src) if src.exists() => {
            let src_str = src.to_string_lossy();
            let install_success =
                run_visible_sudo("cp", &[&src_str, "/usr/local/bin/mino-sandbox-helper"]).await;

            if install_success {
                // Ensure correct ownership and permissions
                let _ =
                    run_visible_sudo("chmod", &["755", "/usr/local/bin/mino-sandbox-helper"]).await;
                let _ = run_visible_sudo(
                    "chown",
                    &["root:wheel", "/usr/local/bin/mino-sandbox-helper"],
                )
                .await;

                ui::step_ok_detail(
                    ctx,
                    "Helper binary installed",
                    &format!("v{}", mino_version),
                );
                StepResult::Installed
            } else {
                ui::step_error(ctx, "Failed to install helper binary");
                StepResult::Failed
            }
        }
        _ => {
            ui::step_error_detail(
                ctx,
                "Helper binary not found next to mino executable",
                "Build with: cargo build --release",
            );
            StepResult::Failed
        }
    }
}

/// Check the version of the installed helper binary
async fn check_installed_helper_version() -> Option<String> {
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

async fn configure_sudoers(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let sudoers_file = "/etc/sudoers.d/mino";

    if std::path::Path::new(sudoers_file).exists() {
        ui::step_ok(ctx, "Sudoers configured");
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error(ctx, "Sudoers not configured");
        return StepResult::Failed;
    }

    // Get the current user's username
    let username = std::env::var("USER")
        .unwrap_or_else(|_| std::env::var("LOGNAME").unwrap_or_else(|_| "unknown".to_string()));

    // Validate username to prevent injection into sudoers file.
    // Only allow alphanumeric, underscore, and hyphen; max 32 chars.
    if username.is_empty()
        || username.len() > 32
        || !username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        ui::step_error(
            ctx,
            &format!(
                "Invalid username '{}' for sudoers (must be 1-32 alphanumeric/underscore/hyphen chars)",
                username
            ),
        );
        return StepResult::Failed;
    }

    let sudoers_content = format!(
        "{} ALL=(root) NOPASSWD: /usr/local/bin/mino-sandbox-helper\n",
        username
    );

    // Write to a temp file, then copy via sudo (avoiding sudo tee complexity)
    let tmp_file = std::env::temp_dir().join("mino-sudoers");
    if std::fs::write(&tmp_file, &sudoers_content).is_err() {
        ui::step_error(ctx, "Failed to write temporary sudoers file");
        return StepResult::Failed;
    }

    let tmp_str = tmp_file.to_string_lossy();
    let success = run_visible_sudo("cp", &[&tmp_str, sudoers_file]).await;
    let _ = std::fs::remove_file(&tmp_file);

    if success {
        // sudoers files must be mode 0440
        let _ = run_visible_sudo("chmod", &["0440", sudoers_file]).await;
        // Validate the sudoers file
        let valid = Command::new("sudo")
            .args(["visudo", "-c", "-f", sudoers_file])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if valid {
            ui::step_ok_detail(ctx, "Sudoers configured", &username);
            StepResult::Installed
        } else {
            // Invalid sudoers file — remove it to avoid locking out sudo
            let _ = run_visible_sudo("rm", &[sudoers_file]).await;
            ui::step_error(ctx, "Sudoers validation failed — file removed");
            StepResult::Failed
        }
    } else {
        ui::step_error(ctx, "Failed to install sudoers file");
        StepResult::Failed
    }
}

async fn configure_pf_anchor(ctx: &UiContext, args: &SetupArgs, sandbox_user: &str) -> StepResult {
    // Check if anchor exists in pf.conf
    let pf_check = Command::new("sudo")
        .args(["pfctl", "-s", "Anchors"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    if let Ok(output) = pf_check {
        let anchors = String::from_utf8_lossy(&output.stdout);
        if anchors.lines().any(|l| l.trim() == "mino") {
            ui::step_ok(ctx, "pf anchor configured");
            return StepResult::AlreadyOk;
        }
    }

    if args.check {
        ui::step_error(ctx, "pf anchor not configured");
        return StepResult::Failed;
    }

    // Generate and write anchor rules
    let anchor_rules = match crate::sandbox::macos::generate_pf_rules(sandbox_user, "default", None)
    {
        Ok(rules) => rules,
        Err(e) => {
            ui::step_error(ctx, &format!("Failed to generate pf rules: {}", e));
            return StepResult::Failed;
        }
    };
    let anchor_file = "/etc/pf.anchors/mino";

    let tmp_file = std::env::temp_dir().join("mino-pf-anchor");
    if std::fs::write(&tmp_file, &anchor_rules).is_err() {
        ui::step_error(ctx, "Failed to write temporary anchor file");
        return StepResult::Failed;
    }

    let tmp_str = tmp_file.to_string_lossy();
    let copy_success = run_visible_sudo("cp", &[&tmp_str, anchor_file]).await;
    let _ = std::fs::remove_file(&tmp_file);

    if !copy_success {
        ui::step_error(ctx, "Failed to install pf anchor file");
        return StepResult::Failed;
    }

    // Load the anchor
    let load_success = Command::new("sudo")
        .args(["pfctl", "-a", "mino", "-f", anchor_file])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if load_success {
        ui::step_ok(ctx, "pf anchor configured and loaded");
        StepResult::Installed
    } else {
        // Anchor file is written but not loaded — might need pf to be enabled
        ui::step_warn_hint(
            ctx,
            "pf anchor file installed but loading failed",
            "Ensure pf is enabled: sudo pfctl -e",
        );
        StepResult::Installed
    }
}

// =============================================================================
// Native Sandbox Uninstall (macOS)
// =============================================================================

/// Remove all native sandbox artifacts on macOS.
///
/// Steps (all require sudo):
/// 1. Kill any running `_mino_agent` processes
/// 2. Flush pf anchor rules
/// 3. Remove pf anchor file
/// 4. Remove sudoers entry
/// 5. Remove helper binary
/// 6. Delete `_mino_agent` system user
async fn uninstall_native_macos(ctx: &UiContext) -> MinoResult<()> {
    ui::section(ctx, "Removing native sandbox components...");

    let sandbox_user = crate::sandbox::config::DEFAULT_SANDBOX_USER;

    // 1. Kill any running processes owned by _mino_agent
    let kill_output = Command::new("sudo")
        .args(["pkill", "-u", sandbox_user])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    match kill_output {
        Ok(status) if status.success() => {
            ui::step_ok(ctx, "Killed running _mino_agent processes");
        }
        _ => {
            // pkill exits non-zero if no processes found — that's fine
            ui::step_ok(ctx, "No running _mino_agent processes");
        }
    }

    // 2. Flush pf anchor rules
    let _ = Command::new("sudo")
        .args(["pfctl", "-a", "mino", "-F", "rules"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    ui::step_ok(ctx, "Flushed pf anchor rules");

    // 3. Remove pf anchor file
    let pf_anchor = "/etc/pf.anchors/mino";
    if std::path::Path::new(pf_anchor).exists() {
        if run_visible_sudo("rm", &[pf_anchor]).await {
            ui::step_ok(ctx, "Removed pf anchor file");
        } else {
            ui::step_warn(ctx, "Failed to remove pf anchor file");
        }
    } else {
        ui::step_ok(ctx, "pf anchor file already removed");
    }

    // 4. Remove sudoers entry
    let sudoers_file = "/etc/sudoers.d/mino";
    if std::path::Path::new(sudoers_file).exists() {
        if run_visible_sudo("rm", &[sudoers_file]).await {
            ui::step_ok(ctx, "Removed sudoers entry");
        } else {
            ui::step_warn(ctx, "Failed to remove sudoers entry");
        }
    } else {
        ui::step_ok(ctx, "Sudoers entry already removed");
    }

    // 5. Remove helper binary
    let helper_path = "/usr/local/bin/mino-sandbox-helper";
    if std::path::Path::new(helper_path).exists() {
        if run_visible_sudo("rm", &[helper_path]).await {
            ui::step_ok(ctx, "Removed helper binary");
        } else {
            ui::step_warn(ctx, "Failed to remove helper binary");
        }
    } else {
        ui::step_ok(ctx, "Helper binary already removed");
    }

    // 6. Delete _mino_agent system user
    let user_exists = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", sandbox_user)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if user_exists {
        if run_visible_sudo(
            "dscl",
            &[".", "-delete", &format!("/Users/{}", sandbox_user)],
        )
        .await
        {
            ui::step_ok_detail(ctx, "Deleted system user", sandbox_user);
        } else {
            ui::step_warn(
                ctx,
                &format!("Failed to delete system user '{}'", sandbox_user),
            );
        }
    } else {
        ui::step_ok(ctx, "System user already removed");
    }

    ui::outro_success(ctx, "Native sandbox uninstalled.");
    Ok(())
}

// =============================================================================
// Native Sandbox Setup (Linux)
// =============================================================================

async fn setup_native_linux(ctx: &UiContext, args: &SetupArgs) -> MinoResult<()> {
    ui::section(ctx, "Native Sandbox Setup (Linux)");

    // Step 1: Verify user namespace support
    let userns_result = check_user_namespaces(ctx, args).await;

    // Step 2: Check unshare is available
    let unshare_result = check_unshare(ctx).await;

    // Summary
    let issues = [userns_result, unshare_result]
        .iter()
        .filter(|r| r.is_issue())
        .count();

    if issues > 0 {
        ui::outro_warn(
            ctx,
            "Native sandbox prerequisites not met. See issues above.",
        );
    } else {
        ui::outro_success(
            ctx,
            "Native sandbox ready! Use: mino run --runtime native -- <command>",
        );
    }

    Ok(())
}

async fn check_unshare(ctx: &UiContext) -> StepResult {
    let output = Command::new("which")
        .arg("unshare")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout);
            ui::step_ok_detail(ctx, "unshare available", path.trim());
            StepResult::AlreadyOk
        }
        _ => {
            ui::step_error_detail(
                ctx,
                "unshare not found",
                "Install util-linux for user namespace support",
            );
            StepResult::Failed
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
}
