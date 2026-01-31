//! Logs command - view session logs

use crate::cli::args::LogsArgs;
use crate::config::Config;
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::create_runtime;
use crate::session::SessionManager;

/// Execute the logs command
pub async fn execute(args: LogsArgs, config: &Config) -> MinotaurResult<()> {
    let manager = SessionManager::new().await?;

    // Find session
    let session = manager
        .get(&args.session)
        .await?
        .ok_or_else(|| MinotaurError::SessionNotFound(args.session.clone()))?;

    let container_id = session
        .container_id
        .as_ref()
        .ok_or_else(|| MinotaurError::ContainerNotFound(args.session.clone()))?;

    let runtime = create_runtime(config)?;

    if args.follow {
        runtime.logs_follow(container_id).await?;
    } else {
        let logs = runtime.logs(container_id, args.lines).await?;
        print!("{}", logs);
    }

    Ok(())
}
