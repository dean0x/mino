//! Status command - check system health and dependencies

use crate::config::Config;
use crate::error::MinoResult;
use crate::orchestration::{create_runtime, OrbStack, Platform};
use crate::session::{Session, SessionStatus};
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

/// Execute the status command
pub async fn execute(config: &Config) -> MinoResult<()> {
    let ctx = UiContext::detect();

    ui::intro(&ctx, "Mino System Status");

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
            ui::step_error(
                &ctx,
                "Unsupported platform - Mino supports macOS and Linux only",
            );
            all_ok = false;
        }
    }

    // Check native sandbox
    ui::section(&ctx, "Native Sandbox");
    check_native_sandbox_status(&ctx, &platform).await;

    // Check cloud CLIs
    ui::section(&ctx, "Cloud CLIs");
    check_cli(&ctx, "aws", "aws --version", "brew install awscli").await;
    check_cli(
        &ctx,
        "gcloud",
        "gcloud --version",
        "brew install google-cloud-sdk",
    )
    .await;
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
                    "Run: mino setup (will auto-install)",
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
            ui::step_warn_hint(
                ctx,
                &format!("{} not found", name),
                &format!("Install: {}", install_hint),
            );
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

/// Check native sandbox prerequisites and stale sessions.
async fn check_native_sandbox_status(ctx: &UiContext, platform: &Platform) {
    match platform {
        Platform::MacOS => check_native_sandbox_macos(ctx).await,
        Platform::Linux => check_native_sandbox_linux(ctx).await,
        Platform::Unsupported => {}
    }

    check_stale_native_sessions(ctx).await;
}

async fn check_native_sandbox_macos(ctx: &UiContext) {
    // Check _mino_agent user exists
    let user_exists = Command::new("dscl")
        .args([".", "-read", "/Users/_mino_agent"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if user_exists {
        ui::step_ok(ctx, "Sandbox user (_mino_agent)");
    } else {
        ui::step_info(
            ctx,
            "Sandbox user not configured (run: mino setup --native)",
        );
    }

    // Check helper binary
    let helper_exists = Command::new("which")
        .arg("mino-sandbox-helper")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if helper_exists {
        ui::step_ok(ctx, "Helper binary installed");
    } else {
        ui::step_info(
            ctx,
            "Helper binary not installed (run: mino setup --native)",
        );
    }

    // Check sudoers
    let sudoers_exists = std::path::Path::new("/etc/sudoers.d/mino").exists();
    if sudoers_exists {
        ui::step_ok(ctx, "Sudoers configured");
    } else {
        ui::step_info(ctx, "Sudoers not configured (run: mino setup --native)");
    }
}

async fn check_native_sandbox_linux(ctx: &UiContext) {
    // Check user namespaces
    let userns_output = Command::new("cat")
        .arg("/proc/sys/user/max_user_namespaces")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match userns_output {
        Ok(output) if output.status.success() => {
            let val: u32 = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .unwrap_or(0);
            if val > 0 {
                ui::step_ok_detail(ctx, "User namespaces enabled", &format!("max: {}", val));
            } else {
                ui::step_warn(ctx, "User namespaces disabled");
            }
        }
        _ => {
            ui::step_ok(ctx, "User namespaces (could not check, assuming enabled)");
        }
    }

    // Check unshare binary
    let unshare_exists = Command::new("which")
        .arg("unshare")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if unshare_exists {
        ui::step_ok(ctx, "unshare binary available");
    } else {
        ui::step_warn(ctx, "unshare not found (install util-linux)");
    }
}

/// Check for stale native sessions where the PID is no longer alive.
async fn check_stale_native_sessions(ctx: &UiContext) {
    if let Ok(sessions) = Session::list_all().await {
        let stale_count = count_stale_native_sessions(&sessions);
        if stale_count > 0 {
            ui::step_warn(
                ctx,
                &format!(
                    "{} stale native session(s) detected. Clean up with: mino list --all",
                    stale_count
                ),
            );
        }
    }
}

/// Count native sessions that appear active but whose PID is no longer alive.
fn count_stale_native_sessions(sessions: &[Session]) -> usize {
    sessions
        .iter()
        .filter(|s| is_stale_native_session(s))
        .count()
}

/// Check if a native session is stale (PID dead but status active).
pub(crate) fn is_stale_native_session(session: &Session) -> bool {
    session.runtime_mode.as_deref() == Some("native")
        && matches!(
            session.status,
            SessionStatus::Running | SessionStatus::Starting
        )
        && !is_pid_alive(session.process_id)
}

/// Clean up stale native sessions — mark them as Failed.
///
/// Returns the number of sessions cleaned up. On macOS, also attempts
/// to remove ACLs and pf rules for the stale session.
pub async fn cleanup_stale_native_sessions() -> crate::error::MinoResult<usize> {
    let sessions = Session::list_all().await?;
    let mut cleaned = 0;

    for session in &sessions {
        if !is_stale_native_session(session) {
            continue;
        }

        tracing::debug!(
            "Cleaning up stale native session: {} (pid: {:?})",
            session.name,
            session.process_id
        );

        // On macOS, attempt to clean up ACLs and pf rules via the helper
        #[cfg(target_os = "macos")]
        {
            if let Err(e) =
                crate::sandbox::macos::cleanup_macos_sandbox(&session.name, &session.project_dir)
                    .await
            {
                tracing::warn!(
                    "Failed to clean up macOS sandbox for session {}: {}",
                    session.name,
                    e
                );
                // Continue anyway — mark session as Failed regardless
            }
        }

        // Update session status to Failed
        let manager = crate::session::SessionManager::new().await?;
        manager
            .update_status(&session.name, SessionStatus::Failed)
            .await?;
        cleaned += 1;
    }

    Ok(cleaned)
}

/// Check if a process with the given PID is still alive.
pub(crate) fn is_pid_alive(pid: Option<u32>) -> bool {
    #[cfg(unix)]
    match pid {
        Some(0) => {
            // PID 0 is the kernel scheduler process — never signal it.
            // A session with PID 0 is definitely stale.
            false
        }
        // SAFETY: libc::kill with signal 0 does not send any signal — it only
        // checks whether the process exists and we have permission to signal it.
        Some(p) => unsafe { libc::kill(p as i32, 0) == 0 },
        None => false,
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::test_session;

    #[test]
    fn is_pid_alive_none_returns_false() {
        assert!(!is_pid_alive(None));
    }

    #[test]
    fn is_pid_alive_dead_pid_returns_false() {
        // Use a very large PID unlikely to exist
        assert!(!is_pid_alive(Some(u32::MAX - 1)));
    }

    #[cfg(unix)]
    #[test]
    fn is_pid_alive_own_pid_returns_true() {
        assert!(is_pid_alive(Some(std::process::id())));
    }

    #[test]
    fn is_pid_alive_pid_zero_returns_false() {
        // PID 0 is the kernel scheduler — should never be considered alive
        // for sandbox session purposes
        assert!(!is_pid_alive(Some(0)));
    }

    #[test]
    fn count_stale_no_native_sessions() {
        let sessions = vec![
            test_session("s1", SessionStatus::Running, Some("c1")),
            test_session("s2", SessionStatus::Stopped, Some("c2")),
        ];
        assert_eq!(count_stale_native_sessions(&sessions), 0);
    }

    #[test]
    fn count_stale_native_running_dead_pid() {
        let mut session = test_session("s1", SessionStatus::Running, None);
        session.runtime_mode = Some("native".to_string());
        session.process_id = Some(u32::MAX - 1); // dead PID
        assert_eq!(count_stale_native_sessions(&[session]), 1);
    }

    #[test]
    fn count_stale_native_stopped_ignored() {
        let mut session = test_session("s1", SessionStatus::Stopped, None);
        session.runtime_mode = Some("native".to_string());
        session.process_id = Some(u32::MAX - 1);
        assert_eq!(count_stale_native_sessions(&[session]), 0);
    }

    #[test]
    fn count_stale_native_no_pid_is_stale() {
        let mut session = test_session("s1", SessionStatus::Running, None);
        session.runtime_mode = Some("native".to_string());
        // process_id is None — is_pid_alive(None) returns false -> stale
        assert_eq!(count_stale_native_sessions(&[session]), 1);
    }

    // ---- is_stale_native_session tests ----

    #[test]
    fn is_stale_container_session_returns_false() {
        let session = test_session("s1", SessionStatus::Running, Some("c1"));
        // Container sessions are not native, so never stale in native sense
        assert!(!is_stale_native_session(&session));
    }

    #[test]
    fn is_stale_native_starting_dead_pid() {
        let mut session = test_session("s1", SessionStatus::Starting, None);
        session.runtime_mode = Some("native".to_string());
        session.process_id = Some(u32::MAX - 1);
        assert!(is_stale_native_session(&session));
    }

    #[test]
    fn is_stale_native_failed_is_not_stale() {
        let mut session = test_session("s1", SessionStatus::Failed, None);
        session.runtime_mode = Some("native".to_string());
        session.process_id = Some(u32::MAX - 1);
        assert!(!is_stale_native_session(&session));
    }

    #[cfg(unix)]
    #[test]
    fn is_stale_native_running_live_pid_is_not_stale() {
        let mut session = test_session("s1", SessionStatus::Running, None);
        session.runtime_mode = Some("native".to_string());
        session.process_id = Some(std::process::id()); // our own PID is alive
        assert!(!is_stale_native_session(&session));
    }
}
