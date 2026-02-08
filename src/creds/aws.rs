//! AWS credential provider using AWS CLI

use crate::config::schema::AwsConfig;
use crate::credentials::cache::{CachedCredential, CredentialCache};
use crate::error::{MinoError, MinoResult};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::process::Stdio;
use tokio::process::Command;
use tracing::debug;

/// AWS session credentials
#[derive(Debug, Clone)]
pub struct AwsSessionCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// AWS credential provider
pub struct AwsCredentials;

impl AwsCredentials {
    const CACHE_KEY: &'static str = "aws-session";

    /// Get session credentials, using cache if valid
    pub async fn get_session_token(
        config: &AwsConfig,
        cache: &CredentialCache,
    ) -> MinoResult<AwsSessionCredentials> {
        // Check cache first
        if let Some(cached) = cache.get(Self::CACHE_KEY).await? {
            debug!("Using cached AWS credentials");
            return Self::parse_cached(&cached);
        }

        // Generate new credentials
        let creds = if config.role_arn.is_some() {
            Self::assume_role(config).await?
        } else {
            Self::get_session_token_internal(config).await?
        };

        // Cache the credentials
        if let Some(expires_at) = creds.expires_at {
            let cached = CachedCredential::new(
                "aws",
                serde_json::to_string(&SerializableAwsCreds {
                    access_key_id: creds.access_key_id.clone(),
                    secret_access_key: creds.secret_access_key.clone(),
                    session_token: creds.session_token.clone(),
                })
                .unwrap(),
                expires_at,
            );
            cache.set(Self::CACHE_KEY, &cached).await?;
        }

        Ok(creds)
    }

    /// Get session token using AWS CLI
    async fn get_session_token_internal(
        config: &AwsConfig,
    ) -> MinoResult<AwsSessionCredentials> {
        debug!("Requesting AWS session token via CLI...");

        let mut cmd = Command::new("aws");
        cmd.args(["sts", "get-session-token"]);
        cmd.args([
            "--duration-seconds",
            &config.session_duration_secs.to_string(),
        ]);
        cmd.args(["--output", "json"]);

        if let Some(profile) = &config.profile {
            cmd.args(["--profile", profile]);
        }

        if let Some(region) = &config.region {
            cmd.args(["--region", region]);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| MinoError::command_failed("aws sts get-session-token", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Unable to locate credentials") || stderr.contains("not configured")
            {
                return Err(MinoError::AwsNotConfigured);
            }
            return Err(MinoError::AwsSts(stderr.to_string()));
        }

        let response: StsResponse = serde_json::from_slice(&output.stdout)
            .map_err(|e| MinoError::AwsSts(format!("Failed to parse response: {}", e)))?;

        let expires_at = DateTime::parse_from_rfc3339(&response.credentials.expiration)
            .map(|dt| dt.with_timezone(&Utc))
            .ok();

        Ok(AwsSessionCredentials {
            access_key_id: response.credentials.access_key_id,
            secret_access_key: response.credentials.secret_access_key,
            session_token: Some(response.credentials.session_token),
            expires_at,
        })
    }

    /// Assume an IAM role
    async fn assume_role(config: &AwsConfig) -> MinoResult<AwsSessionCredentials> {
        let role_arn = config
            .role_arn
            .as_ref()
            .ok_or_else(|| MinoError::AwsSts("No role ARN configured".to_string()))?;

        debug!("Assuming AWS role: {}", role_arn);

        let mut cmd = Command::new("aws");
        cmd.args(["sts", "assume-role"]);
        cmd.args(["--role-arn", role_arn]);
        cmd.args(["--role-session-name", "mino-session"]);
        cmd.args([
            "--duration-seconds",
            &config.session_duration_secs.to_string(),
        ]);
        cmd.args(["--output", "json"]);

        if let Some(external_id) = &config.external_id {
            cmd.args(["--external-id", external_id]);
        }

        if let Some(profile) = &config.profile {
            cmd.args(["--profile", profile]);
        }

        if let Some(region) = &config.region {
            cmd.args(["--region", region]);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| MinoError::command_failed("aws sts assume-role", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MinoError::AwsSts(stderr.to_string()));
        }

        let response: AssumeRoleResponse = serde_json::from_slice(&output.stdout)
            .map_err(|e| MinoError::AwsSts(format!("Failed to parse response: {}", e)))?;

        let expires_at = DateTime::parse_from_rfc3339(&response.credentials.expiration)
            .map(|dt| dt.with_timezone(&Utc))
            .ok();

        Ok(AwsSessionCredentials {
            access_key_id: response.credentials.access_key_id,
            secret_access_key: response.credentials.secret_access_key,
            session_token: Some(response.credentials.session_token),
            expires_at,
        })
    }

    fn parse_cached(cached: &CachedCredential) -> MinoResult<AwsSessionCredentials> {
        let creds: SerializableAwsCreds = serde_json::from_str(&cached.value)?;
        Ok(AwsSessionCredentials {
            access_key_id: creds.access_key_id,
            secret_access_key: creds.secret_access_key,
            session_token: creds.session_token,
            expires_at: Some(cached.expires_at),
        })
    }

    /// Check if AWS CLI is configured
    pub async fn is_configured() -> bool {
        let result = Command::new("aws")
            .args(["sts", "get-caller-identity"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        result.map(|s| s.success()).unwrap_or(false)
    }
}

#[derive(Deserialize)]
struct StsResponse {
    #[serde(rename = "Credentials")]
    credentials: StsCredentials,
}

#[derive(Deserialize)]
struct AssumeRoleResponse {
    #[serde(rename = "Credentials")]
    credentials: StsCredentials,
}

#[derive(Deserialize)]
struct StsCredentials {
    #[serde(rename = "AccessKeyId")]
    access_key_id: String,
    #[serde(rename = "SecretAccessKey")]
    secret_access_key: String,
    #[serde(rename = "SessionToken")]
    session_token: String,
    #[serde(rename = "Expiration")]
    expiration: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SerializableAwsCreds {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializable_creds_roundtrip() {
        let creds = SerializableAwsCreds {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: Some("token123".to_string()),
        };

        let json = serde_json::to_string(&creds).unwrap();
        let parsed: SerializableAwsCreds = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.access_key_id, creds.access_key_id);
    }

    #[test]
    fn parse_sts_response() {
        let json = r#"{
            "Credentials": {
                "AccessKeyId": "ASIAIOSFODNN7EXAMPLE",
                "SecretAccessKey": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
                "SessionToken": "FwoGZXIvYXdzEB...",
                "Expiration": "2024-01-01T12:00:00Z"
            }
        }"#;

        let response: StsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.credentials.access_key_id, "ASIAIOSFODNN7EXAMPLE");
    }
}
