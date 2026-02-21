//! Stop command - stop a running session

use crate::cli::args::StopArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::create_runtime;
use crate::session::{SessionManager, SessionStatus};
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
    if let Some(container_id) = &session.container_id {
        let runtime = create_runtime(config)?;

        let mut spinner = TaskSpinner::new(&ctx);
        spinner.start(&format!(
            "Stopping session {}...",
            style(&args.session).cyan()
        ));

        // With --rm on detached containers, the container may already be gone
        // when the user runs `mino stop`. Treat "no such container" as success.
        let stop_result = if args.force {
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
