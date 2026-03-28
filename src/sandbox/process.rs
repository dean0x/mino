//! Sandbox process handle for managing the lifecycle of a sandboxed process
//!
//! Wraps a `tokio::process::Child` with session metadata and provides
//! typed methods for wait, terminate, and kill operations.

use crate::error::{MinoError, MinoResult};
use std::fmt;
use std::path::PathBuf;
use tokio::process::Child;

/// Handle to a running sandboxed process
///
/// Note: `Debug` is manually implemented because `tokio::process::Child`
/// does not implement `Debug`.
pub struct SandboxProcess {
    child: Child,
    session_id: String,
    log_file: Option<PathBuf>,
}

impl fmt::Debug for SandboxProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxProcess")
            .field("session_id", &self.session_id)
            .field("pid", &self.child.id())
            .field("log_file", &self.log_file)
            .finish()
    }
}

impl SandboxProcess {
    /// Create a new sandbox process handle
    pub fn new(child: Child, session_id: String) -> Self {
        Self {
            child,
            session_id,
            log_file: None,
        }
    }

    /// Set the log file path (for detached mode)
    pub fn with_log_file(mut self, path: PathBuf) -> Self {
        self.log_file = Some(path);
        self
    }

    /// Get the session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the process ID (None if the process has already exited)
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Get the log file path (if detached)
    pub fn log_file(&self) -> Option<&PathBuf> {
        self.log_file.as_ref()
    }

    /// Wait for the process to exit, return exit code
    pub async fn wait(&mut self) -> MinoResult<i32> {
        let status = self.child.wait().await.map_err(|e| {
            MinoError::io(
                format!("waiting for sandbox process (session {})", self.session_id),
                e,
            )
        })?;

        Ok(status.code().unwrap_or(128))
    }

    /// Send SIGTERM to the process
    #[cfg(unix)]
    pub async fn terminate(&mut self) -> MinoResult<()> {
        let pid = self.pid().ok_or_else(|| {
            MinoError::Internal(format!(
                "Cannot terminate sandbox process for session {}: process already exited",
                self.session_id
            ))
        })?;

        // SAFETY: libc::kill sends a signal to a process. We have a valid PID
        // from the child process handle. SIGTERM is a standard termination signal.
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(MinoError::io(
                format!(
                    "sending SIGTERM to PID {} (session {})",
                    pid, self.session_id
                ),
                err,
            ));
        }

        Ok(())
    }

    /// Send SIGTERM to the process (non-Unix stub)
    #[cfg(not(unix))]
    pub async fn terminate(&mut self) -> MinoResult<()> {
        self.kill().await
    }

    /// Send SIGKILL to the process
    pub async fn kill(&mut self) -> MinoResult<()> {
        self.child.kill().await.map_err(|e| {
            MinoError::io(
                format!("killing sandbox process (session {})", self.session_id),
                e,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    fn spawn_true() -> Child {
        Command::new("true")
            .spawn()
            .expect("failed to spawn 'true'")
    }

    fn spawn_sleep() -> Child {
        Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("failed to spawn sleep")
    }

    #[tokio::test]
    async fn accessors_reflect_construction() {
        let process = SandboxProcess::new(spawn_true(), "sess-1".to_string())
            .with_log_file(PathBuf::from("/tmp/test.log"));
        assert_eq!(process.session_id(), "sess-1");
        assert_eq!(process.log_file(), Some(&PathBuf::from("/tmp/test.log")));
    }

    #[tokio::test]
    async fn wait_returns_exit_code() {
        let mut success = SandboxProcess::new(spawn_true(), "ok".to_string());
        assert_eq!(success.wait().await.unwrap(), 0);

        let child = Command::new("false")
            .spawn()
            .expect("failed to spawn 'false'");
        let mut failure = SandboxProcess::new(child, "fail".to_string());
        assert_ne!(failure.wait().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn kill_stops_running_process() {
        let mut process = SandboxProcess::new(spawn_sleep(), "kill".to_string());
        assert!(process.pid().is_some());
        assert!(process.kill().await.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminate_sends_sigterm() {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let mut process = SandboxProcess::new(child, "test-terminate".to_string());
        process.terminate().await.unwrap();
        let status = process.wait().await.unwrap();
        assert_ne!(status, 0);
    }
}
