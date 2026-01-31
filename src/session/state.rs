//! Session state persistence

use crate::config::ConfigManager;
use crate::error::{MinotaurError, MinotaurResult};
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
    pub async fn load(name: &str) -> MinotaurResult<Option<Self>> {
        let path = ConfigManager::sessions_dir().join(format!("{}.json", name));

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| MinotaurError::io(format!("reading session file {}", path.display()), e))?;

        let session: Session = serde_json::from_str(&content)?;
        Ok(Some(session))
    }

    /// Save session to file
    pub async fn save(&self) -> MinotaurResult<()> {
        let path = self.file_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| MinotaurError::io("creating sessions directory", e))?;
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)
            .await
            .map_err(|e| MinotaurError::io(format!("writing session file {}", path.display()), e))?;

        Ok(())
    }

    /// Delete session file
    pub async fn delete(&self) -> MinotaurResult<()> {
        let path = self.file_path();
        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| MinotaurError::io(format!("deleting session file {}", path.display()), e))?;
        }
        Ok(())
    }

    /// List all sessions
    pub async fn list_all() -> MinotaurResult<Vec<Session>> {
        let sessions_dir = ConfigManager::sessions_dir();

        if !sessions_dir.exists() {
            return Ok(vec![]);
        }

        let mut sessions = vec![];
        let mut entries = fs::read_dir(&sessions_dir)
            .await
            .map_err(|e| MinotaurError::io("reading sessions directory", e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| MinotaurError::io("reading session entry", e))?
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
}
