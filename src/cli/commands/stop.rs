//! Stop command - stop a running session

use crate::cli::args::StopArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::{create_runtime, ContainerRuntime};
use crate::session::{Session, SessionManager, SessionStatus};
use crate::ui::{self, TaskSpinner, UiContext};
use console::style;

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
                format!("{:?}", session.status).to_lowercase()
            ),
        );
        return Ok(());
    }

    // Stop container
    if session.container_id.is_some() {
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

    // Remove container (already tolerates "no such container")
    let _ = runtime.remove(container_id).await;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::{test_session, MockRuntime};

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
}
