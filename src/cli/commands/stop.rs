//! Stop command - stop a running session

use crate::cli::args::StopArgs;
use crate::config::Config;
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::create_runtime;
use crate::session::{SessionManager, SessionStatus};
use crate::ui::{self, TaskSpinner, UiContext};
use console::style;

/// Execute the stop command
pub async fn execute(args: StopArgs, config: &Config) -> MinotaurResult<()> {
    let ctx = UiContext::detect();
    let manager = SessionManager::new().await?;

    // Find session
    let session = manager
        .get(&args.session)
        .await?
        .ok_or_else(|| MinotaurError::SessionNotFound(args.session.clone()))?;

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
        spinner.start(&format!("Stopping session {}...", style(&args.session).cyan()));

        if args.force {
            runtime.kill(container_id).await?;
        } else {
            runtime.stop(container_id).await?;
        }

        // Remove container
        runtime.remove(container_id).await?;

        spinner.stop(&format!(
            "Session {} stopped",
            style(&args.session).cyan()
        ));
    } else {
        ui::step_ok(&ctx, &format!("Session {} stopped", style(&args.session).cyan()));
    }

    // Update session status
    manager
        .update_status(&args.session, SessionStatus::Stopped)
        .await?;

    Ok(())
}
