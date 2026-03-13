//! Exec command - execute a command in a running session

use crate::cli::args::ExecArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::{create_runtime, ContainerRuntime};
use crate::session::{Session, SessionManager, SessionStatus};
use crate::ui::{self, UiContext};
use console::style;
use std::io::IsTerminal;
use tracing::debug;

const DEFAULT_SHELL: &str = "/bin/zsh";

/// Execute the exec command
pub async fn execute(args: ExecArgs, config: &Config) -> MinoResult<()> {
    let ctx = UiContext::detect();
    let manager = SessionManager::new().await?;

    let session = resolve_session(&manager, args.session.as_deref()).await?;

    ui::step_info(
        &ctx,
        &format!("Exec into session {}", style(&session.name).cyan()),
    );

    let command = if args.command.is_empty() {
        vec![DEFAULT_SHELL.to_string()]
    } else {
        args.command
    };

    let runtime = create_runtime(config)?;
    let tty = std::io::stdin().is_terminal();
    let exit_code = exec_in_session(&session, &*runtime, &command, tty).await?;

    debug!(exit_code, "Container exec finished");

    if exit_code != 0 {
        std::process::exit((exit_code & 0xFF) as i32);
    }

    Ok(())
}

/// Resolve which session to exec into.
async fn resolve_session(manager: &SessionManager, name: Option<&str>) -> MinoResult<Session> {
    match name {
        Some(name) => {
            let session = manager
                .get(name)
                .await?
                .ok_or_else(|| MinoError::SessionNotFound(name.to_string()))?;
            validate_session_running(&session)?;
            Ok(session)
        }
        None => {
            let sessions = manager.list().await?;
            find_running_session(sessions)
        }
    }
}

/// Validate that a named session is in Running state.
fn validate_session_running(session: &Session) -> MinoResult<()> {
    if session.status != SessionStatus::Running {
        return Err(MinoError::User(format!(
            "Session '{}' is not running (status: {}). Use 'mino list' to see active sessions.",
            session.name,
            session.status
        )));
    }
    Ok(())
}

/// Find the most recent running session from a list (expected sorted newest-first).
fn find_running_session(sessions: Vec<Session>) -> MinoResult<Session> {
    sessions
        .into_iter()
        .find(|s| s.status == SessionStatus::Running)
        .ok_or(MinoError::NoActiveSessions)
}

/// Execute a command inside the session's container.
async fn exec_in_session(
    session: &Session,
    runtime: &dyn ContainerRuntime,
    command: &[String],
    tty: bool,
) -> MinoResult<i32> {
    let container_id = session
        .container_id
        .as_ref()
        .ok_or_else(|| MinoError::ContainerNotFound(session.name.clone()))?;

    runtime.exec_in_container(container_id, command, tty).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::{test_session, MockResponse, MockRuntime};

    // -- find_running_session tests (pure function) --

    #[test]
    fn find_running_picks_first_running() {
        let sessions = vec![
            test_session("sess-1", SessionStatus::Running, Some("cid-1")),
            test_session("sess-2", SessionStatus::Running, Some("cid-2")),
        ];
        let result = find_running_session(sessions).unwrap();
        assert_eq!(result.name, "sess-1");
    }

    #[test]
    fn find_running_skips_stopped() {
        let sessions = vec![
            test_session("stopped-1", SessionStatus::Stopped, Some("cid-1")),
            test_session("running-1", SessionStatus::Running, Some("cid-2")),
        ];
        let result = find_running_session(sessions).unwrap();
        assert_eq!(result.name, "running-1");
    }

    #[test]
    fn find_running_empty_list() {
        let err = find_running_session(vec![]).unwrap_err();
        assert!(matches!(err, MinoError::NoActiveSessions));
    }

    #[test]
    fn find_running_no_running() {
        let sessions = vec![
            test_session("stopped", SessionStatus::Stopped, None),
            test_session("failed", SessionStatus::Failed, None),
            test_session("starting", SessionStatus::Starting, None),
        ];
        let err = find_running_session(sessions).unwrap_err();
        assert!(matches!(err, MinoError::NoActiveSessions));
    }

    // -- validate_session_running tests (pure function) --

    #[test]
    fn validate_running_accepts_running() {
        let session = test_session("s", SessionStatus::Running, Some("cid"));
        assert!(validate_session_running(&session).is_ok());
    }

    #[test]
    fn validate_running_rejects_stopped() {
        let session = test_session("s", SessionStatus::Stopped, None);
        let err = validate_session_running(&session).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not running"));
        assert!(msg.contains("mino list"));
    }

    #[test]
    fn validate_running_rejects_starting() {
        let session = test_session("s", SessionStatus::Starting, None);
        let err = validate_session_running(&session).unwrap_err();
        assert!(err.to_string().contains("not running"));
    }

    #[test]
    fn validate_running_rejects_failed() {
        let session = test_session("s", SessionStatus::Failed, None);
        let err = validate_session_running(&session).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not running"));
        assert!(msg.contains("failed"));
    }

    // -- exec_in_session tests (MockRuntime) --

    #[tokio::test]
    async fn exec_no_container_id_errors() {
        let session = test_session("s", SessionStatus::Running, None);
        let runtime = MockRuntime::new();
        let cmd = vec!["bash".to_string()];
        let err = exec_in_session(&session, &runtime, &cmd, false)
            .await
            .unwrap_err();
        assert!(matches!(err, MinoError::ContainerNotFound(_)));
        runtime.assert_no_calls();
    }

    #[tokio::test]
    async fn exec_delegates_to_runtime() {
        let session = test_session("s", SessionStatus::Running, Some("abc123"));
        let runtime = MockRuntime::new();
        let cmd = vec!["bash".to_string()];

        let code = exec_in_session(&session, &runtime, &cmd, false)
            .await
            .unwrap();

        assert_eq!(code, 0);
        runtime.assert_called("exec_in_container", 1);
        runtime.assert_called_with("exec_in_container", &["abc123", "false", "bash"]);
    }

    #[tokio::test]
    async fn exec_passes_command_args() {
        let session = test_session("s", SessionStatus::Running, Some("abc123"));
        let runtime = MockRuntime::new();
        let cmd = vec!["ls".to_string(), "-la".to_string(), "/workspace".to_string()];

        exec_in_session(&session, &runtime, &cmd, true)
            .await
            .unwrap();

        runtime.assert_called_with(
            "exec_in_container",
            &["abc123", "true", "ls", "-la", "/workspace"],
        );
    }

    #[tokio::test]
    async fn exec_propagates_exit_code() {
        let session = test_session("s", SessionStatus::Running, Some("abc123"));
        let runtime =
            MockRuntime::new().on("exec_in_container", Ok(MockResponse::Int(42)));
        let cmd = vec!["false".to_string()];

        let code = exec_in_session(&session, &runtime, &cmd, false)
            .await
            .unwrap();
        assert_eq!(code, 42);
    }

    #[tokio::test]
    async fn exec_runtime_error_propagates() {
        let session = test_session("s", SessionStatus::Running, Some("abc123"));
        let runtime = MockRuntime::new().on(
            "exec_in_container",
            Err(MinoError::Internal("test error".to_string())),
        );
        let cmd = vec!["bash".to_string()];

        let err = exec_in_session(&session, &runtime, &cmd, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("test error"));
    }

    #[tokio::test]
    async fn exec_tty_true_forwarded() {
        let session = test_session("s", SessionStatus::Running, Some("cid"));
        let runtime = MockRuntime::new();
        let cmd = vec!["bash".to_string()];

        exec_in_session(&session, &runtime, &cmd, true)
            .await
            .unwrap();

        runtime.assert_called_with("exec_in_container", &["cid", "true", "bash"]);
    }
}
