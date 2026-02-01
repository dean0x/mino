//! Status command - check system health and dependencies

use crate::config::Config;
use crate::error::MinotaurResult;
use crate::orchestration::{create_runtime, OrbStack, Platform};
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

/// Execute the status command
pub async fn execute(config: &Config) -> MinotaurResult<()> {
    let ctx = UiContext::detect();

    ui::intro(&ctx, "Minotaur System Status");

    let mut all_ok = true;
    let platform = Platform::detect();

    // Show detected platform
    ui::section(&ctx, "Platform");
    ui::step_ok_detail(&ctx, "Detected", platform.name());

    // Check runtime based on platform
    match platform {
        Platform::MacOS => {
            all_ok &= check_orbstack(&ctx).await;
            // Check Podman (if OrbStack is available)
            if OrbStack::is_installed().await {
                all_ok &= check_podman_in_vm(&ctx, config).await;
            }
        }
        Platform::Linux => {
            all_ok &= check_native_podman(&ctx).await;
        }
        Platform::Unsupported => {
            ui::step_error(&ctx, "Unsupported platform - Minotaur supports macOS and Linux only");
            all_ok = false;
        }
    }

    // Check cloud CLIs
    ui::section(&ctx, "Cloud CLIs");
    check_cli(&ctx, "aws", "aws --version", "brew install awscli").await;
    check_cli(&ctx, "gcloud", "gcloud --version", "brew install google-cloud-sdk").await;
    check_cli(&ctx, "az", "az --version", "brew install azure-cli").await;
    check_cli(&ctx, "gh", "gh --version", "brew install gh").await;

    // Check SSH agent
    ui::section(&ctx, "SSH Agent");
    check_ssh_agent(&ctx).await;

    if all_ok {
        ui::outro_success(&ctx, "All critical checks passed");
    } else {
        ui::outro_warn(&ctx, "Some checks failed - see above for details");
    }

    Ok(())
}

async fn check_orbstack(ctx: &UiContext) -> bool {
    ui::section(ctx, "OrbStack");

    if !OrbStack::is_installed().await {
        ui::step_error_detail(ctx, "Not installed", "Install from https://orbstack.dev");
        return false;
    }

    ui::step_ok(ctx, "Installed");

    // Check if running
    match OrbStack::is_running().await {
        Ok(true) => {
            ui::step_ok(ctx, "Running");
        }
        Ok(false) => {
            ui::step_warn_hint(ctx, "Not running", "Run: orb start");
            return false;
        }
        Err(e) => {
            ui::step_error_detail(ctx, "Error checking status", &e.to_string());
            return false;
        }
    }

    // Get version
    if let Ok(version) = OrbStack::version().await {
        ui::step_ok_detail(ctx, "Version", &version);
    }

    true
}

async fn check_podman_in_vm(ctx: &UiContext, config: &Config) -> bool {
    ui::section(ctx, "Podman (in VM)");

    match create_runtime(config) {
        Ok(runtime) => match runtime.is_available().await {
            Ok(true) => {
                ui::step_ok(ctx, "Available in VM");
                true
            }
            Ok(false) => {
                ui::step_warn_hint(
                    ctx,
                    "Not installed in VM",
                    "Run: minotaur setup (will auto-install)",
                );
                false
            }
            Err(e) => {
                ui::step_error_detail(ctx, "Error", &e.to_string());
                false
            }
        },
        Err(e) => {
            ui::step_error_detail(ctx, "Error", &e.to_string());
            false
        }
    }
}

async fn check_native_podman(ctx: &UiContext) -> bool {
    ui::section(ctx, "Podman (native)");

    // Check if podman is installed
    let installed = Command::new("podman")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match installed {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("unknown");
            ui::step_ok_detail(ctx, "Installed", first_line.trim());
        }
        _ => {
            ui::step_error_detail(
                ctx,
                "Not installed",
                "Install: sudo dnf install podman (or apt-get)",
            );
            return false;
        }
    }

    // Check if rootless is configured
    let rootless = Command::new("podman")
        .args(["info", "--format", "{{.Host.Security.Rootless}}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match rootless {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim() == "true" {
                ui::step_ok(ctx, "Rootless mode");
            } else {
                ui::step_warn_hint(ctx, "Not in rootless mode", "Run: podman system migrate");
                return false;
            }
        }
        _ => {
            ui::step_warn(ctx, "Could not check rootless status");
        }
    }

    true
}

async fn check_cli(ctx: &UiContext, name: &str, version_cmd: &str, install_hint: &str) {
    let parts: Vec<&str> = version_cmd.split_whitespace().collect();
    let result = Command::new(parts[0])
        .args(&parts[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("unknown");
            ui::step_ok_detail(ctx, name, first_line.trim());
        }
        _ => {
            ui::step_warn_hint(ctx, &format!("{} not found", name), &format!("Install: {}", install_hint));
        }
    }
}

async fn check_ssh_agent(ctx: &UiContext) {
    match std::env::var("SSH_AUTH_SOCK") {
        Ok(sock) => {
            // Try to list identities
            let result = Command::new("ssh-add")
                .arg("-l")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await;

            match result {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let key_count = stdout.lines().count();
                    if output.status.success() && key_count > 0 {
                        ui::step_ok_detail(ctx, "Running", &format!("{} keys loaded", key_count));
                    } else {
                        ui::step_warn_hint(ctx, "Running", "No keys loaded. Run: ssh-add");
                    }
                }
                Err(_) => {
                    ui::step_warn(ctx, "ssh-add failed");
                }
            }
            ui::key_value(ctx, "Socket", &sock);
        }
        Err(_) => {
            ui::step_error_detail(
                ctx,
                "Not running",
                "SSH_AUTH_SOCK not set. Start ssh-agent.",
            );
        }
    }
}
