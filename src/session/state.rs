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
    pub async fn create_file(&self) -> MinoResult<()> {
        validate_session_name(&self.name)?;
        let path = self.file_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| MinoError::io("creating sessions directory", e))?;
        }

        let content = serde_json::to_string_pretty(self)?;

        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    MinoError::SessionExists(self.name.clone())
                } else {
                    MinoError::io(format!("creating session file {}", path.display()), e)
                }
            })?;

        use tokio::io::AsyncWriteExt;
        file.write_all(content.as_bytes())
            .await
            .map_err(|e| MinoError::io(format!("writing session file {}", path.display()), e))?;

        Ok(())
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

    // -- SessionStatus Display tests --

    #[test]
    fn status_display() {
        assert_eq!(SessionStatus::Starting.to_string(), "starting");
        assert_eq!(SessionStatus::Running.to_string(), "running");
        assert_eq!(SessionStatus::Stopped.to_string(), "stopped");
        assert_eq!(SessionStatus::Failed.to_string(), "failed");
    }
}
