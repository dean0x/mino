//! Azure credential provider using az CLI

use crate::config::schema::AzureConfig;
use crate::credentials::cache::{CachedCredential, CredentialCache};
use crate::error::{MinotaurError, MinotaurResult};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::process::Stdio;
use tokio::process::Command;
use tracing::debug;

/// Azure credential provider
pub struct AzureCredentials;

impl AzureCredentials {
    const CACHE_KEY: &'static str = "azure-token";

    /// Get access token, using cache if valid
    pub async fn get_access_token(
        config: &AzureConfig,
        cache: &CredentialCache,
    ) -> MinotaurResult<String> {
        // Check cache first
        if let Some(cached) = cache.get(Self::CACHE_KEY).await? {
            debug!("Using cached Azure access token");
            return Ok(cached.value);
        }

        // Generate new token
        let (token, expires_at) = Self::get_access_token_internal(config).await?;

        // Cache the token
        let cached = CachedCredential::new("azure", token.clone(), expires_at);
        cache.set(Self::CACHE_KEY, &cached).await?;

        Ok(token)
    }

    /// Get access token from az CLI
    async fn get_access_token_internal(
        config: &AzureConfig,
    ) -> MinotaurResult<(String, DateTime<Utc>)> {
        debug!("Requesting Azure access token...");

        let mut cmd = Command::new("az");
        cmd.args(["account", "get-access-token", "--output", "json"]);

        if let Some(subscription) = &config.subscription {
            cmd.args(["--subscription", subscription]);
        }

        if let Some(tenant) = &config.tenant {
            cmd.args(["--tenant", tenant]);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("az account get-access-token", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("az login") || stderr.contains("not logged in") {
                return Err(MinotaurError::AzureNotAuthenticated);
            }
            return Err(MinotaurError::AzureCredential(stderr.to_string()));
        }

        let response: AzureTokenResponse = serde_json::from_slice(&output.stdout).map_err(|e| {
            MinotaurError::AzureCredential(format!("Failed to parse response: {}", e))
        })?;

        let expires_at = DateTime::parse_from_rfc3339(&response.expires_on)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now() + chrono::Duration::hours(1));

        Ok((response.access_token, expires_at))
    }

    /// Check if az CLI is authenticated
    pub async fn is_authenticated() -> bool {
        let result = Command::new("az")
            .args(["account", "show"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        result.map(|s| s.success()).unwrap_or(false)
    }

    /// Get the current subscription
    pub async fn get_subscription() -> MinotaurResult<Option<String>> {
        let output = Command::new("az")
            .args(["account", "show", "--query", "id", "-o", "tsv"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("az account show", e))?;

        if output.status.success() {
            let sub = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if sub.is_empty() {
                Ok(None)
            } else {
                Ok(Some(sub))
            }
        } else {
            Ok(None)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzureTokenResponse {
    access_token: String,
    expires_on: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_azure_response() {
        let json = r#"{
            "accessToken": "token123",
            "expiresOn": "2024-01-01T12:00:00+00:00",
            "subscription": "sub123",
            "tenant": "tenant123",
            "tokenType": "Bearer"
        }"#;

        let response: AzureTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.access_token, "token123");
    }
}
