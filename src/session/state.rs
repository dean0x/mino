//! Session state persistence

use crate::config::ConfigManager;
use crate::error::{MinoError, MinoResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

/// Session status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Starting,
    Running,
    Stopped,
    Failed,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Session record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique session ID
    pub id: Uuid,

    /// Human-readable session name
    pub name: String,

    /// Project directory mounted in container
    pub project_dir: PathBuf,

    /// Command being executed
    pub command: Vec<String>,

    /// Container ID (once started)
    pub container_id: Option<String>,

    /// Current status
    pub status: SessionStatus,

    /// When session was created
    pub created_at: DateTime<Utc>,

    /// When session was last updated
    pub updated_at: DateTime<Utc>,

    /// Cloud providers enabled
    pub cloud_providers: Vec<String>,

    /// Home volume name (if mounted)
    #[serde(default)]
    pub home_volume: Option<String>,

    /// Runtime mode used for this session ("container" or "native")
    #[serde(default)]
    pub runtime_mode: Option<String>,

    /// Native mode: PID of sandboxed process
    #[serde(default)]
    pub process_id: Option<u32>,

    /// Native detached: path to log file
    #[serde(default)]
    pub log_file: Option<PathBuf>,

    /// Native mode: sandbox user name (for exec dispatch)
    #[serde(default)]
    pub sandbox_user: Option<String>,
}

impl Session {
    /// Create a new session
    pub fn new(
        name: String,
        project_dir: PathBuf,
        command: Vec<String>,
        status: SessionStatus,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            project_dir,
            command,
            container_id: None,
            status,
            created_at: now,
            updated_at: now,
            cloud_providers: vec![],
            home_volume: None,
            runtime_mode: None,
            process_id: None,
            log_file: None,
            sandbox_user: None,
        }
    }

    /// Get session file path
    pub fn file_path(&self) -> PathBuf {
        ConfigManager::sessions_dir().join(format!("{}.json", self.name))
    }

    /// Load session from file
    pub async fn load(name: &str) -> MinoResult<Option<Self>> {
        validate_session_name(name)?;
        let path = ConfigManager::sessions_dir().join(format!("{}.json", name));

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| MinoError::io(format!("reading session file {}", path.display()), e))?;

        let session: Session = serde_json::from_str(&content)?;
        Ok(Some(session))
    }

    /// Create session file atomically — fails if file already exists.
    /// Uses O_CREAT | O_EXCL for kernel-level atomic create-or-fail,
    /// eliminating the TOCTOU race in load-then-save.
    ///
    /// All file I/O runs in a single `spawn_blocking` call so that open, write,
    /// and close complete synchronously before `.await` resolves — preventing a
    /// race where a subsequent read sees an empty file because the async drop of
    /// `tokio::fs::File` defers `close()` to a background task.
    pub async fn create_file(&self) -> MinoResult<()> {
        validate_session_name(&self.name)?;
        let path = self.file_path();

        let content = serde_json::to_string_pretty(self)?;
        let session_name = self.name.clone();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| MinoError::io("creating sessions directory", e))?;
        }

        match tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        MinoError::SessionExists(session_name)
                    } else {
                        MinoError::io(format!("creating session file {}", path.display()), e)
                    }
                })?;
            file.write_all(content.as_bytes())
                .map_err(|e| MinoError::io(format!("writing session file {}", path.display()), e))
        })
        .await
        {
            Ok(result) => result,
            Err(e) => Err(MinoError::Internal(format!(
                "session create task failed: {}",
                e
            ))),
        }
    }

    /// Save session to file (overwrites existing). Use for status updates.
    pub async fn save(&self) -> MinoResult<()> {
        let path = self.file_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| MinoError::io("creating sessions directory", e))?;
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)
            .await
            .map_err(|e| MinoError::io(format!("writing session file {}", path.display()), e))?;

        Ok(())
    }

    /// Delete session file
    pub async fn delete(&self) -> MinoResult<()> {
        let path = self.file_path();
        if path.exists() {
            fs::remove_file(&path).await.map_err(|e| {
                MinoError::io(format!("deleting session file {}", path.display()), e)
            })?;
        }
        Ok(())
    }

    /// List all sessions
    pub async fn list_all() -> MinoResult<Vec<Session>> {
        let sessions_dir = ConfigManager::sessions_dir();

        if !sessions_dir.exists() {
            return Ok(vec![]);
        }

        let mut sessions = vec![];
        let mut entries = fs::read_dir(&sessions_dir)
            .await
            .map_err(|e| MinoError::io("reading sessions directory", e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| MinoError::io("reading session entry", e))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let content = fs::read_to_string(&path).await.ok();
                if let Some(content) = content {
                    if let Ok(session) = serde_json::from_str::<Session>(&content) {
                        sessions.push(session);
                    }
                }
            }
        }

        // Sort by creation time, newest first
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(sessions)
    }
}

/// Validate that a session name is safe (no path traversal, no special characters).
pub fn validate_session_name(name: &str) -> MinoResult<()> {
    if name.is_empty() {
        return Err(MinoError::User("Session name cannot be empty".to_string()));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
        return Err(MinoError::User(format!(
            "Invalid session name '{}': must not contain path separators or '..'",
            name
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(MinoError::User(format!(
            "Invalid session name '{}': must contain only alphanumeric characters, hyphens, or underscores",
            name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_new() {
        let session = Session::new(
            "test-session".to_string(),
            PathBuf::from("/project"),
            vec!["bash".to_string()],
            SessionStatus::Starting,
        );

        assert_eq!(session.name, "test-session");
        assert_eq!(session.status, SessionStatus::Starting);
        assert!(session.container_id.is_none());
    }

    #[test]
    fn session_serialize() {
        let session = Session::new(
            "test-session".to_string(),
            PathBuf::from("/project"),
            vec!["bash".to_string()],
            SessionStatus::Running,
        );

        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("test-session"));
        assert!(json.contains("running"));

        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, session.name);
    }

    // -- validate_session_name tests --

    #[test]
    fn valid_session_names() {
        assert!(validate_session_name("my-session").is_ok());
        assert!(validate_session_name("session_1").is_ok());
        assert!(validate_session_name("abc123").is_ok());
    }

    #[test]
    fn rejects_empty_name() {
        let err = validate_session_name("").unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_session_name("../../../etc/passwd").is_err());
        assert!(validate_session_name("..").is_err());
        assert!(validate_session_name("foo/bar").is_err());
        assert!(validate_session_name("foo\\bar").is_err());
    }

    #[test]
    fn rejects_null_byte() {
        assert!(validate_session_name("foo\0bar").is_err());
    }

    #[test]
    fn rejects_special_characters() {
        assert!(validate_session_name("foo bar").is_err());
        assert!(validate_session_name("foo.bar").is_err());
        assert!(validate_session_name("foo@bar").is_err());
    }

    #[test]
    fn session_serialize_with_runtime_mode() {
        let mut session = Session::new(
            "test-session".to_string(),
            PathBuf::from("/project"),
            vec!["bash".to_string()],
            SessionStatus::Running,
        );
        session.runtime_mode = Some("native".to_string());
        session.process_id = Some(12345);
        session.log_file = Some(PathBuf::from("/tmp/mino-session.log"));
        session.sandbox_user = Some("_mino_agent".to_string());

        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"runtime_mode\":\"native\""));
        assert!(json.contains("\"process_id\":12345"));
        assert!(json.contains("mino-session.log"));
        assert!(json.contains("\"sandbox_user\":\"_mino_agent\""));

        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.runtime_mode.as_deref(), Some("native"));
        assert_eq!(parsed.process_id, Some(12345));
        assert!(parsed.log_file.is_some());
        assert_eq!(parsed.sandbox_user.as_deref(), Some("_mino_agent"));
    }

    #[test]
    fn session_deserialize_backward_compat() {
        // Old format without new fields — must deserialize with defaults
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000000",
            "name": "old-session",
            "project_dir": "/project",
            "command": ["bash"],
            "container_id": null,
            "status": "running",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "cloud_providers": []
        }"#;
        let session: Session = serde_json::from_str(json).unwrap();
        assert_eq!(session.name, "old-session");
        assert!(session.runtime_mode.is_none());
        assert!(session.process_id.is_none());
        assert!(session.log_file.is_none());
        assert!(session.home_volume.is_none());
        assert!(session.sandbox_user.is_none());
    }

    #[test]
    fn session_new_fields_default_none() {
        let session = Session::new(
            "test".to_string(),
            PathBuf::from("/project"),
            vec!["bash".to_string()],
            SessionStatus::Starting,
        );
        assert!(session.runtime_mode.is_none());
        assert!(session.process_id.is_none());
        assert!(session.log_file.is_none());
        assert!(session.sandbox_user.is_none());
    }

    // -- SessionStatus Display tests --

    #[test]
    fn status_display() {
        assert_eq!(SessionStatus::Starting.to_string(), "starting");
        assert_eq!(SessionStatus::Running.to_string(), "running");
        assert_eq!(SessionStatus::Stopped.to_string(), "stopped");
        assert_eq!(SessionStatus::Failed.to_string(), "failed");
    }
}
