//! Podman shared types
//!
//! Contains data structures used by container runtimes.
//! The actual Podman execution logic is in the runtime implementations.

use std::collections::HashMap;

/// Container configuration for running a new container
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Container image to use
    pub image: String,
    /// Working directory inside the container
    pub workdir: String,
    /// Volume mounts (host:container format)
    pub volumes: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Network mode
    pub network: String,
    /// Enable interactive mode
    pub interactive: bool,
    /// Allocate a TTY
    pub tty: bool,
}

/// Information about a running container
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    /// Container ID
    pub id: String,
    /// Container name
    pub name: String,
    /// Container status (e.g., "Up 2 hours")
    pub status: String,
    /// Container image
    pub image: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_config_default() {
        let config = ContainerConfig {
            image: "fedora:43".to_string(),
            workdir: "/workspace".to_string(),
            volumes: vec![],
            env: HashMap::new(),
            network: "host".to_string(),
            interactive: true,
            tty: true,
        };

        assert_eq!(config.image, "fedora:43");
    }
}
