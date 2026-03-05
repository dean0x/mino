//! Credential gathering for cloud providers and GitHub

use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::credentials::{
    AwsCredentials, AzureCredentials, CredentialCache, GcpCredentials, GithubCredentials,
};
use crate::error::MinoResult;
use std::collections::HashMap;
use tracing::debug;

/// Returns (env_vars, successfully loaded providers, failed providers with errors)
pub(super) async fn gather_credentials(
    args: &RunArgs,
    config: &Config,
) -> MinoResult<(HashMap<String, String>, Vec<String>, Vec<(String, String)>)> {
    let mut env_vars = HashMap::new();
    let mut providers = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let cache = CredentialCache::new().await?;

    let (use_aws, use_gcp, use_azure) = if args.all_clouds {
        (true, true, true)
    } else {
        (
            args.aws || config.credentials.aws.enabled,
            args.gcp || config.credentials.gcp.enabled,
            args.azure || config.credentials.azure.enabled,
        )
    };

    // AWS credentials
    if use_aws {
        debug!("Fetching AWS credentials...");
        match AwsCredentials::get_session_token(&config.credentials.aws, &cache).await {
            Ok(creds) => {
                env_vars.insert("AWS_ACCESS_KEY_ID".to_string(), creds.access_key_id);
                env_vars.insert("AWS_SECRET_ACCESS_KEY".to_string(), creds.secret_access_key);
                if let Some(token) = creds.session_token {
                    env_vars.insert("AWS_SESSION_TOKEN".to_string(), token);
                }
                if let Some(region) = &config.credentials.aws.region {
                    env_vars.insert("AWS_REGION".to_string(), region.clone());
                    env_vars.insert("AWS_DEFAULT_REGION".to_string(), region.clone());
                }
                providers.push("aws".to_string());
                debug!("AWS credentials loaded");
            }
            Err(e) => {
                failures.push(("AWS".to_string(), e.to_string()));
            }
        }
    }

    // GCP credentials
    if use_gcp {
        debug!("Fetching GCP credentials...");
        match GcpCredentials::get_access_token(&config.credentials.gcp, &cache).await {
            Ok(token) => {
                env_vars.insert("CLOUDSDK_AUTH_ACCESS_TOKEN".to_string(), token);
                if let Some(project) = &config.credentials.gcp.project {
                    env_vars.insert("CLOUDSDK_CORE_PROJECT".to_string(), project.clone());
                }
                providers.push("gcp".to_string());
                debug!("GCP credentials loaded");
            }
            Err(e) => {
                failures.push(("GCP".to_string(), e.to_string()));
            }
        }
    }

    // Azure credentials
    if use_azure {
        debug!("Fetching Azure credentials...");
        match AzureCredentials::get_access_token(&config.credentials.azure, &cache).await {
            Ok(token) => {
                env_vars.insert("AZURE_ACCESS_TOKEN".to_string(), token);
                providers.push("azure".to_string());
                debug!("Azure credentials loaded");
            }
            Err(e) => {
                failures.push(("Azure".to_string(), e.to_string()));
            }
        }
    }

    // GitHub token
    if !args.no_github {
        debug!("Fetching GitHub token...");
        match GithubCredentials::get_token(&config.credentials.github).await {
            Ok(token) => {
                env_vars.insert("GITHUB_TOKEN".to_string(), token.clone());
                env_vars.insert("GH_TOKEN".to_string(), token);
                providers.push("github".to_string());
                debug!("GitHub token loaded");
            }
            Err(e) => {
                failures.push(("GitHub".to_string(), e.to_string()));
            }
        }
    }

    // Add user-specified env vars
    for (key, value) in &args.env {
        env_vars.insert(key.clone(), value.clone());
    }

    Ok((env_vars, providers, failures))
}
