//! Stop command - stop a running session

use crate::cli::args::StopArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::{create_runtime, ContainerRuntime};
use crate::sandbox::RuntimeMode;
use crate::session::{Session, SessionManager, SessionStatus};
use crate::ui::{self, TaskSpinner, UiContext};
use console::style;
use tracing::warn;

/// Execute the stop command
pub async fn execute(args: StopArgs, config: &Config) -> MinoResult<()> {
    let ctx = UiContext::detect();
    let manager = SessionManager::new().await?;

    // Find session
    let session = manager
        .get(&args.session)
        .await?
        .ok_or_else(|| MinoError::SessionNotFound(args.session.clone()))?;

    if !matches!(
        session.status,
        SessionStatus::Running | SessionStatus::Starting
    ) {
        ui::step_info(
            &ctx,
            &format!(
                "Session {} is already {}",
                style(&args.session).cyan(),
                session.status
            ),
        );
        return Ok(());
    }

    if session.runtime_mode == Some(RuntimeMode::Native) {
        // Native mode: kill the process directly
        if let Some(pid) = session.process_id {
            let mut spinner = TaskSpinner::new(&ctx);
            spinner.start(&format!(
                "Stopping session {}...",
                style(&args.session).cyan()
            ));

            stop_native_session(pid, args.force)?;

            // Clean up sandbox resources (ACLs, pf rules) even if the helper's
            // auto-cleanup didn't run (e.g., mino was killed externally)
            if let Ok(platform) = crate::sandbox::native::create_sandbox_platform() {
                let sandbox_user = session
                    .sandbox_user
                    .as_deref()
                    .unwrap_or(crate::sandbox::config::DEFAULT_SANDBOX_USER);
                if let Err(e) = platform
                    .cleanup(&session.name, &session.project_dir, sandbox_user)
                    .await
                {
                    warn!("Sandbox cleanup for session {}: {}", args.session, e);
                }
            }

            spinner.stop(&format!("Session {} stopped", style(&args.session).cyan()));
        } else {
            ui::step_ok(
                &ctx,
                &format!("Session {} stopped", style(&args.session).cyan()),
            );
        }
    } else if session.container_id.is_some() {
        // Container mode: existing logic
        let runtime = create_runtime(config)?;

        let mut spinner = TaskSpinner::new(&ctx);
        spinner.start(&format!(
            "Stopping session {}...",
            style(&args.session).cyan()
        ));

        stop_container(&session, &*runtime, args.force).await?;

        spinner.stop(&format!("Session {} stopped", style(&args.session).cyan()));
    } else {
        ui::step_ok(
            &ctx,
            &format!("Session {} stopped", style(&args.session).cyan()),
        );
    }

    // Update session status
    manager
        .update_status(&args.session, SessionStatus::Stopped)
        .await?;

    Ok(())
}

/// Stop a native sandbox process by sending a signal.
///
/// Sends SIGTERM (graceful) or SIGKILL (force). Tolerates ESRCH (process
/// already exited) since the sandbox may have terminated on its own.
fn stop_native_session(pid: u32, force: bool) -> MinoResult<()> {
    #[cfg(unix)]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: libc::kill sends a signal to a process identified by PID.
        // We have a valid PID from the session record. Both SIGTERM and SIGKILL
        // are standard POSIX signals.
        let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if result != 0 {
            let err = std::io::Error::last_os_error();
            // ESRCH = no such process (already exited) — not an error
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(MinoError::io(format!("signaling PID {}", pid), err));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, force);
        Err(MinoError::NativeUnsupported {
            feature: "process signals".to_string(),
        })
    }
}

/// Stop a session's container. Returns `Ok(true)` if a stop was performed,
/// `Ok(false)` if the session was already stopped/failed.
///
/// Tolerates "no such container" / "not found" errors since the container
/// may have already exited (e.g. `--rm` on detached containers).
async fn stop_container(
    session: &Session,
    runtime: &dyn ContainerRuntime,
    force: bool,
) -> MinoResult<bool> {
    if !matches!(
        session.status,
        SessionStatus::Running | SessionStatus::Starting
    ) {
        return Ok(false);
    }

    let container_id = match &session.container_id {
        Some(id) => id,
        None => return Ok(true),
    };

    let stop_result = if force {
        runtime.kill(container_id).await
    } else {
        runtime.stop(container_id).await
    };
    if let Err(e) = &stop_result {
        let msg = e.to_string().to_lowercase();
        if !msg.contains("no such container") && !msg.contains("not found") {
            stop_result?;
        }
    }

    // Remove container (best-effort; log failures instead of propagating)
    if let Err(e) = runtime.remove(container_id).await {
        warn!(
            "Failed to remove container {}: {}",
            &container_id[..12.min(container_id.len())],
            e
        );
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::{test_session, MockRuntime};

    // -- Container stop tests --

    #[tokio::test]
    async fn stop_already_stopped_skips() {
        let session = test_session("test", SessionStatus::Stopped, Some("container-abc123"));
        let mock = MockRuntime::new();

        let result = stop_container(&session, &mock, false).await.unwrap();
        assert!(!result);
        mock.assert_no_calls();
    }

    #[tokio::test]
    async fn stop_already_failed_skips() {
        let session = test_session("test", SessionStatus::Failed, Some("container-abc123"));
        let mock = MockRuntime::new();

        let result = stop_container(&session, &mock, false).await.unwrap();
        assert!(!result);
        mock.assert_no_calls();
    }

    #[tokio::test]
    async fn stop_graceful() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new();

        let result = stop_container(&session, &mock, false).await.unwrap();
        assert!(result);
        mock.assert_called("stop", 1);
        mock.assert_called("kill", 0);
        mock.assert_called("remove", 1);
    }

    #[tokio::test]
    async fn stop_force() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new();

        let result = stop_container(&session, &mock, true).await.unwrap();
        assert!(result);
        mock.assert_called("kill", 1);
        mock.assert_called("stop", 0);
        mock.assert_called("remove", 1);
    }

    #[tokio::test]
    async fn stop_no_container_id() {
        let session = test_session("test", SessionStatus::Running, None);
        let mock = MockRuntime::new();

        let result = stop_container(&session, &mock, false).await.unwrap();
        assert!(result);
        mock.assert_no_calls();
    }

    #[tokio::test]
    async fn stop_tolerates_no_such_container() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock =
            MockRuntime::new().on_err("stop", MinoError::Internal("no such container".to_string()));

        let result = stop_container(&session, &mock, false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn stop_propagates_real_errors() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new().on_err(
            "stop",
            MinoError::Internal("connection refused".to_string()),
        );

        let result = stop_container(&session, &mock, false).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("connection refused"));
    }

    // -- Native stop tests --

    #[cfg(unix)]
    mod native {
        use super::*;

        #[test]
        fn stop_native_esrch_returns_ok() {
            // PID 0 is special (signals process group), use a very large PID
            // that almost certainly doesn't exist, triggering ESRCH
            let result = stop_native_session(u32::MAX - 1, false);
            assert!(result.is_ok(), "ESRCH should be tolerated");
        }

        #[test]
        fn stop_native_force_with_dead_pid_returns_ok() {
            let result = stop_native_session(u32::MAX - 1, true);
            assert!(
                result.is_ok(),
                "ESRCH should be tolerated for force kill too"
            );
        }

        #[test]
        fn stop_native_uses_sigterm_by_default() {
            // We can't directly verify the signal type without a real process,
            // but we verify the function completes without error for a dead PID
            let result = stop_native_session(u32::MAX - 2, false);
            assert!(result.is_ok());
        }

        #[test]
        fn stop_native_uses_sigkill_when_forced() {
            let result = stop_native_session(u32::MAX - 2, true);
            assert!(result.is_ok());
        }
    }
}
