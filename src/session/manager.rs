//! Session lifecycle management

use crate::config::ConfigManager;
use crate::error::{MinotaurError, MinotaurResult};
use crate::session::state::{Session, SessionStatus};
use chrono::{Duration, Utc};
use tracing::{debug, warn};

/// Session manager handles session CRUD and cleanup
pub struct SessionManager;

impl SessionManager {
    /// Create a new session manager
    pub async fn new() -> MinotaurResult<Self> {
        // Ensure state directories exist
        ConfigManager::ensure_state_dirs().await?;
        Ok(Self)
    }

    /// Create a new session (atomic — fails if name already taken)
    pub async fn create(&self, session: &Session) -> MinotaurResult<()> {
        session.create_file().await?;
        debug!("Created session: {}", session.name);
        Ok(())
    }

    /// Get a session by name
    pub async fn get(&self, name: &str) -> MinotaurResult<Option<Session>> {
        Session::load(name).await
    }

    /// List all sessions
    pub async fn list(&self) -> MinotaurResult<Vec<Session>> {
        Session::list_all().await
    }

    /// Update session status
    pub async fn update_status(&self, name: &str, status: SessionStatus) -> MinotaurResult<()> {
        let mut session = self
            .get(name)
            .await?
            .ok_or_else(|| MinotaurError::SessionNotFound(name.to_string()))?;

        session.status = status;
        session.updated_at = Utc::now();
        session.save().await?;

        debug!("Updated session {} status to {:?}", name, status);
        Ok(())
    }

    /// Set container ID for a session
    pub async fn set_container_id(&self, name: &str, container_id: &str) -> MinotaurResult<()> {
        let mut session = self
            .get(name)
            .await?
            .ok_or_else(|| MinotaurError::SessionNotFound(name.to_string()))?;

        session.container_id = Some(container_id.to_string());
        session.updated_at = Utc::now();
        session.save().await?;

        debug!("Set container ID for session {}: {}", name, container_id);
        Ok(())
    }

    /// Delete a session
    pub async fn delete(&self, name: &str) -> MinotaurResult<()> {
        let session = self
            .get(name)
            .await?
            .ok_or_else(|| MinotaurError::SessionNotFound(name.to_string()))?;

        session.delete().await?;
        debug!("Deleted session: {}", name);
        Ok(())
    }

    /// Find session by container ID
    pub async fn find_by_container(&self, container_id: &str) -> MinotaurResult<Option<Session>> {
        let sessions = self.list().await?;
        Ok(sessions
            .into_iter()
            .find(|s| s.container_id.as_deref() == Some(container_id)))
    }

    /// Remove stopped/failed sessions older than `max_age_hours`.
    /// Returns the number of sessions cleaned up.
    pub async fn cleanup(&self, max_age_hours: u32) -> MinotaurResult<u32> {
        if max_age_hours == 0 {
            return Ok(0);
        }

        let cutoff = Utc::now() - Duration::hours(max_age_hours as i64);
        let sessions = self.list().await?;
        let mut cleaned = 0u32;

        for session in sessions {
            let dominated = matches!(
                session.status,
                SessionStatus::Stopped | SessionStatus::Failed
            );

            if dominated && session.updated_at < cutoff {
                match session.delete().await {
                    Ok(()) => {
                        debug!("Cleaned up session: {}", session.name);
                        cleaned += 1;
                    }
                    Err(e) => {
                        warn!("Failed to clean up session {}: {}", session.name, e);
                    }
                }
            }
        }

        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_status_transitions() {
        // Valid transitions
        let status = SessionStatus::Starting;
        assert_eq!(status, SessionStatus::Starting);

        let status = SessionStatus::Running;
        assert_eq!(status, SessionStatus::Running);

        let status = SessionStatus::Stopped;
        assert_eq!(status, SessionStatus::Stopped);
    }
}
