//! Status command - check system health and dependencies

use crate::config::Config;
use crate::error::MinotaurResult;
use crate::orchestration::{create_runtime, OrbStack, Platform};
use console::{style, Emoji};
use std::process::Stdio;
use tokio::process::Command;

static CHECK: Emoji<'_, '_> = Emoji("✓ ", "[OK] ");
static CROSS: Emoji<'_, '_> = Emoji("✗ ", "[FAIL] ");
static WARN: Emoji<'_, '_> = Emoji("⚠ ", "[WARN] ");

/// Execute the status command
pub async fn execute(config: &Config) -> MinotaurResult<()> {
    println!("{}", style("Minotaur System Status").bold().cyan());
    println!();

    let mut all_ok = true;
    let platform = Platform::detect();

    // Show detected platform
    println!("{}", style("Platform:").bold());
    println!("  {} Detected: {}", CHECK, platform.name());

    // Check runtime based on platform
    match platform {
        Platform::MacOS => {
            all_ok &= check_orbstack().await;
            // Check Podman (if OrbStack is available)
            if OrbStack::is_installed().await {
                all_ok &= check_podman_in_vm(config).await;
            }
        }
        Platform::Linux => {
            all_ok &= check_native_podman().await;
        }
        Platform::Unsupported => {
            println!();
            println!(
                "  {} {} - Minotaur supports macOS and Linux only",
                CROSS,
                style("Unsupported platform").red()
            );
            all_ok = false;
        }
    }

    // Check cloud CLIs
    println!();
    println!("{}", style("Cloud CLIs:").bold());
    check_cli("aws", "aws --version", "brew install awscli").await;
    check_cli("gcloud", "gcloud --version", "brew install google-cloud-sdk").await;
    check_cli("az", "az --version", "brew install azure-cli").await;
    check_cli("gh", "gh --version", "brew install gh").await;

    // Check SSH agent
    println!();
    println!("{}", style("SSH Agent:").bold());
    check_ssh_agent().await;

    println!();
    if all_ok {
        println!("{}", style("All critical checks passed").green().bold());
    } else {
        println!(
            "{}",
            style("Some checks failed - see above for details").yellow().bold()
        );
    }

    Ok(())
}

async fn check_orbstack() -> bool {
    println!();
    println!("{}", style("OrbStack:").bold());

    if !OrbStack::is_installed().await {
        println!(
            "  {} {} - Install from https://orbstack.dev",
            CROSS,
            style("Not installed").red()
        );
        return false;
    }

    println!("  {} {}", CHECK, style("Installed").green());

    // Check if running
    match OrbStack::is_running().await {
        Ok(true) => {
            println!("  {} {}", CHECK, style("Running").green());
        }
        Ok(false) => {
            println!(
                "  {} {} - Run: orb start",
                WARN,
                style("Not running").yellow()
            );
            return false;
        }
        Err(e) => {
            println!("  {} {} - {}", CROSS, style("Error checking status").red(), e);
            return false;
        }
    }

    // Get version
    if let Ok(version) = OrbStack::version().await {
        println!("  {} Version: {}", CHECK, version);
    }

    true
}

async fn check_podman_in_vm(config: &Config) -> bool {
    println!();
    println!("{}", style("Podman (in VM):").bold());

    match create_runtime(config) {
        Ok(runtime) => match runtime.is_available().await {
            Ok(true) => {
                println!("  {} {}", CHECK, style("Available in VM").green());
                true
            }
            Ok(false) => {
                println!(
                    "  {} {} - Run: minotaur run (will auto-install)",
                    WARN,
                    style("Not installed in VM").yellow()
                );
                false
            }
            Err(e) => {
                println!("  {} {} - {}", CROSS, style("Error").red(), e);
                false
            }
        },
        Err(e) => {
            println!("  {} {} - {}", CROSS, style("Error").red(), e);
            false
        }
    }
}

async fn check_native_podman() -> bool {
    println!();
    println!("{}", style("Podman (native):").bold());

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
            println!("  {} {}", CHECK, style(first_line.trim()).green());
        }
        _ => {
            println!(
                "  {} {} - Install: sudo dnf install podman (or apt-get)",
                CROSS,
                style("Not installed").red()
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
                println!("  {} {}", CHECK, style("Rootless mode").green());
            } else {
                println!(
                    "  {} {} - Run: podman system migrate",
                    WARN,
                    style("Not in rootless mode").yellow()
                );
                return false;
            }
        }
        _ => {
            println!(
                "  {} {} - Could not check rootless status",
                WARN,
                style("Unknown").yellow()
            );
        }
    }

    true
}

async fn check_cli(name: &str, version_cmd: &str, install_hint: &str) {
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
            println!("  {} {} - {}", CHECK, style(name).green(), first_line.trim());
        }
        _ => {
            println!(
                "  {} {} - Not found. Install: {}",
                WARN,
                style(name).yellow(),
                install_hint
            );
        }
    }
}

async fn check_ssh_agent() {
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
                        println!(
                            "  {} {} ({} keys loaded)",
                            CHECK,
                            style("Running").green(),
                            key_count
                        );
                    } else {
                        println!(
                            "  {} {} - No keys loaded. Run: ssh-add",
                            WARN,
                            style("Running").yellow()
                        );
                    }
                }
                Err(_) => {
                    println!(
                        "  {} {} - ssh-add failed",
                        WARN,
                        style("Unknown").yellow()
                    );
                }
            }
            println!("  {} Socket: {}", CHECK, sock);
        }
        Err(_) => {
            println!(
                "  {} {} - SSH_AUTH_SOCK not set. Start ssh-agent.",
                CROSS,
                style("Not running").red()
            );
        }
    }
}
