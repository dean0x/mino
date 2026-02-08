//! GitHub credential provider using gh CLI

use crate::config::schema::GithubConfig;
use crate::error::{MinoError, MinoResult};
use std::process::Stdio;
use tokio::process::Command;
use tracing::debug;

/// GitHub credential provider
pub struct GithubCredentials;

impl GithubCredentials {
    /// Get GitHub token from gh CLI
    pub async fn get_token(config: &GithubConfig) -> MinoResult<String> {
        debug!("Getting GitHub token from gh CLI...");

        let mut cmd = Command::new("gh");
        cmd.args(["auth", "token"]);

        if config.host != "github.com" {
            cmd.args(["--hostname", &config.host]);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| MinoError::command_failed("gh auth token", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not logged in") || stderr.contains("gh auth login") {
                return Err(MinoError::GithubNotAuthenticated);
            }
            return Err(MinoError::User(format!(
                "gh auth token failed: {}",
                stderr
            )));
        }

        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if token.is_empty() {
            return Err(MinoError::GithubNotAuthenticated);
        }

        Ok(token)
    }

    /// Check if gh CLI is authenticated
    pub async fn is_authenticated(config: &GithubConfig) -> bool {
        let mut cmd = Command::new("gh");
        cmd.args(["auth", "status"]);

        if config.host != "github.com" {
            cmd.args(["--hostname", &config.host]);
        }

        let result = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        result.map(|s| s.success()).unwrap_or(false)
    }

    /// Get the authenticated user
    pub async fn get_user(config: &GithubConfig) -> MinoResult<Option<String>> {
        let mut cmd = Command::new("gh");
        cmd.args(["api", "user", "--jq", ".login"]);

        if config.host != "github.com" {
            cmd.args(["--hostname", &config.host]);
        }

        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| MinoError::command_failed("gh api user", e))?;

        if output.status.success() {
            let user = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if user.is_empty() {
                Ok(None)
            } else {
                Ok(Some(user))
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
    async fn is_authenticated_returns_bool() {
        let config = GithubConfig::default();
        // Just verify it doesn't panic
        let _ = GithubCredentials::is_authenticated(&config).await;
    }
}
