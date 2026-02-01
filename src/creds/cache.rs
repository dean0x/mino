//! Credential caching with TTL support

use crate::config::ConfigManager;
use crate::error::{MinotaurError, MinotaurResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tracing::debug;

/// Cached credential entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCredential {
    /// The credential value
    pub value: String,

    /// When the credential expires
    pub expires_at: DateTime<Utc>,

    /// Provider name (aws, gcp, azure, github)
    pub provider: String,
}

impl CachedCredential {
    /// Create a new cached credential
    pub fn new(provider: &str, value: String, expires_at: DateTime<Utc>) -> Self {
        Self {
            value,
            expires_at,
            provider: provider.to_string(),
        }
    }

    /// Check if credential is expired
    pub fn is_expired(&self) -> bool {
        // Add 60 second buffer to prevent using almost-expired creds
        Utc::now() >= self.expires_at - chrono::Duration::seconds(60)
    }
}

/// Credential cache manager
pub struct CredentialCache {
    cache_dir: PathBuf,
}

impl CredentialCache {
    /// Create a new credential cache
    pub async fn new() -> MinotaurResult<Self> {
        let cache_dir = ConfigManager::credentials_dir();
        fs::create_dir_all(&cache_dir)
            .await
            .map_err(|e| MinotaurError::io("creating credentials cache dir", e))?;

        // Set restrictive permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(&cache_dir, perms)
                .map_err(|e| MinotaurError::io("setting credentials dir permissions", e))?;
        }

        Ok(Self { cache_dir })
    }

    /// Get a cached credential if valid
    pub async fn get(&self, key: &str) -> MinotaurResult<Option<CachedCredential>> {
        let path = self.cache_path(key);

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| MinotaurError::io(format!("reading cache file {}", path.display()), e))?;

        let cred: CachedCredential = serde_json::from_str(&content)?;

        if cred.is_expired() {
            debug!("Cached credential {} is expired", key);
            self.remove(key).await?;
            return Ok(None);
        }

        debug!("Using cached credential {}", key);
        Ok(Some(cred))
    }

    /// Store a credential in cache
    pub async fn set(&self, key: &str, cred: &CachedCredential) -> MinotaurResult<()> {
        let path = self.cache_path(key);
        let content = serde_json::to_string_pretty(cred)?;

        fs::write(&path, content)
            .await
            .map_err(|e| MinotaurError::io(format!("writing cache file {}", path.display()), e))?;

        // Set restrictive permissions on credential file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perms)
                .map_err(|e| MinotaurError::io("setting cache file permissions", e))?;
        }

        debug!("Cached credential {} until {}", key, cred.expires_at);
        Ok(())
    }

    /// Remove a cached credential
    pub async fn remove(&self, key: &str) -> MinotaurResult<()> {
        let path = self.cache_path(key);
        if path.exists() {
            fs::remove_file(&path).await.map_err(|e| {
                MinotaurError::io(format!("removing cache file {}", path.display()), e)
            })?;
        }
        Ok(())
    }

    /// Clear all cached credentials
    pub async fn clear(&self) -> MinotaurResult<()> {
        let mut entries = fs::read_dir(&self.cache_dir)
            .await
            .map_err(|e| MinotaurError::io("reading cache directory", e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| MinotaurError::io("reading cache entry", e))?
        {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                fs::remove_file(entry.path())
                    .await
                    .map_err(|e| MinotaurError::io("removing cache file", e))?;
            }
        }

        Ok(())
    }

    fn cache_path(&self, key: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.json", key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn test_cache() -> (CredentialCache, TempDir) {
        let temp = TempDir::new().unwrap();
        let cache = CredentialCache {
            cache_dir: temp.path().to_path_buf(),
        };
        (cache, temp)
    }

    #[tokio::test]
    async fn cache_set_and_get() {
        let (cache, _temp) = test_cache().await;

        let cred = CachedCredential::new(
            "test",
            "secret123".to_string(),
            Utc::now() + chrono::Duration::hours(1),
        );

        cache.set("test-key", &cred).await.unwrap();
        let retrieved = cache.get("test-key").await.unwrap().unwrap();

        assert_eq!(retrieved.value, "secret123");
        assert_eq!(retrieved.provider, "test");
    }

    #[tokio::test]
    async fn cache_expired_returns_none() {
        let (cache, _temp) = test_cache().await;

        let cred = CachedCredential::new(
            "test",
            "secret123".to_string(),
            Utc::now() - chrono::Duration::hours(1), // Already expired
        );

        cache.set("test-key", &cred).await.unwrap();
        let retrieved = cache.get("test-key").await.unwrap();

        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn cache_missing_returns_none() {
        let (cache, _temp) = test_cache().await;
        let retrieved = cache.get("nonexistent").await.unwrap();
        assert!(retrieved.is_none());
    }
}
