//! Container runtime setup for Linux (native Podman + rootless mode)

use super::{detect_package_manager, run_visible, run_visible_sudo, StepResult};
use crate::error::MinoResult;
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

use crate::cli::args::SetupArgs;

pub(super) async fn setup_linux(ctx: &UiContext, args: &SetupArgs) -> MinoResult<()> {
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
        super::check_user_namespaces(ctx, args).await
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
