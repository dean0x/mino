//! Logs command - view session logs

use crate::cli::args::LogsArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::{create_runtime, ContainerRuntime};
use crate::session::{Session, SessionManager};

/// Execute the logs command
pub async fn execute(args: LogsArgs, config: &Config) -> MinoResult<()> {
    let manager = SessionManager::new().await?;

    // Find session
    let session = manager
        .get(&args.session)
        .await?
        .ok_or_else(|| MinoError::SessionNotFound(args.session.clone()))?;

    let runtime = create_runtime(config)?;
    let output = get_logs(&args, &session, &*runtime).await?;
    if let Some(logs) = output {
        print!("{}", logs);
    }

    Ok(())
}

/// Fetch logs for a session. Returns `Some(content)` for normal fetch,
/// `None` for follow mode (output streamed directly by the runtime).
async fn get_logs(
    args: &LogsArgs,
    session: &Session,
    runtime: &dyn ContainerRuntime,
) -> MinoResult<Option<String>> {
    let container_id = session
        .container_id
        .as_ref()
        .ok_or_else(|| MinoError::ContainerNotFound(session.name.clone()))?;

    if args.follow {
        runtime.logs_follow(container_id).await?;
        Ok(None)
    } else {
        let logs = runtime.logs(container_id, args.lines).await?;
        Ok(Some(logs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::LogsArgs;
    use crate::orchestration::mock::{test_session, MockResponse, MockRuntime};
    use crate::session::SessionStatus;

    fn test_logs_args(session: &str, follow: bool, lines: u32) -> LogsArgs {
        LogsArgs {
            session: session.to_string(),
            follow,
            lines,
        }
    }

    #[tokio::test]
    async fn logs_no_container_id() {
        let session = test_session("test", SessionStatus::Starting, None);
        let mock = MockRuntime::new();
        let args = test_logs_args("test", false, 100);

        let result = get_logs(&args, &session, &mock).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("test"), "error should mention session name");
        mock.assert_no_calls();
    }

    #[tokio::test]
    async fn logs_returns_output() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new().on(
            "logs",
            Ok(MockResponse::String("log line 1\nlog line 2\n".to_string())),
        );
        let args = test_logs_args("test", false, 100);

        let result = get_logs(&args, &session, &mock).await.unwrap();
        assert_eq!(result, Some("log line 1\nlog line 2\n".to_string()));
        mock.assert_called("logs", 1);
    }

    #[tokio::test]
    async fn logs_passes_line_count() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new();
        let args = test_logs_args("test", false, 50);

        get_logs(&args, &session, &mock).await.unwrap();
        mock.assert_called_with("logs", &["container-abc123", "50"]);
    }

    #[tokio::test]
    async fn logs_follow_mode() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new();
        let args = test_logs_args("test", true, 100);

        let result = get_logs(&args, &session, &mock).await.unwrap();
        assert_eq!(result, None);
        mock.assert_called("logs_follow", 1);
        mock.assert_called("logs", 0);
    }

    #[tokio::test]
    async fn logs_runtime_error_propagates() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock =
            MockRuntime::new().on_err("logs", MinoError::Internal("runtime failure".to_string()));
        let args = test_logs_args("test", false, 100);

        let result = get_logs(&args, &session, &mock).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("runtime failure"));
    }
}
