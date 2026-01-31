//! Credential providers for cloud services

pub mod aws;
pub mod azure;
pub mod cache;
pub mod gcp;
pub mod github;

pub use aws::AwsCredentials;
pub use azure::AzureCredentials;
pub use cache::CredentialCache;
pub use gcp::GcpCredentials;
pub use github::GithubCredentials;
