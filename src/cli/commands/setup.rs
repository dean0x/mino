//! Setup command - interactive prerequisite installation

use crate::config::Config;
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::{OrbStack, Platform};
use console::{style, Emoji};
use std::io::{self, Write};
use std::process::Stdio;
use tokio::process::Command;

use super::super::args::SetupArgs;

static CHECK: Emoji<'_, '_> = Emoji("✓ ", "[OK] ");
static CROSS: Emoji<'_, '_> = Emoji("✗ ", "[FAIL] ");
static CIRCLE: Emoji<'_, '_> = Emoji("○ ", "[ ] ");
static DASH: Emoji<'_, '_> = Emoji("- ", "[-] ");

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
    if args.check {
        println!("{}", style("Minotaur Setup (check only)").bold().cyan());
    } else {
        println!("{}", style("Minotaur Setup").bold().cyan());
    }
    println!();

    match Platform::detect() {
        Platform::MacOS => setup_macos(&args, config).await,
        Platform::Linux => setup_linux(&args).await,
        Platform::Unsupported => Err(MinotaurError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        )),
    }
}

async fn setup_macos(args: &SetupArgs, config: &Config) -> MinotaurResult<()> {
    println!("{}", style("Checking prerequisites...").bold());
    println!();

    let mut issues = 0;

    // Step 1: Check Homebrew
    let homebrew_result = check_homebrew(args).await;
    if homebrew_result == StepResult::Failed || homebrew_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 2: Check OrbStack
    let orbstack_result = if homebrew_result == StepResult::AlreadyOk
        || homebrew_result == StepResult::Installed
    {
        check_orbstack(args).await
    } else {
        print_blocked("OrbStack", "Homebrew");
        StepResult::Blocked
    };
    if orbstack_result == StepResult::Failed || orbstack_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 3: Check OrbStack running
    let orbstack_running_result = if orbstack_result == StepResult::AlreadyOk
        || orbstack_result == StepResult::Installed
    {
        check_orbstack_running(args).await
    } else {
        print_blocked("OrbStack Service", "OrbStack");
        StepResult::Blocked
    };
    if orbstack_running_result == StepResult::Failed || orbstack_running_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 4: Check VM exists
    let vm_name = &config.vm.name;
    let vm_distro = &config.vm.distro;
    let vm_result = if orbstack_running_result == StepResult::AlreadyOk
        || orbstack_running_result == StepResult::Installed
    {
        check_vm(args, vm_name, vm_distro).await
    } else {
        print_blocked(&format!("Minotaur VM ({})", vm_name), "OrbStack");
        StepResult::Blocked
    };
    if vm_result == StepResult::Failed || vm_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 5: Check Podman in VM
    let podman_result = if vm_result == StepResult::AlreadyOk || vm_result == StepResult::Installed {
        check_podman_in_vm(args, vm_name, vm_distro).await
    } else {
        print_blocked("Podman (in VM)", "VM");
        StepResult::Blocked
    };
    if podman_result == StepResult::Failed || podman_result == StepResult::Skipped {
        issues += 1;
    }

    // Summary
    println!();
    if issues > 0 {
        if args.check {
            println!(
                "{} issue(s) found. Run '{}' to install.",
                issues,
                style("minotaur setup").cyan()
            );
        } else {
            println!(
                "{}",
                style("Setup incomplete - see above for details.").yellow().bold()
            );
        }
    } else {
        println!(
            "{}",
            style("Setup complete! Run 'minotaur run -- <command>' to start.").green().bold()
        );
    }

    Ok(())
}

async fn setup_linux(args: &SetupArgs) -> MinotaurResult<()> {
    println!("{}", style("Checking prerequisites...").bold());
    println!();

    let mut issues = 0;

    // Step 1: Check/install Podman
    let podman_result = check_native_podman(args).await;
    if podman_result == StepResult::Failed || podman_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 2: Check rootless mode
    let rootless_result = if podman_result == StepResult::AlreadyOk
        || podman_result == StepResult::Installed
    {
        check_rootless_mode(args).await
    } else {
        print_blocked("Rootless Mode", "Podman");
        StepResult::Blocked
    };
    if rootless_result == StepResult::Failed || rootless_result == StepResult::Skipped {
        issues += 1;
    }

    // Step 3: Check user namespace support
    let userns_result = if rootless_result == StepResult::AlreadyOk
        || rootless_result == StepResult::Installed
    {
        check_user_namespaces(args).await
    } else {
        print_blocked("User Namespaces", "Rootless Mode");
        StepResult::Blocked
    };
    if userns_result == StepResult::Failed || userns_result == StepResult::Skipped {
        issues += 1;
    }

    // Summary
    println!();
    if issues > 0 {
        if args.check {
            println!(
                "{} issue(s) found. Run '{}' to install.",
                issues,
                style("minotaur setup").cyan()
            );
        } else {
            println!(
                "{}",
                style("Setup incomplete - see above for details.").yellow().bold()
            );
        }
    } else {
        println!(
            "{}",
            style("Setup complete! Run 'minotaur run -- <command>' to start.").green().bold()
        );
    }

    Ok(())
}

// =============================================================================
// macOS Steps
// =============================================================================

async fn check_homebrew(args: &SetupArgs) -> StepResult {
    println!("  {}", style("Homebrew").bold());

    let output = Command::new("brew")
        .arg("--prefix")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let prefix = String::from_utf8_lossy(&out.stdout);
            println!("  {} Installed ({})", CHECK, prefix.trim());
            StepResult::AlreadyOk
        }
        _ => {
            println!("  {} Not installed", CIRCLE);

            if args.check {
                return StepResult::Failed;
            }

            println!();
            println!("  Homebrew is required to install OrbStack.");
            println!("  Install from: {}", style("https://brew.sh").cyan());
            println!();

            if confirm("Install Homebrew now?", args.yes) {
                println!("  Running Homebrew installer...");
                println!();

                let install_result = run_visible(
                    "/bin/bash",
                    &[
                        "-c",
                        "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)",
                    ],
                )
                .await;

                if install_result {
                    println!("  {} Homebrew installed", CHECK);
                    StepResult::Installed
                } else {
                    println!(
                        "  {} Homebrew installation failed",
                        CROSS
                    );
                    println!(
                        "  Visit {} for manual installation",
                        style("https://brew.sh").cyan()
                    );
                    StepResult::Failed
                }
            } else {
                println!("  Skipped Homebrew installation");
                StepResult::Skipped
            }
        }
    }
}

async fn check_orbstack(args: &SetupArgs) -> StepResult {
    println!();
    println!("  {}", style("OrbStack").bold());

    if OrbStack::is_installed().await {
        if let Ok(version) = OrbStack::version().await {
            println!("  {} Installed ({})", CHECK, version);
        } else {
            println!("  {} Installed", CHECK);
        }
        return StepResult::AlreadyOk;
    }

    println!("  {} Not installed", CIRCLE);

    if args.check {
        return StepResult::Failed;
    }

    if confirm("Install OrbStack via Homebrew?", args.yes) {
        println!("  Running: brew install --cask orbstack");
        println!();

        if run_visible("brew", &["install", "--cask", "orbstack"]).await {
            println!("  {} OrbStack installed", CHECK);
            StepResult::Installed
        } else {
            println!("  {} OrbStack installation failed", CROSS);
            println!(
                "  Download manually from: {}",
                style("https://orbstack.dev").cyan()
            );
            StepResult::Failed
        }
    } else {
        println!("  Skipped OrbStack installation");
        StepResult::Skipped
    }
}

async fn check_orbstack_running(args: &SetupArgs) -> StepResult {
    println!();
    println!("  {}", style("OrbStack Service").bold());

    match OrbStack::is_running().await {
        Ok(true) => {
            println!("  {} Running", CHECK);
            StepResult::AlreadyOk
        }
        Ok(false) => {
            println!("  {} Not running", CIRCLE);

            if args.check {
                return StepResult::Failed;
            }

            println!("  Starting OrbStack...");
            match OrbStack::start().await {
                Ok(()) => {
                    println!("  {} OrbStack started", CHECK);
                    StepResult::Installed
                }
                Err(e) => {
                    println!("  {} Failed to start OrbStack: {}", CROSS, e);
                    println!("  Try starting OrbStack manually from Applications");
                    StepResult::Failed
                }
            }
        }
        Err(e) => {
            println!("  {} Error checking status: {}", CROSS, e);
            StepResult::Failed
        }
    }
}

async fn check_vm(args: &SetupArgs, vm_name: &str, vm_distro: &str) -> StepResult {
    println!();
    println!("  {}", style(format!("Minotaur VM ({})", vm_name)).bold());

    let output = Command::new("orb")
        .args(["list", "-f", "{{.Name}}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    let vm_exists = match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.lines().any(|line| line.trim() == vm_name)
        }
        _ => false,
    };

    if vm_exists {
        println!("  {} Exists", CHECK);
        return StepResult::AlreadyOk;
    }

    println!("  {} Not found", CIRCLE);

    if args.check {
        return StepResult::Failed;
    }

    if confirm(&format!("Create {} VM '{}'?", vm_distro, vm_name), args.yes) {
        println!("  Creating VM...");
        println!();

        if run_visible("orb", &["create", vm_distro, vm_name]).await {
            println!("  {} VM created ({})", CHECK, vm_name);
            StepResult::Installed
        } else {
            println!("  {} VM creation failed", CROSS);
            println!(
                "  Try: orb delete {} && minotaur setup",
                vm_name
            );
            StepResult::Failed
        }
    } else {
        println!("  Skipped VM creation");
        StepResult::Skipped
    }
}

async fn check_podman_in_vm(args: &SetupArgs, vm_name: &str, vm_distro: &str) -> StepResult {
    println!();
    println!("  {}", style("Podman (in VM)").bold());

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
            println!("  {} Installed ({})", CHECK, first_line.trim());
            StepResult::AlreadyOk
        }
        _ => {
            println!("  {} Not installed", CIRCLE);

            if args.check {
                return StepResult::Failed;
            }

            if confirm("Install Podman in VM?", args.yes) {
                println!("  Installing Podman...");
                println!();

                // Determine package manager based on distro
                let install_cmd = match vm_distro {
                    "fedora" | "rhel" | "centos" | "rocky" | "alma" => {
                        vec!["sudo", "dnf", "install", "-y", "podman"]
                    }
                    "ubuntu" | "debian" => {
                        vec!["sudo", "apt-get", "update", "&&", "sudo", "apt-get", "install", "-y", "podman"]
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

                // For apt-based systems, we need to run update first
                if vm_distro == "ubuntu" || vm_distro == "debian" {
                    let update_success = run_visible_orb(vm_name, &["sudo", "apt-get", "update"]).await;
                    if !update_success {
                        println!("  {} Package update failed", CROSS);
                        return StepResult::Failed;
                    }

                    if run_visible_orb(vm_name, &["sudo", "apt-get", "install", "-y", "podman"]).await {
                        println!("  {} Podman installed", CHECK);
                        return StepResult::Installed;
                    }
                } else if run_visible_orb(vm_name, &install_cmd).await {
                    println!("  {} Podman installed", CHECK);
                    return StepResult::Installed;
                }

                println!("  {} Podman installation failed", CROSS);
                StepResult::Failed
            } else {
                println!("  Skipped Podman installation");
                StepResult::Skipped
            }
        }
    }
}

// =============================================================================
// Linux Steps
// =============================================================================

async fn check_native_podman(args: &SetupArgs) -> StepResult {
    println!("  {}", style("Podman").bold());

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
            println!("  {} {}", CHECK, first_line.trim());
            StepResult::AlreadyOk
        }
        _ => {
            println!("  {} Not installed", CIRCLE);

            if args.check {
                return StepResult::Failed;
            }

            let pkg_manager = detect_package_manager().await;
            match pkg_manager {
                Some((name, install_args)) => {
                    if confirm(&format!("Install Podman via {}?", name), args.yes) {
                        println!("  Running: sudo {} {}", name, install_args.join(" "));
                        println!();

                        let mut cmd_args = vec!["install", "-y", "podman"];
                        if name == "pacman" {
                            cmd_args = vec!["-S", "--noconfirm", "podman"];
                        }

                        if run_visible_sudo(name, &cmd_args).await {
                            println!("  {} Podman installed", CHECK);
                            StepResult::Installed
                        } else {
                            println!("  {} Podman installation failed", CROSS);
                            StepResult::Failed
                        }
                    } else {
                        println!("  Skipped Podman installation");
                        StepResult::Skipped
                    }
                }
                None => {
                    println!(
                        "  {} Could not detect package manager",
                        CROSS
                    );
                    println!("  Supported: dnf, apt-get, pacman, zypper");
                    println!("  Install Podman manually for your distribution");
                    StepResult::Failed
                }
            }
        }
    }
}

async fn check_rootless_mode(args: &SetupArgs) -> StepResult {
    println!();
    println!("  {}", style("Rootless Mode").bold());

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
                println!("  {} Enabled", CHECK);
                StepResult::AlreadyOk
            } else {
                println!("  {} Not enabled", CIRCLE);

                if args.check {
                    return StepResult::Failed;
                }

                println!("  Running: podman system migrate");
                if run_visible("podman", &["system", "migrate"]).await {
                    println!("  {} Rootless mode configured", CHECK);
                    StepResult::Installed
                } else {
                    println!("  {} Failed to configure rootless mode", CROSS);
                    StepResult::Failed
                }
            }
        }
        _ => {
            println!("  {} Could not check rootless status", CROSS);
            StepResult::Failed
        }
    }
}

async fn check_user_namespaces(args: &SetupArgs) -> StepResult {
    println!();
    println!("  {}", style("User Namespaces").bold());

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
                println!("  {} Enabled (max: {})", CHECK, max_ns);
                StepResult::AlreadyOk
            } else {
                println!("  {} Disabled (max_user_namespaces = 0)", CIRCLE);

                if args.check {
                    return StepResult::Failed;
                }

                println!();
                println!("  User namespaces must be enabled for rootless containers.");
                println!("  Run the following command:");
                println!();
                println!(
                    "    {}",
                    style("sudo sysctl -w user.max_user_namespaces=15000").cyan()
                );
                println!();
                println!("  To make permanent, add to /etc/sysctl.conf:");
                println!();
                println!(
                    "    {}",
                    style("user.max_user_namespaces=15000").cyan()
                );
                println!();

                StepResult::Failed
            }
        }
        _ => {
            // If we can't read the file, assume it's fine (some distros don't have this)
            println!("  {} Could not check (assuming enabled)", CHECK);
            StepResult::AlreadyOk
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn print_blocked(name: &str, dependency: &str) {
    println!();
    println!("  {}", style(name).bold());
    println!("  {} Blocked (requires {})", DASH, dependency);
}

/// Prompt user for confirmation, respecting --yes flag
fn confirm(prompt: &str, auto_yes: bool) -> bool {
    if auto_yes {
        println!("  {} (auto-approved)", prompt);
        return true;
    }

    print!("  {} [y/N] ", prompt);
    if io::stdout().flush().is_err() {
        return false;
    }

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    input.trim().eq_ignore_ascii_case("y")
}

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
