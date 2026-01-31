//! Error types for Minotaur
//!
//! All modules use `MinotaurResult<T>` as their return type.

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for Minotaur operations
pub type MinotaurResult<T> = Result<T, MinotaurError>;

/// All errors that can occur in Minotaur
#[derive(Error, Debug)]
pub enum MinotaurError {
    // Environment errors
    #[error("OrbStack not found. Install from https://orbstack.dev or run: brew install orbstack")]
    OrbStackNotFound,

    #[error("OrbStack is not running. Start it with: orb start")]
    OrbStackNotRunning,

    #[error("Podman not available in OrbStack VM. Run: orb -m <vm> sudo dnf install -y podman")]
    PodmanNotFound,

    #[error("Unsupported platform: {0}. Minotaur supports macOS and Linux.")]
    UnsupportedPlatform(String),

    #[error("Podman rootless setup incomplete: {reason}")]
    PodmanRootlessSetup { reason: String },

    #[error("Required CLI not found: {name}. {hint}")]
    CliNotFound { name: String, hint: String },

    // Configuration errors
    #[error("Invalid configuration at {path}: {reason}")]
    ConfigInvalid { path: PathBuf, reason: String },

    #[error("Configuration file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error("Failed to create config directory {path}: {source}")]
    ConfigDirCreate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    // Credential errors
    #[error("AWS credentials not configured. Run: aws configure")]
    AwsNotConfigured,

    #[error("AWS STS error: {0}")]
    AwsSts(String),

    #[error("GCP not authenticated. Run: gcloud auth login")]
    GcpNotAuthenticated,

    #[error("GCP credential error: {0}")]
    GcpCredential(String),

    #[error("Azure not authenticated. Run: az login")]
    AzureNotAuthenticated,

    #[error("Azure credential error: {0}")]
    AzureCredential(String),

    #[error("GitHub CLI not authenticated. Run: gh auth login")]
    GithubNotAuthenticated,

    #[error("Credential expired for {provider}, refresh required")]
    CredentialExpired { provider: String },

    // Session errors
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Session already exists: {0}")]
    SessionExists(String),

    #[error("Failed to persist session state: {0}")]
    SessionPersist(String),

    #[error("No active sessions")]
    NoActiveSessions,

    // Container errors
    #[error("Container failed to start: {0}")]
    ContainerStart(String),

    #[error("Container not found: {0}")]
    ContainerNotFound(String),

    #[error("Container command failed: {command}, exit code: {code}")]
    ContainerCommand { command: String, code: i32 },

    #[error("Image pull failed: {image}: {reason}")]
    ImagePull { image: String, reason: String },

    // VM errors
    #[error("VM not found: {0}")]
    VmNotFound(String),

    #[error("VM failed to start: {0}")]
    VmStart(String),

    #[error("VM command failed: {0}")]
    VmCommand(String),

    // Cache errors
    #[error("Failed to create cache volume {name}: {reason}")]
    CacheVolumeCreate { name: String, reason: String },

    #[error("Cache volume not found: {0}")]
    CacheVolumeNotFound(String),

    #[error("Failed to read lockfile {path}: {reason}")]
    CacheLockfileRead { path: String, reason: String },

    // IO errors
    #[error("IO error: {context}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Path not found: {0}")]
    PathNotFound(PathBuf),

    #[error("Invalid path: {path}: {reason}")]
    PathInvalid { path: PathBuf, reason: String },

    // Process errors
    #[error("Command failed: {command}")]
    CommandFailed {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Command execution error: {command}, stderr: {stderr}")]
    CommandExecution { command: String, stderr: String },

    #[error("Process terminated by signal")]
    ProcessSignaled,

    // Serialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    // General errors
    #[error("Internal error: {0}")]
    Internal(String),

    #[error("{0}")]
    User(String),
}

impl MinotaurError {
    /// Create an IO error with context
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Create a command failed error
    pub fn command_failed(command: impl Into<String>, source: std::io::Error) -> Self {
        Self::CommandFailed {
            command: command.into(),
            source,
        }
    }

    /// Create a command execution error
    pub fn command_exec(command: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self::CommandExecution {
            command: command.into(),
            stderr: stderr.into(),
        }
    }

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::CredentialExpired { .. }
                | Self::OrbStackNotRunning
                | Self::ContainerStart(_)
                | Self::VmStart(_)
        )
    }

    /// Get actionable hint for the error
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::OrbStackNotFound => Some("Install OrbStack from https://orbstack.dev"),
            Self::OrbStackNotRunning => Some("Run: orb start"),
            Self::AwsNotConfigured => Some("Run: aws configure"),
            Self::GcpNotAuthenticated => Some("Run: gcloud auth login"),
            Self::AzureNotAuthenticated => Some("Run: az login"),
            Self::GithubNotAuthenticated => Some("Run: gh auth login"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let err = MinotaurError::OrbStackNotFound;
        assert!(err.to_string().contains("OrbStack not found"));
    }

    #[test]
    fn error_hint() {
        let err = MinotaurError::AwsNotConfigured;
        assert_eq!(err.hint(), Some("Run: aws configure"));
    }

    #[test]
    fn error_retryable() {
        assert!(MinotaurError::OrbStackNotRunning.is_retryable());
        assert!(!MinotaurError::OrbStackNotFound.is_retryable());
    }
}
