//! GCP credential provider using gcloud CLI

use crate::config::schema::GcpConfig;
use crate::credentials::cache::{CachedCredential, CredentialCache};
use crate::error::{MinotaurError, MinotaurResult};
use chrono::{Duration, Utc};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info};

/// GCP credential provider
pub struct GcpCredentials;

impl GcpCredentials {
    const CACHE_KEY: &'static str = "gcp-token";

    /// Get access token, using cache if valid
    pub async fn get_access_token(
        config: &GcpConfig,
        cache: &CredentialCache,
    ) -> MinotaurResult<String> {
        // Check cache first
        if let Some(cached) = cache.get(Self::CACHE_KEY).await? {
            debug!("Using cached GCP access token");
            return Ok(cached.value);
        }

        // Generate new token
        let token = Self::get_access_token_internal(config).await?;

        // Cache for 55 minutes (tokens are valid for 1 hour)
        let expires_at = Utc::now() + Duration::minutes(55);
        let cached = CachedCredential::new("gcp", token.clone(), expires_at);
        cache.set(Self::CACHE_KEY, &cached).await?;

        Ok(token)
    }

    /// Get access token from gcloud CLI
    async fn get_access_token_internal(config: &GcpConfig) -> MinotaurResult<String> {
        info!("Requesting GCP access token...");

        let mut cmd = Command::new("gcloud");
        cmd.args(["auth", "print-access-token"]);

        if let Some(account) = &config.service_account {
            cmd.args(["--impersonate-service-account", account]);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("gcloud auth print-access-token", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not logged in") || stderr.contains("no active account") {
                return Err(MinotaurError::GcpNotAuthenticated);
            }
            return Err(MinotaurError::GcpCredential(stderr.to_string()));
        }

        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if token.is_empty() {
            return Err(MinotaurError::GcpCredential("Empty token returned".to_string()));
        }

        Ok(token)
    }

    /// Check if gcloud is authenticated
    pub async fn is_authenticated() -> bool {
        let result = Command::new("gcloud")
            .args(["auth", "print-identity-token"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        result.map(|s| s.success()).unwrap_or(false)
    }

    /// Get the current project
    pub async fn get_project() -> MinotaurResult<Option<String>> {
        let output = Command::new("gcloud")
            .args(["config", "get-value", "project"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("gcloud config get-value project", e))?;

        if output.status.success() {
            let project = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if project.is_empty() || project == "(unset)" {
                Ok(None)
            } else {
                Ok(Some(project))
            }
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn project_returns_option() {
        // This test just verifies the function doesn't panic
        let _ = GcpCredentials::get_project().await;
    }
}
