//! Configuration schema for Minotaur
//!
//! Configuration is stored at `~/.config/minotaur/config.toml`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Root configuration structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// General settings
    pub general: GeneralConfig,

    /// OrbStack VM settings
    pub vm: VmConfig,

    /// Container settings
    pub container: ContainerConfig,

    /// Cloud credential settings
    pub credentials: CredentialsConfig,

    /// Session defaults
    pub session: SessionConfig,

    /// Cache settings
    pub cache: CacheConfig,
}


/// General application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Enable verbose logging
    pub verbose: bool,

    /// Log format: "text" or "json"
    pub log_format: String,

    /// Enable audit logging
    pub audit_log: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            verbose: false,
            log_format: "text".to_string(),
            audit_log: true,
        }
    }
}

/// OrbStack VM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VmConfig {
    /// VM name to use
    pub name: String,

    /// VM distribution
    pub distro: String,

    /// CPU cores allocated
    pub cpus: Option<u32>,

    /// Memory in MB
    pub memory_mb: Option<u32>,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            name: "minotaur".to_string(),
            distro: "fedora".to_string(),
            cpus: None,
            memory_mb: None,
        }
    }
}

/// Container configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainerConfig {
    /// Base image to use
    pub image: String,

    /// Additional packages to install
    pub packages: Vec<String>,

    /// Environment variables to set
    pub env: HashMap<String, String>,

    /// Additional volume mounts (host:container)
    pub volumes: Vec<String>,

    /// Network mode
    pub network: String,

    /// Working directory inside container
    pub workdir: String,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            image: "fedora:41".to_string(),
            packages: vec![
                "git".to_string(),
                "curl".to_string(),
                "which".to_string(),
            ],
            env: HashMap::new(),
            volumes: vec![],
            network: "host".to_string(),
            workdir: "/workspace".to_string(),
        }
    }
}

/// Cloud credentials configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CredentialsConfig {
    /// AWS settings
    pub aws: AwsConfig,

    /// GCP settings
    pub gcp: GcpConfig,

    /// Azure settings
    pub azure: AzureConfig,

    /// GitHub settings
    pub github: GithubConfig,
}


/// AWS credential settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AwsConfig {
    /// Session token duration in seconds (default: 1 hour)
    pub session_duration_secs: u32,

    /// IAM role to assume (optional)
    pub role_arn: Option<String>,

    /// External ID for role assumption (optional)
    pub external_id: Option<String>,

    /// AWS profile to use
    pub profile: Option<String>,

    /// AWS region
    pub region: Option<String>,
}

impl Default for AwsConfig {
    fn default() -> Self {
        Self {
            session_duration_secs: 3600,
            role_arn: None,
            external_id: None,
            profile: None,
            region: None,
        }
    }
}

/// GCP credential settings
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GcpConfig {
    /// GCP project ID
    pub project: Option<String>,

    /// Service account to impersonate
    pub service_account: Option<String>,
}


/// Azure credential settings
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AzureConfig {
    /// Azure subscription ID
    pub subscription: Option<String>,

    /// Azure tenant ID
    pub tenant: Option<String>,
}


/// GitHub credential settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubConfig {
    /// GitHub host (for GitHub Enterprise)
    pub host: String,
}

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            host: "github.com".to_string(),
        }
    }
}

/// Session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Default shell inside container
    pub shell: String,

    /// Auto-cleanup sessions older than N hours (0 = disabled)
    pub auto_cleanup_hours: u32,

    /// Maximum concurrent sessions
    pub max_sessions: u32,

    /// Default project directory to mount
    pub default_project_dir: Option<PathBuf>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            shell: "/bin/bash".to_string(),
            auto_cleanup_hours: 24,
            max_sessions: 10,
            default_project_dir: None,
        }
    }
}

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Enable dependency caching (default: true)
    pub enabled: bool,

    /// Auto-remove caches older than N days (0 = disabled)
    pub gc_days: u32,

    /// Maximum total cache size in GB before triggering gc
    pub max_total_gb: u32,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gc_days: 30,
            max_total_gb: 50,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_serializes() {
        let config = Config::default();
        let toml = toml::to_string_pretty(&config).unwrap();
        assert!(toml.contains("[general]"));
        assert!(toml.contains("[vm]"));
    }

    #[test]
    fn config_deserializes_empty() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.vm.name, "minotaur");
    }

    #[test]
    fn config_deserializes_partial() {
        let toml = r#"
            [vm]
            name = "custom-vm"
        "#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.vm.name, "custom-vm");
        assert_eq!(config.container.image, "fedora:41"); // default preserved
    }
}
