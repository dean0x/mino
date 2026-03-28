//! Container runtime setup for macOS (OrbStack + Podman-in-VM)

use super::{run_visible, run_visible_orb, vm_exists, StepResult};
use crate::config::Config;
use crate::error::MinoResult;
use crate::orchestration::OrbStack;
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

use crate::cli::args::SetupArgs;

pub(super) async fn setup_macos(
    ctx: &UiContext,
    args: &SetupArgs,
    config: &Config,
) -> MinoResult<()> {
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
                }

                let install_cmd = super::distro_install_cmd(vm_distro, "podman");
                let install_args: Vec<&str> = std::iter::once("sudo")
                    .chain(install_cmd.iter().map(String::as_str))
                    .collect();

                if run_visible_orb(vm_name, &install_args).await {
                    ui::step_ok(ctx, "Podman installed");
                    return StepResult::Installed;
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
    // For apt-based systems, run update first
    if vm_distro == "ubuntu" || vm_distro == "debian" {
        let update_success = run_visible_orb(vm_name, &["sudo", "apt-get", "update"]).await;
        if !update_success {
            ui::remark(ctx, "Package update failed, skipping upgrade");
            return false;
        }
    }

    let upgrade_cmd = super::distro_upgrade_cmd(vm_distro, "podman");
    let upgrade_args: Vec<&str> = std::iter::once("sudo")
        .chain(upgrade_cmd.iter().map(String::as_str))
        .collect();

    run_visible_orb(vm_name, &upgrade_args).await
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
