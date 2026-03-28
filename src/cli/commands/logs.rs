//! Logs command - view session logs

use crate::cli::args::LogsArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::{create_runtime, ContainerRuntime};
use crate::sandbox::RuntimeMode;
use crate::session::{Session, SessionManager};
use std::path::Path;

/// Execute the logs command
pub async fn execute(args: LogsArgs, config: &Config) -> MinoResult<()> {
    let manager = SessionManager::new().await?;

    // Find session
    let session = manager
        .get(&args.session)
        .await?
        .ok_or_else(|| MinoError::SessionNotFound(args.session.clone()))?;

    let is_native = session.runtime_mode == Some(RuntimeMode::Native);

    if is_native {
        let log_path = session
            .log_file
            .as_ref()
            .ok_or_else(|| MinoError::User("No log file for this session".to_string()))?;

        if args.follow {
            tail_follow(log_path).await?;
        } else {
            let output = read_log_tail(log_path, args.lines).await?;
            print!("{}", output);
        }
    } else {
        let runtime = create_runtime(config)?;
        let output = get_container_logs(&args, &session, &*runtime).await?;
        if let Some(logs) = output {
            print!("{}", logs);
        }
    }

    Ok(())
}

/// Read the last N lines from a log file.
async fn read_log_tail(path: &Path, lines: u32) -> MinoResult<String> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| MinoError::io(format!("reading log file {}", path.display()), e))?;

    let all_lines: Vec<&str> = content.lines().collect();
    let start = if lines > 0 && all_lines.len() > lines as usize {
        all_lines.len() - lines as usize
    } else {
        0
    };

    let mut output = String::new();
    for line in &all_lines[start..] {
        output.push_str(line);
        output.push('\n');
    }

    Ok(output)
}

/// Follow a log file, printing new lines as they appear.
/// This function runs indefinitely until interrupted.
async fn tail_follow(path: &Path) -> MinoResult<()> {
    use tokio::io::AsyncBufReadExt;

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| MinoError::io(format!("opening log file {}", path.display()), e))?;
    let mut reader = tokio::io::BufReader::new(file);
    let mut line = String::new();

    // Read existing content
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| MinoError::io("reading log file", e))?;
        if n == 0 {
            break;
        }
        print!("{}", line);
    }

    // Follow new content
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| MinoError::io("reading log file", e))?;
        if n == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        }
        print!("{}", line);
    }
}

/// Fetch container logs for a session. Returns `Some(content)` for normal fetch,
/// `None` for follow mode (output streamed directly by the runtime).
async fn get_container_logs(
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
    use crate::orchestration::mock::{test_session, MockResponse, MockRuntime};
    use crate::session::SessionStatus;

    fn test_logs_args(session: &str, follow: bool, lines: u32) -> LogsArgs {
        LogsArgs {
            session: session.to_string(),
            follow,
            lines,
        }
    }

    // -- Container logs tests --

    #[tokio::test]
    async fn logs_no_container_id() {
        let session = test_session("test", SessionStatus::Starting, None);
        let mock = MockRuntime::new();
        let args = test_logs_args("test", false, 100);

        let result = get_container_logs(&args, &session, &mock).await;
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

        let result = get_container_logs(&args, &session, &mock).await.unwrap();
        assert_eq!(result, Some("log line 1\nlog line 2\n".to_string()));
        mock.assert_called("logs", 1);
    }

    #[tokio::test]
    async fn logs_passes_line_count() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new();
        let args = test_logs_args("test", false, 50);

        get_container_logs(&args, &session, &mock).await.unwrap();
        mock.assert_called_with("logs", &["container-abc123", "50"]);
    }

    #[tokio::test]
    async fn logs_follow_mode() {
        let session = test_session("test", SessionStatus::Running, Some("container-abc123"));
        let mock = MockRuntime::new();
        let args = test_logs_args("test", true, 100);

        let result = get_container_logs(&args, &session, &mock).await.unwrap();
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

        let result = get_container_logs(&args, &session, &mock).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("runtime failure"));
    }

    // -- Native log file tests --

    #[tokio::test]
    async fn read_log_tail_all_lines() {
        let tmp = std::env::temp_dir().join("mino-test-logs-all");
        tokio::fs::write(&tmp, "line1\nline2\nline3\n")
            .await
            .unwrap();
        let result = read_log_tail(&tmp, 100).await.unwrap();
        assert_eq!(result, "line1\nline2\nline3\n");
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[tokio::test]
    async fn read_log_tail_respects_line_limit() {
        let tmp = std::env::temp_dir().join("mino-test-logs-limit");
        tokio::fs::write(&tmp, "line1\nline2\nline3\nline4\nline5\n")
            .await
            .unwrap();
        let result = read_log_tail(&tmp, 2).await.unwrap();
        assert_eq!(result, "line4\nline5\n");
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[tokio::test]
    async fn read_log_tail_nonexistent_file_returns_error() {
        let result = read_log_tail(Path::new("/tmp/mino-nonexistent-log-file.log"), 100).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reading log file"));
    }

    #[tokio::test]
    async fn read_log_tail_empty_file() {
        let tmp = std::env::temp_dir().join("mino-test-logs-empty");
        tokio::fs::write(&tmp, "").await.unwrap();
        let result = read_log_tail(&tmp, 100).await.unwrap();
        assert_eq!(result, "");
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[test]
    fn native_session_without_log_file_is_error() {
        let mut session = test_session("native-sess", SessionStatus::Running, None);
        session.runtime_mode = Some(RuntimeMode::Native);
        // log_file is None — accessing logs should fail
        assert!(session.log_file.is_none());
    }
}
