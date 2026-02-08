//! Audit logging for security events
//!
//! Writes JSON lines to `~/.local/share/minotaur/audit.log`.
//! Always-on by default (security tool — audit should be opt-out, not opt-in).

use crate::config::{schema::Config, ConfigManager};
use chrono::Utc;
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::warn;

/// File-based audit logger that appends JSON lines
pub struct AuditLog {
    enabled: bool,
    path: PathBuf,
}

impl AuditLog {
    /// Create a new audit logger from config
    pub fn new(config: &Config) -> Self {
        Self {
            enabled: config.general.audit_log,
            path: ConfigManager::audit_log_path(),
        }
    }

    /// Log an audit event as a JSON line
    ///
    /// Silently drops events on IO failure — audit logging must never
    /// block or crash the primary workflow.
    pub async fn log(&self, event: &str, data: &serde_json::Value) {
        if !self.enabled {
            return;
        }

        let entry = serde_json::json!({
            "timestamp": Utc::now().to_rfc3339(),
            "event": event,
            "data": data,
        });

        let mut line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to serialize audit event: {}", e);
                return;
            }
        };
        line.push('\n');

        if let Err(e) = self.append(&line).await {
            warn!("Failed to write audit log: {}", e);
        }
    }

    async fn append(&self, line: &str) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_audit_log(dir: &TempDir, enabled: bool) -> AuditLog {
        AuditLog {
            enabled,
            path: dir.path().join("audit.log"),
        }
    }

    #[tokio::test]
    async fn writes_json_line() {
        let dir = TempDir::new().unwrap();
        let audit = test_audit_log(&dir, true);

        audit
            .log(
                "session.created",
                &serde_json::json!({"name": "test-session"}),
            )
            .await;

        let content = tokio::fs::read_to_string(&audit.path).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();

        assert_eq!(parsed["event"], "session.created");
        assert_eq!(parsed["data"]["name"], "test-session");
        assert!(parsed["timestamp"].is_string());
    }

    #[tokio::test]
    async fn appends_multiple_lines() {
        let dir = TempDir::new().unwrap();
        let audit = test_audit_log(&dir, true);

        audit.log("event.one", &serde_json::json!({})).await;
        audit.log("event.two", &serde_json::json!({})).await;

        let content = tokio::fs::read_to_string(&audit.path).await.unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[tokio::test]
    async fn skips_when_disabled() {
        let dir = TempDir::new().unwrap();
        let audit = test_audit_log(&dir, false);

        audit.log("should.not.appear", &serde_json::json!({})).await;

        assert!(!audit.path.exists());
    }
}
